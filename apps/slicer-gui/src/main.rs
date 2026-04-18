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
mod app_state;
mod camera;
mod error_dialog;
mod i18n;
mod mesh_render;
mod viewport_controller;

use std::error::Error;
use std::rc::Rc;

use i18n::ctr;
use slint::ComponentHandle;

use crate::app_state::AppState;
use crate::viewport_controller::ViewportController;

slint::include_modules!();

// Reject mesh files larger than this before spawning the loader worker, so
// the UI doesn't sit in `loading=true` for hours (or OOM) on an accidental
// huge pick. 2 GiB is already well beyond anything a resin printer slices.
const MAX_MESH_FILE_SIZE_MIB: u64 = 2048;
const MAX_MESH_FILE_SIZE_BYTES: u64 = MAX_MESH_FILE_SIZE_MIB * 1024 * 1024;

fn main() -> Result<(), Box<dyn Error>> {
    slint::BackendSelector::new()
        .require_wgpu_28(slint::wgpu_28::WGPUConfiguration::default())
        .select()?;

    let app = AppState::new();
    app.spawn_main_window()?;
    slint::run_event_loop()?;
    Ok(())
}

pub(crate) fn wire_viewport_resized(ui: &MainWindow, ctrl: Rc<ViewportController>) {
    ui.on_viewport_resized(move |w, h| ctrl.on_viewport_resized(w, h));
}

pub(crate) fn wire_viewport_input(ui: &MainWindow, ctrl: Rc<ViewportController>) {
    {
        let ctrl = ctrl.clone();
        ui.on_rotate_drag(move |dx, dy| ctrl.on_rotate(dx, dy));
    }
    {
        let ctrl = ctrl.clone();
        ui.on_pan_drag(move |dx, dy| ctrl.on_pan(dx, dy));
    }
    {
        let ctrl = ctrl.clone();
        ui.on_zoom_at(move |delta, x, y| ctrl.on_zoom(delta, x, y));
    }
    {
        let ctrl = ctrl.clone();
        ui.on_frame_view_requested(move || ctrl.on_frame_view());
    }
    ui.on_axis_view_requested(move |code| ctrl.on_axis_view(code));
}

pub(crate) fn install_rendering_notifier(
    ui: &MainWindow,
    ctrl: Rc<ViewportController>,
) -> Result<(), slint::SetRenderingNotifierError> {
    ui.window()
        .set_rendering_notifier(move |state, api| match state {
            slint::RenderingState::RenderingSetup => {
                let slint::GraphicsAPI::WGPU28 { device, queue, .. } = api else {
                    return;
                };
                ctrl.handle_rendering_setup(device.clone(), queue.clone());
            }
            slint::RenderingState::BeforeRendering => {
                ctrl.handle_before_rendering();
            }
            slint::RenderingState::RenderingTeardown => {
                ctrl.handle_rendering_teardown();
            }
            _ => {}
        })
}

pub(crate) fn wire_close_scene(ui: &MainWindow, ctrl: Rc<ViewportController>) {
    ui.on_close_current_scene(move || ctrl.on_close_scene());
}

pub(crate) fn wire_open_file(ui: &MainWindow, ctrl: Rc<ViewportController>) {
    {
        let ctrl = ctrl.clone();
        ui.on_scene_load_finished(move || ctrl.on_scene_load_finished());
    }

    let ui_weak = ui.as_weak();
    ui.on_open_file_requested(move || {
        // The Slint event loop keeps processing callbacks while the native
        // file dialog is open, and `enabled: !root.loading` on the menu item
        // doesn't block that. Gate in Rust via `begin_load` *before* touching
        // the dialog, so `loading` flips immediately (not after pick_file
        // awaits) and the in-flight counter is bumped atomically with it.
        if !ctrl.begin_load() {
            return;
        }

        let dialog = rfd::AsyncFileDialog::new()
            .add_filter(
                ctr!("FileDialog", "All supported files").to_string(),
                &["obj", "stl", "stla", "stlb"],
            )
            .add_filter(ctr!("FileDialog", "OBJ files").to_string(), &["obj"])
            .add_filter(
                ctr!("FileDialog", "STL files").to_string(),
                &["stl", "stla", "stlb"],
            )
            .add_filter(ctr!("FileDialog", "All files").to_string(), &["*"]);

        let ctrl_async = ctrl.clone();
        let ui_weak_inner = ui_weak.clone();
        if let Err(err) = slint::spawn_local(async move {
            let Some(handle) = dialog.pick_file().await else {
                // User cancelled the dialog: release the gate.
                ctrl_async.end_load();
                return;
            };
            let path = handle.path().to_path_buf();

            let my_stamp = ctrl_async.next_load_stamp();
            let slot = ctrl_async.load_slot();

            // Pre-check the file size. If it exceeds the ceiling, deposit an
            // error into the slot and trigger the same finish-handler path as
            // a real load failure. `metadata()` failures fall through to the
            // worker, which surfaces a more specific error when it opens the
            // file.
            if let Ok(meta) = std::fs::metadata(&path)
                && meta.len() > MAX_MESH_FILE_SIZE_BYTES
            {
                let size_mib = meta.len() / (1024 * 1024);
                let message = ctr!(
                    "ErrorDialog",
                    "File is {} MiB, which exceeds the {} MiB limit.",
                    size_mib,
                    MAX_MESH_FILE_SIZE_MIB
                );
                let err = anyhow::Error::msg(message.to_string());
                *slot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some((my_stamp, path, Err(err)));
                if let Some(ui) = ui_weak_inner.upgrade() {
                    ui.invoke_scene_load_finished();
                }
                return;
            }

            let worker_ui_weak = ui_weak_inner.clone();
            std::thread::spawn(move || {
                // Catch panics from the parser *and* from the post-parse
                // deposit. A panic between `load_mesh_from_path` returning
                // and the UI invocation below (e.g. allocation failure while
                // constructing the slot tuple) would otherwise kill the
                // worker silently and leave `loading == true` forever.
                //
                // `AssertUnwindSafe` is sound today because the captures
                // (`slot: Arc<Mutex<…>>`, `path: PathBuf`, `my_stamp: u64`)
                // carry no `!UnwindSafe` interior-mutability, and the parser
                // returns an owned `Result<LoadedMesh, anyhow::Error>`, so a
                // mid-parse panic drops any partial state on the stack. If
                // `load_mesh_from_path` is ever changed to return a type
                // borrowing from a memory-mapped region, revisit this — the
                // assertion would then hide potential UB.
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        sc_mesh_formats::load_mesh_from_path(&path, None)
                    }))
                    .unwrap_or_else(|payload| {
                        let msg = panic_message(&payload);
                        Err(anyhow::anyhow!("The mesh loader panicked: {msg}"))
                    });

                    // Tolerate poisoning so a previous worker's panic here
                    // doesn't cascade into this one.
                    *slot
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) =
                        Some((my_stamp, path, result));
                }));

                // Always notify the UI so `loading` is cleared, even if the
                // deposit above panicked. The UI handler tolerates an empty
                // slot and simply drops the load as stale.
                let _ = worker_ui_weak.upgrade_in_event_loop(|ui| {
                    ui.invoke_scene_load_finished();
                });
            });
        }) {
            eprintln!("Failed to spawn file dialog task: {err}");
            ctrl.end_load();
        }
    });
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}
