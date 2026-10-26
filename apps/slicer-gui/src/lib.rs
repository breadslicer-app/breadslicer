/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

//! Library root for the slicer GUI crate. The binary entry point in `main.rs`
//! is a thin wrapper that calls into `run`.
//!
//! Modules touched by external integration tests (`apps/slicer-gui/tests/...`)
//! are re-published as `pub`; the rest stay crate-private.

mod aa_level;
mod about_window;
mod app_settings;
mod app_state;
pub mod build_volume;
pub mod camera;
mod clipboard;
mod default_envelope;
mod df_support_run;
mod error_dialog;
mod export_print_file;
mod export_progress;
mod export_progress_panel;
mod failure_gate;
mod front_label;
mod gizmo;
mod gizmo_geometry;
pub mod graphics_adapter;
mod graphics_status;
mod i18n;
mod language;
mod load_progress;
#[cfg(target_os = "macos")]
mod macos_open_documents;
mod material_form;
mod mesh_health;
mod mesh_loader;
pub mod mesh_render;
mod pending_load;
mod picking;
mod preferences_bus;
mod preferences_filters;
mod preferences_window;
mod print_file_preview;
mod print_format;
mod print_selection;
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
mod startup_token;
mod support_gen_run;
#[cfg(test)]
mod test_support;
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
mod ui_language;
mod viewport;
mod window_icon;
mod window_placement;
mod window_prefs;
mod wiring;
mod workspace_section;

use std::error::Error;
use std::path::PathBuf;

// Slint's generated code allows the clippy groups by itself, but not the rustc
// lints the workspace denies. Lint attributes are ignored on macro invocations,
// hence the wrapper module.
#[allow(missing_debug_implementations, unreachable_pub, unused_qualifications)]
mod slint_ui;
pub use slint_ui::*;

use crate::app_state::AppState;
use crate::i18n::ctr;
use crate::mesh_loader::is_supported_mesh_extension;

