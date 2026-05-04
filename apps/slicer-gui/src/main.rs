/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

// Prevent console window in addition to Slint window in Windows release builds
// when, e.g., starting the app via file manager. Ignored on other platforms.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod about_window;
mod app_settings;
mod app_state;
mod camera;
mod error_dialog;
mod gizmo;
mod gizmo_geometry;
mod i18n;
mod language;
mod mesh_loader;
mod mesh_render;
mod pending_load;
mod picking;
mod preferences_bus;
mod preferences_window;
mod scene_document;
mod scene_export;
mod scene_loader;
mod selection;
mod viewport;
mod window_prefs;
mod wiring;

use std::error::Error;
use std::path::PathBuf;

use crate::app_state::AppState;
use crate::mesh_loader::is_supported_mesh_extension;

slint::include_modules!();

fn main() -> Result<(), Box<dyn Error>> {
    // Build the wgpu Instance/Adapter/Device/Queue once up front and
    // hand them to Slint via `WGPUConfiguration::Manual` so every
    // top-level Window shares the same Device. Letting Slint pick
    // `Automatic` triggers a full Vulkan ICD re-enumeration per
    // window (4-5 stalls of ~225 ms each on NVIDIA — measured), which
    // makes "New Window" feel like it hangs.
    use slint::wgpu_28::wgpu;
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
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

    slint::BackendSelector::new()
        .require_wgpu_28(slint::wgpu_28::WGPUConfiguration::Manual {
            instance,
            adapter,
            device,
            queue,
        })
        .select()?;

    let app = AppState::new();

    // OS "Open With…" / file-association support on Linux + Windows:
    // every positional argument that names a supported mesh file gets
    // its own window. Rejected paths (unsupported extension, missing
    // file) are reported on stderr without aborting the launch — the
    // user still sees the app come up. macOS file association doesn't
    // go through argv (Finder posts an `application:openURLs:` event
    // to the running NSApplication); see the open-with plan.
    let accepted = collect_argv_paths();
    if accepted.is_empty() {
        // No paths supplied (or none were supported): preserve the
        // current "launch with one empty window" behaviour.
        app.spawn_main_window()?;
    } else {
        // Decision: N files → N windows. The path is parked on each
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
