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
        // doesn't block that. So gate here in Rust *before* touching the
        // dialog, and flip `loading` immediately (not after pick_file awaits).
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        if ui.get_loading() {
            return;
        }
        ui.set_loading(true);

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

        let ctrl = ctrl.clone();
        let ui_weak_inner = ui_weak.clone();
        let ui_weak_for_err = ui_weak.clone();
        if let Err(err) = slint::spawn_local(async move {
            let Some(handle) = dialog.pick_file().await else {
                // User cancelled the dialog: clear the gate.
                if let Some(ui) = ui_weak_inner.upgrade() {
                    ui.set_loading(false);
                }
                return;
            };
            let path = handle.path().to_path_buf();

            let my_stamp = ctrl.next_load_stamp();
            let slot = ctrl.load_slot();

            let worker_ui_weak = ui_weak_inner.clone();
            std::thread::spawn(move || {
                // Catch a panic in the parser so the UI always gets notified
                // and the loading flag is cleared.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    sc_mesh_formats::load_mesh_from_path(&path, None)
                }))
                .unwrap_or_else(|payload| {
                    let msg = panic_message(&payload);
                    Err(anyhow::anyhow!("The mesh loader panicked: {msg}"))
                });

                *slot.lock().expect("load slot poisoned") = Some((my_stamp, path, result));
                let _ = worker_ui_weak.upgrade_in_event_loop(|ui| {
                    ui.invoke_scene_load_finished();
                });
            });
        }) {
            eprintln!("Failed to spawn file dialog task: {err}");
            if let Some(ui) = ui_weak_for_err.upgrade() {
                ui.set_loading(false);
            }
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
