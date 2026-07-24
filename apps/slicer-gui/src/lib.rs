/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

//! Library root for the slicer GUI crate. Owns every module that
//! previously sat under `main.rs`; the binary entry point now lives
//! in `main.rs` as a thin wrapper that calls into `run`.
//!
//! Modules touched by external integration tests
//! (`apps/slicer-gui/tests/...`) are re-published as `pub`; the rest
//! stay crate-private.

mod about_window;
mod app_settings;
mod app_state;
pub mod build_volume;
pub mod camera;
mod clipboard;
mod error_dialog;
mod gizmo;
mod gizmo_geometry;
mod i18n;
mod language;
mod material_form;
mod mesh_loader;
pub mod mesh_render;
mod pending_load;
mod picking;
mod preferences_bus;
mod preferences_window;
pub mod profile_catalog;
mod profile_forking;
mod profile_transfer;
mod profile_view;
mod scene_document;
mod scene_export;
mod scene_loader;
mod selection;
#[cfg(test)]
mod tests_mesh_loader;
#[cfg(test)]
mod tests_scene_loader;
mod viewport;
mod window_icon;
mod window_prefs;
mod wiring;

use std::error::Error;
use std::path::PathBuf;

slint::include_modules!();

use crate::app_state::AppState;
use crate::mesh_loader::is_supported_mesh_extension;

/// Entry point invoked by the binary wrapper. Builds the shared wgpu
/// stack, wires it into Slint, spawns one window per accepted CLI
/// path (or one empty window if none were supplied) and runs the
/// event loop.
pub fn run() -> Result<(), Box<dyn Error>> {
    // Build the wgpu Instance/Adapter/Device/Queue once up front and
    // hand them to Slint via `WGPUConfiguration::Manual` so every
    // top-level Window shares the same Device. Letting Slint pick
    // `Automatic` triggers a full Vulkan ICD re-enumeration per
    // window (4-5 stalls of ~225 ms each on NVIDIA, measured), which
    // makes "New Window" feel like it hangs.
    use slint::wgpu_29::wgpu;

    // Slint's Skia renderer only wires one wgpu backend per platform:
    // Dx12 on Windows, Vulkan on Linux, Metal on macOS. An instance
    // that enumerates every backend can hand Slint an adapter it
    // rejects; on Windows `request_adapter` may return a Vulkan
    // adapter, which the Skia surface refuses with "Unsupported WGPU
    // backend for use with Skia: vulkan". Restrict the instance to the
    // single backend the renderer accepts here. The cfg predicates
    // mirror i-slint-renderer-skia so the two stay in lockstep.
    #[cfg(target_family = "windows")]
    let backends = wgpu::Backends::DX12;
    #[cfg(all(target_family = "unix", not(target_vendor = "apple")))]
    let backends = wgpu::Backends::VULKAN;
    #[cfg(target_vendor = "apple")]
    let backends = wgpu::Backends::METAL;

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("slicer shared device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::default(),
        experimental_features: wgpu::ExperimentalFeatures::default(),
        trace: wgpu::Trace::Off,
    }))?;

    // Pin wgpu-core's SNATCH_LOCK_TRACE thread-local on the main
    // thread before BackendSelector::select() registers Slint's own
    // thread-local SlintContext (which takes ownership of `device`
    // and `queue` below). TLS destructors fire LIFO at process exit;
    // left to the natural order, SNATCH_LOCK_TRACE gets registered
    // later (during the first render) and so dies first, and the
    // queue's drop reached via SlintContext then panics on
    // AccessError when it tries to take a snatch lock. Polling now
    // exercises SnatchLock::read() so the trace TLS outlives
    // SlintContext. Debug-build only; release stubs the trace out.
    // See https://github.com/gfx-rs/wgpu/issues/8259.
    let _ = device.poll(wgpu::PollType::Poll);

    slint::BackendSelector::new()
        .require_wgpu_29(slint::wgpu_29::WGPUConfiguration::Manual {
            instance,
            adapter,
            device,
            queue,
        })
        .select()?;

    let app = AppState::new()?;

    // OS "Open With…" / file-association support on Linux + Windows:
    // every positional argument that names a supported mesh file gets
    // its own window. Rejected paths (unsupported extension, missing
    // file) are reported on stderr without aborting the launch; the
    // user still sees the app come up. macOS file association doesn't
    // go through argv (Finder posts an `application:openURLs:` event
    // to the running NSApplication); see the open-with plan.
    let accepted = collect_argv_paths();
    if accepted.is_empty() {
        // No paths supplied (or none were supported): preserve the
        // current "launch with one empty window" behaviour.
        app.spawn_main_window()?;
    } else {
        // Decision: N files -> N windows. The path is parked on each
        // window's `PendingLoad` slot and dispatched once
        // `RenderingSetup` publishes its GPU device.
        for path in accepted {
            let window_id = app.spawn_main_window()?;
            app.open_path_in_window(window_id, path);
        }
    }

    slint::run_event_loop()?;
    Ok(())
}

/// Walks `std::env::args()` and returns the positional paths whose
/// extensions are recognised by the mesh loader. Anything else
/// (unsupported extensions, options, missing files) is logged on
/// stderr and skipped. The returned list preserves argv order so the
/// per-window spawn order is deterministic.
fn collect_argv_paths() -> Vec<PathBuf> {
    let mut accepted = Vec::new();
    // Skip argv[0] (the binary path).
    for raw in std::env::args_os().skip(1) {
        let path = PathBuf::from(&raw);
        // Treat anything starting with `-` as an unrecognised option
        // rather than a file. We don't (yet) parse flags, so a leading
        // dash is far more likely to be a mistake than a real filename
        // beginning with `-`.
        if path
            .to_str()
            .is_some_and(|s| s.starts_with('-') && s != "-")
        {
            eprintln!("Ignoring unrecognised option: {}", path.display());
            continue;
        }
        if !is_supported_mesh_extension(&path) {
            eprintln!(
                "Ignoring file with unsupported extension: {}",
                path.display()
            );
            continue;
        }
        if !path.is_file() {
            eprintln!(
                "Ignoring path that is not a regular file: {}",
                path.display()
            );
            continue;
        }
        accepted.push(path);
    }
    accepted
}
