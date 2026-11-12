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
mod arrange_run;
mod autosave;
mod autosave_store;
mod clipboard;
mod cut_gizmo;
mod cut_run;
mod default_envelope;
mod df_support_run;
mod diagnostics;
mod error_dialog;
mod export_print_file;
mod export_progress;
mod export_progress_panel;
mod failure_gate;
mod frame_rate;
mod graphics_status;
mod hollow_persist;
mod hollow_run;
mod i18n;
mod import_notes;
mod imported_supports;
mod language;
mod launch_files;
mod layer_keys;
mod load_progress;
#[cfg(target_os = "macos")]
mod macos_open_documents;
mod material_form;
mod mesh_check;
mod mesh_health;
mod mesh_loader;
mod model_import;
mod orient_run;
mod overhang_tips;
mod pending_load;
mod preferences_bus;
mod preferences_filters;
mod preferences_window;
mod presupported_import;
mod print_format;
mod print_selection;
pub mod profile_catalog;
mod profile_extending;
mod profile_forking;
mod profile_transfer;
mod profile_view;
mod project_profiles;
mod project_versions;
mod raft_clash;
mod recent_files;
mod reloaded_slices;
mod repair_marks;
mod repair_report;
mod repair_run;
mod review;
mod review_marks;
mod review_panel;
mod save_progress;
mod save_progress_panel;
mod scene_document;
mod scene_export;
mod scene_loader;
mod scene_save;
mod selection;
mod single_instance;
mod slice_panel;
mod slice_projection;
mod slice_result;
mod slice_run;
mod slice_selection;
mod split_bodies;
mod start_dialog;
mod startup_token;
mod support_follow;
mod support_gen_run;
mod support_hints;
mod support_lines;
mod support_persist;
mod surface_brush;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests_bread_roundtrip;
#[cfg(test)]
mod tests_import_timing;
#[cfg(test)]
mod tests_layer_view;
#[cfg(test)]
mod tests_mesh_loader;
#[cfg(test)]
mod tests_message_cards;
#[cfg(test)]
mod tests_model_import;
#[cfg(test)]
mod tests_paint_timing;
#[cfg(test)]
mod tests_paste_prep;
#[cfg(test)]
mod tests_paste_timing;
#[cfg(test)]
mod tests_pick_timing;
#[cfg(test)]
mod tests_presupported_import;
#[cfg(test)]
mod tests_scene_dirty;
#[cfg(test)]
mod tests_scene_loader;
#[cfg(test)]
mod tests_slice_result;
#[cfg(test)]
mod tests_start_dialog;
#[cfg(test)]
mod tests_surface_paint;
#[cfg(test)]
mod tests_tab_translations;
#[cfg(test)]
mod tests_ui_inventory;
mod ui_language;
mod viewport;
mod window_chrome;
mod window_icon;
mod window_placement;
mod window_prefs;
mod wiring;
mod workspace_section;

use std::error::Error;

// The interface Slint generates, compiled in a crate of its own (#361).
use sc_slicer_gui_ui as slint_ui;
pub use sc_slicer_gui_ui::*;

// The 3D view and the base the GUI crates share, in crates of their own
// (#361). The integration tests reach the view's modules through this crate.
use sc_slicer_gui_base::{breadcrumbs, startup_timing};
pub use sc_slicer_gui_render::{build_volume, camera, graphics_adapter, mesh_render};
use sc_slicer_gui_render::{
    front_label, gizmo, picking, print_file_preview, slice_cap, under_plate,
};

use crate::app_state::AppState;
use crate::i18n::ctr;

/// A release build has no console: a panic would close the window without a
/// trace. Every panic is appended to `logs/crash.log` in the app's data
/// folder (message, place, backtrace) before the default handling runs.
fn install_crash_log() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        use std::io::Write;
        if let Some(dir) = sc_app_paths::project_dirs().map(|d| d.data_local_dir().join("logs"))
            && std::fs::create_dir_all(&dir).is_ok()
            && let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("crash.log"))
        {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let _ = writeln!(
                file,
                "=== panic at unix {stamp} (v{}): {info}
{}",
                env!("CARGO_PKG_VERSION"),
                std::backtrace::Backtrace::force_capture()
            );
            // What the app was doing just before, kept in memory even with the
            // diagnostic log off.
            let _ = writeln!(file, "--- last events");
            for line in breadcrumbs::recent() {
                let _ = writeln!(file, "{line}");
            }
        }
        previous(info);
    }));
}

