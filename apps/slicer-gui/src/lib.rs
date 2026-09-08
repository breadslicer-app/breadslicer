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

mod aa_level;
mod about_window;
mod app_settings;
mod app_state;
pub mod build_volume;
pub mod camera;
mod clipboard;
mod error_dialog;
mod export_print_file;
mod export_progress;
mod export_progress_panel;
mod failure_gate;
mod gizmo;
mod gizmo_geometry;
pub mod graphics_adapter;
mod graphics_status;
mod i18n;
mod language;
mod load_progress;
mod material_form;
mod mesh_health;
mod mesh_loader;
pub mod mesh_render;
mod pending_load;
mod picking;
mod preferences_bus;
mod preferences_window;
mod print_file_preview;
mod print_format;
pub mod profile_catalog;
mod profile_extending;
mod profile_forking;
mod profile_transfer;
mod profile_view;
mod project_profiles;
mod reloaded_slices;
mod repair_run;
mod save_progress;
mod save_progress_panel;
mod scene_document;
mod scene_export;
mod scene_loader;
mod scene_save;
mod selection;
mod slice_cap;
mod slice_panel;
mod slice_projection;
mod slice_result;
mod slice_run;
mod slice_selection;
mod support_gen_run;
#[cfg(test)]
mod tests_mesh_loader;
#[cfg(test)]
mod tests_print_file_preview;
#[cfg(test)]
mod tests_scene_dirty;
#[cfg(test)]
mod tests_scene_loader;
#[cfg(test)]
mod tests_slice_result;
mod text_metrics;
mod viewport;
mod window_icon;
mod window_prefs;
mod wiring;
mod workspace_section;

use std::error::Error;
use std::path::PathBuf;

// Slint's generated code allows the clippy groups by itself, but not
// the rustc lints the workspace denies. Lint attributes are ignored on
// macro invocations, hence the wrapper module.
#[allow(missing_debug_implementations, unreachable_pub, unused_qualifications)]
mod slint_ui {
    slint::include_modules!();
}
pub use slint_ui::*;

use crate::app_state::AppState;
use crate::i18n::ctr;
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
    // Adapter choice goes through `graphics_adapter` rather than
    // `Instance::request_adapter`, which with default options returns
    // whatever the platform enumerated first. See that module for why
    // that matters on machines carrying more than one adapter.
    let graphics_adapter::AdapterSelection {
        adapter,
        device,
        queue,
        skipped,
        recovery,
    } = graphics_adapter::select_adapter_and_device(
        &instance,
        backends,
        &wgpu::DeviceDescriptor {
            label: Some("slicer shared device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::default(),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            trace: wgpu::Trace::Off,
        },
    )?;
    mesh_render::install_device_error_logger(&device);
    let device_loss = mesh_render::DeviceLoss::default();
    {
        let marker = device_loss.clone();
        device.set_device_lost_callback(move |reason, message| {
            let reason = format!("{reason:?}: {message}");
            marker.mark(reason.clone());
            // Fails only when the event loop is already gone; the
            // post-mortem branch after the loop covers that case.
            let _ = slint::invoke_from_event_loop(move || {
                AppState::dispatch_device_lost(reason);
            });
        });
    }
    // Outlives the copy Slint takes, for the post-mortem check below.
    let device_for_post_mortem = device.clone();
    // A skipped adapter means the session is running on hardware the
    // ordering policy ranks below what the machine carries. That can
    // be a real refusal (unsupported limits) or a transient driver
    // failure; either way the reason has to reach a bug report, since
    // the only user-visible symptom is a slower viewport.
    for refusal in &skipped {
        let attempts = refusal
            .errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; on retry: ");
        match &refusal.shortfall {
            Some(shortfall) => eprintln!(
                "Graphics adapter {} cannot satisfy the device profile ({}); refused: {}",
                refusal.name, shortfall, attempts
            ),
            None => eprintln!(
                "Graphics adapter {} refused a device request: {}",
                refusal.name, attempts
            ),
        }
    }
    // A recovered refusal leaves the session on the right adapter but
    // is still the trace of a transient failure; keep it visible.
    if let Some(recovered) = &recovery {
        eprintln!(
            "Graphics adapter {} refused its first device request and accepted the retry: {}",
            adapter.get_info().name,
            recovered.error
        );
    }

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

    if let Some(value) = std::env::var_os("SLINT_BACKEND") {
        eprintln!(
            "SLINT_BACKEND is set to {value:?}, but sc-slicer-gui always uses the winit backend \
             with the Skia renderer; the variable is ignored."
        );
    }
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("skia".into())
        .require_wgpu_29(slint::wgpu_29::WGPUConfiguration::Manual {
            instance,
            adapter,
            device,
            queue,
        })
        .select()?;

    let app = AppState::new(device_loss.clone())?;

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

    let Err(err) = slint::run_event_loop() else {
        return Ok(());
    };
    // A lost device that nothing polled since the loss has not fired
    // its callback yet (wgpu fires it from a poll); a real loss has,
    // and polling a device whose hardware is gone is a wgpu fatal path.
    if !device_loss.is_lost() {
        let _ = device_for_post_mortem.poll(wgpu::PollType::Poll);
    }
    match device_loss.reason() {
        Some(reason) => Err(report_lost_device(&reason).into()),
        None => Err(err.into()),
    }
}

/// Tells the user the graphics device is gone when no window is left
/// to say it in: stderr plus a native message box (on Linux rfd runs
/// `zenity`; without it, stderr only). Returns the error to exit with.
fn report_lost_device(reason: &str) -> String {
    let heading = ctr!("ErrorDialog", "The graphics device was lost").to_string();
    let message = format!(
        "{} {}",
        ctr!(
            "ErrorDialog",
            "The application has to close; restart it once the graphics driver is back."
        ),
        reason
    );
    eprintln!("{heading}: {message}");
    let _ = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title(heading.clone())
        .set_description(message)
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
    format!("{heading}: {reason}")
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