/// Starts the GUI for the binary wrapper: builds the shared wgpu stack, wires
/// it into Slint, spawns 1 window per accepted CLI path (or 1 empty window if
/// none were supplied) and runs the event loop.
pub fn run() -> Result<(), Box<dyn Error>> {
    // First statement of the process: taking the token clears it from the
    // environment, which is only safe before wgpu and the drivers below it
    // start threads.
    let startup_token = startup_token::take_from_env();

    // Build the wgpu Instance/Adapter/Device/Queue once up front and hand them
    // to Slint via `WGPUConfiguration::Manual` so every top-level Window shares
    // the same Device. Letting Slint pick `Automatic` triggers a full Vulkan
    // ICD re-enumeration per window (4-5 stalls of ~225 ms each on NVIDIA,
    // measured), which makes "New Window" feel like it hangs.
    use slint::wgpu_30::wgpu;

    // Slint's Skia renderer only wires 1 wgpu backend per platform: Dx12 on
    // Windows, Vulkan on Linux, Metal on macOS. An instance that enumerates
    // every backend can hand Slint an adapter it rejects; on Windows
    // `request_adapter` may return a Vulkan adapter, which the Skia surface
    // refuses with "Unsupported WGPU backend for use with Skia: vulkan".
    // Restrict the instance to the single backend the renderer accepts here.
    // The cfg predicates mirror i-slint-renderer-skia so both stay in lockstep.
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
    // Adapter choice goes through `graphics_adapter`: with default options,
    // `Instance::request_adapter` returns whatever the platform enumerated
    // first. See that module for why that matters on machines with 2 or more
    // adapters.
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
            // Fails only when the event loop is already gone; the post-mortem
            // branch after the loop covers that case.
            let _ = slint::invoke_from_event_loop(move || {
                AppState::dispatch_device_lost(reason);
            });
        });
    }
    // Outlives the copy Slint takes, for the post-mortem check below.
    let device_for_post_mortem = device.clone();
    // A skipped adapter means the session is running on hardware the ordering
    // policy ranks below what the machine carries. That can be a real refusal
    // (unsupported limits) or a transient driver failure; either way the reason
    // has to reach a bug report, since the only user-visible symptom is a
    // slower viewport.
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
    // A recovered refusal leaves the session on the right adapter but is still
    // the trace of a transient failure; keep it visible.
    if let Some(recovered) = &recovery {
        eprintln!(
            "Graphics adapter {} refused its first device request and accepted the retry: {}",
            adapter.get_info().name,
            recovered.error
        );
    }

    // Pin wgpu-core's SNATCH_LOCK_TRACE thread-local on the main thread before
    // BackendSelector::select() registers Slint's own thread-local SlintContext
    // (which takes ownership of `device` and `queue` below). TLS destructors
    // fire LIFO at process exit; left to the natural order, SNATCH_LOCK_TRACE
    // gets registered later (during the first render) and so dies first, and
    // the queue's drop reached via SlintContext then panics on AccessError when
    // it tries to take a snatch lock. Polling now exercises SnatchLock::read()
    // so the trace TLS outlives SlintContext. Debug-build only; release stubs
    // the trace out. See https://github.com/gfx-rs/wgpu/issues/8259.
    let _ = device.poll(wgpu::PollType::Poll);

    if let Some(value) = std::env::var_os("SLINT_BACKEND") {
        eprintln!(
            "SLINT_BACKEND is set to {value:?}, but sc-slicer-gui always uses the winit backend \
             with the Skia renderer; the variable is ignored."
        );
    }
    let selector = slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("skia".into())
        .require_wgpu_30(slint::wgpu_30::WGPUConfiguration::Manual {
            instance,
            adapter,
            device,
            queue,
        });
    startup_token::attach(selector, startup_token).select()?;

    // Winit calls `set_app_id` only when Slint hands it a name, and the Wayland
    // path has no argv[0] fallback like X11 does. With no id the shell cannot
    // pair the window with the desktop entry, so the launch it started holds
    // its busy cursor until it times out.
    slint::set_xdg_app_id(sc_app_paths::APP_ID)?;

    let app = AppState::new(device_loss.clone())?;

    // OS "Open With..." / file-association support on Linux + Windows: every
    // positional argument that names a supported mesh file gets its own window.
    // Rejected paths (unsupported extension, missing file) are reported on
    // stderr without aborting the launch; the user still sees the app come up.
    // macOS carries the files in an Apple Event, handled by
    // `macos_open_documents`.
    let accepted = collect_argv_paths();
    let empty_window = if accepted.is_empty() {
        // No paths supplied (or none were supported): the launch shows 1 empty
        // window.
        Some(app.spawn_main_window()?)
    } else {
        // 1 window per file. The path is parked on each window's `PendingLoad`
        // slot and dispatched once `RenderingSetup` publishes its GPU device.
        for path in accepted {
            let window_id = app.spawn_main_window()?;
            app.open_path_in_window(window_id, path);
        }
        None
    };

    #[cfg(target_os = "macos")]
    install_finder_open_documents(&app, empty_window);
    #[cfg(not(target_os = "macos"))]
    let _ = empty_window;

    let Err(err) = slint::run_event_loop() else {
        return Ok(());
    };
    // A lost device that nothing polled since the loss has not fired its
    // callback yet (wgpu fires it from a poll); a real loss has, and polling a
    // device whose hardware is gone is a wgpu fatal path.
    if !device_loss.is_lost() {
        let _ = device_for_post_mortem.poll(wgpu::PollType::Poll);
    }
    match device_loss.reason() {
        Some(reason) => Err(report_lost_device(&reason).into()),
        None => Err(err.into()),
    }
}

/// Tells the user the graphics device is gone when no window is left to say it
/// in: stderr plus a native message box (on Linux rfd runs `zenity`; without
/// it, stderr only). Returns the error to exit with.
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

/// Routes the files the Finder opens into windows, 1 window each.
///
/// `empty_window` is the blank window the launch put up when no path came in on
/// `argv`. The first file claims it, so opening a document from the Finder does
/// not leave a blank window sitting beside it. There is no such window once the
/// application is running, and later files get their own.
#[cfg(target_os = "macos")]
fn install_finder_open_documents(app: &std::rc::Rc<AppState>, empty_window: Option<u64>) {
    let app = std::rc::Rc::downgrade(app);
    let empty_window = std::cell::Cell::new(empty_window);
    macos_open_documents::install(move |paths| {
        let Some(app) = app.upgrade() else {
            return;
        };
        for path in paths {
            let window_id = match empty_window.take() {
                Some(window_id) => window_id,
                None => match app.spawn_main_window() {
                    Ok(window_id) => window_id,
                    Err(err) => {
                        eprintln!("Could not open a window for {}: {err}", path.display());
                        return;
                    }
                },
            };
            app.open_path_in_window(window_id, path);
        }
    });
}

/// Walks `std::env::args()` and returns the positional paths whose extensions
/// are recognised by the mesh loader. Anything else (unsupported extensions,
/// options, missing files) is logged on stderr and skipped. The returned list
/// preserves argv order so the per-window spawn order is deterministic.
fn collect_argv_paths() -> Vec<PathBuf> {
    let mut accepted = Vec::new();
    // Skip argv[0] (the binary path).
    for raw in std::env::args_os().skip(1) {
        let path = PathBuf::from(&raw);
        // Anything starting with `-` is an unrecognised option. The app parses
        // no flags, so a leading dash is far more likely a mistake than a real
        // filename beginning with `-`.
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