/// Starts the GUI for the binary wrapper: builds the shared wgpu stack, wires
/// it into Slint, opens the files named on the command line (or 1 empty
/// window if none were) and runs the event loop. A launch whose files the
/// running app takes exits at once (#418).
pub fn run() -> Result<(), Box<dyn Error>> {
    // First statement of the process: taking the token clears it from the
    // environment, which is only safe before wgpu and the drivers below it
    // start threads.
    let startup_token = startup_token::take_from_env();
    startup_timing::mark("main");
    // The files named on the command line (OS "Open With...", file
    // associations, other tools) on Linux and Windows; macOS carries them in
    // an Apple Event, handled by `macos_open_documents`. The app already
    // running takes them when it can, before this launch starts anything
    // (#418).
    let launch_paths = launch_files::from_env();
    let single_instance::Start::Running {
        listening,
        not_taken,
    } = single_instance::start(&launch_paths, app_state::receive_handoff)
    else {
        return Ok(());
    };
    startup_timing::mark("single instance");
    // The formats ported from UVtools write through `sc-sla-formats` once
    // installed. No threads start here.
    sc_sla_formats_uvtools::install();
    // Before anything reads or writes the per-user folders (logs, settings).
    let migrated = sc_app_paths::migrate_legacy_user_dirs();
    install_crash_log();
    breadcrumbs::start(env!("CARGO_PKG_VERSION"), diagnostics::commit());
    for dir in &migrated {
        breadcrumbs::note(&format!("migrated user data into {}", dir.display()));
    }
    if not_taken {
        breadcrumbs::note("launch: the running app did not take the files in time");
    }
    if listening {
        breadcrumbs::note("launch: takes the files of later launches");
    }
    startup_timing::mark("user folders and event trail");
    // Read while the graphics device starts, which takes longer (#359).
    let catalog = profile_catalog::CatalogLoad::start(
        sc_print_profiles::ProfileLibraries::platform_default(),
    );

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
    startup_timing::mark("graphics instance");
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
    startup_timing::mark("graphics adapter and device");
    diagnostics::system::record_graphics(&adapter.get_info());
    mesh_render::install_device_error_logger(&device);
    // The first window's viewport pipelines compile while the window is made
    // (#359).
    mesh_render::prepare_pipelines(&device, &queue);
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
    // slower viewport. A release build has no console, so the events carry it.
    for refusal in &skipped {
        let attempts = refusal
            .errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; on retry: ");
        let report = match &refusal.shortfall {
            Some(shortfall) => format!(
                "Graphics adapter {} cannot satisfy the device profile ({}); refused: {}",
                refusal.name, shortfall, attempts
            ),
            None => format!(
                "Graphics adapter {} refused a device request: {}",
                refusal.name, attempts
            ),
        };
        eprintln!("{report}");
        breadcrumbs::note(&format!("gpu: {report}"));
    }
    // A recovered refusal leaves the session on the right adapter but is still
    // the trace of a transient failure; keep it visible.
    if let Some(recovered) = &recovery {
        let report = format!(
            "Graphics adapter {} refused its first device request and accepted the retry: {}",
            adapter.get_info().name,
            recovered.error
        );
        eprintln!("{report}");
        breadcrumbs::note(&format!("gpu: {report}"));
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
    startup_timing::mark("window backend");

    // Winit calls `set_app_id` only when Slint hands it a name, and the Wayland
    // path has no argv[0] fallback like X11 does. With no id the shell cannot
    // pair the window with the desktop entry, so the launch it started holds
    // its busy cursor until it times out.
    slint::set_xdg_app_id(sc_app_paths::APP_ID)?;

    let app = AppState::new(device_loss.clone(), catalog)?;

    // Every model in one window, each project in its own (#418); a file that
    // cannot be opened is reported there, and the others still open. With
    // no file, 1 empty window.
    let empty_window = app.open_launch_files(launch_paths)?;

    // Scenes an earlier run left unsaved (#18), then the start dialog (#381)
    // when the launch opened no file.
    app.greet(empty_window);
    startup_timing::mark("recovery scan");

    #[cfg(target_os = "macos")]
    install_finder_open_documents(&app, empty_window);
    #[cfg(not(target_os = "macos"))]
    let _ = empty_window;

    breadcrumbs::note("app: running");
    let looped = slint::run_event_loop();
    // However the loop ended, a scene left unsaved keeps its autosave.
    app.finish_autosave();
    let Err(err) = looped else {
        breadcrumbs::note("app: closed normally");
        return Ok(());
    };
    breadcrumbs::note(&format!("app: event loop failed: {err}"));
    // A lost device that nothing polled since the loss has not fired its
    // callback yet (wgpu fires it from a poll); a real loss has, and polling a
    // device whose hardware is gone is a wgpu fatal path.
    if !device_loss.is_lost() {
        let _ = device_for_post_mortem.poll(wgpu::PollType::Poll);
    }
    match device_loss.reason() {
        Some(reason) => {
            breadcrumbs::note(&format!("gpu: device lost: {reason}"));
            Err(report_lost_device(&reason).into())
        }
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
