/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Adapter layer that glues Slint UI callbacks of a `MainWindow` to the
//! `ViewportController` domain object. Kept out of `main.rs` and out of the
//! controller itself so neither gets polluted with `rfd` / file-dialog /
//! worker-thread plumbing.

use std::path::PathBuf;
use std::rc::Rc;

use slint::ComponentHandle;

use crate::MainWindow;
use crate::i18n::ctr;
use crate::mesh_loader;
use crate::scene_loader::{LoadTicket, SceneLoader};
use crate::viewport_controller::{RESIZE_SETTLE, ViewportController};

/// Installs every per-window Slint callback (rendering notifier, input
/// handlers, file open/close, resize debounce). Called once per
/// `MainWindow` from `AppState::spawn_main_window`.
pub(crate) fn wire_main_window(
    ui: &MainWindow,
    ctrl: Rc<ViewportController>,
) -> Result<(), slint::SetRenderingNotifierError> {
    install_rendering_notifier(ui, ctrl.clone())?;
    wire_close_scene(ui, ctrl.clone());
    wire_open_file(ui, ctrl.clone());
    wire_viewport_resized(ui, ctrl.clone());
    wire_viewport_input(ui, ctrl);
    Ok(())
}

fn install_rendering_notifier(
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

fn wire_close_scene(ui: &MainWindow, ctrl: Rc<ViewportController>) {
    ui.on_close_current_scene(move || ctrl.on_close_scene());
}

fn wire_viewport_resized(ui: &MainWindow, ctrl: Rc<ViewportController>) {
    // A single `Timer` captured by the closure below acts as the resize
    // debounce: each `viewport-resized` restarts it, and when no event
    // arrives for `RESIZE_SETTLE`, it fires `on_resize_settled` to snap the
    // renderer allocations back to exact pixel size.
    let settle_timer = slint::Timer::default();
    ui.on_viewport_resized(move |w, h| {
        ctrl.on_viewport_resized(w, h);
        let ctrl_settled = ctrl.clone();
        settle_timer.start(slint::TimerMode::SingleShot, RESIZE_SETTLE, move || {
            ctrl_settled.on_resize_settled()
        });
    });
}

fn wire_viewport_input(ui: &MainWindow, ctrl: Rc<ViewportController>) {
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
    {
        let ctrl = ctrl.clone();
        ui.on_axis_view_requested(move |code| ctrl.on_axis_view(code));
    }
    {
        let ctrl = ctrl.clone();
        ui.on_object_list_selection_changed(move |idx| {
            // Slint hands us an i32; clamp non-negative values into a
            // usize for the controller. A negative idx means "no
            // selection", which the controller models as `None` via
            // `on_clear_selection` rather than via this callback.
            if idx >= 0 {
                ctrl.on_select_by_index(idx as usize);
            } else {
                ctrl.on_clear_selection();
            }
        });
    }
    {
        let ctrl = ctrl.clone();
        ui.on_viewport_pointer_down(move |x, y| ctrl.on_pointer_down(x, y));
    }
    {
        let ctrl = ctrl.clone();
        ui.on_viewport_pointer_move(move |x, y| ctrl.on_pointer_move(x, y));
    }
    {
        let ctrl = ctrl.clone();
        ui.on_viewport_pointer_up(move |x, y| ctrl.on_pointer_up(x, y));
    }
    ui.on_selection_cleared(move || ctrl.on_clear_selection());
}

fn wire_open_file(ui: &MainWindow, ctrl: Rc<ViewportController>) {
    {
        let ctrl = ctrl.clone();
        ui.on_scene_load_finished(move || ctrl.on_scene_load_finished());
    }

    let loader = ctrl.loader();
    ui.on_open_file_requested(move || {
        // The Slint event loop keeps processing callbacks while the native
        // file dialog is open, and `enabled: !root.loading` on the menu item
        // doesn't block that. Gate in Rust via `begin_load` *before* touching
        // the dialog, so `loading` flips immediately (not after pick_file
        // awaits) and a ticket is held for the duration of the load.
        let Some(ticket) = loader.begin_load() else {
            return;
        };
        if let Err(err) = slint::spawn_local(run_open_file_flow(ticket, loader.clone())) {
            eprintln!("Failed to spawn file dialog task: {err}");
            // `ticket` was moved into the future; when `spawn_local`
            // returns `Err`, the future is dropped and the ticket with
            // it, releasing the gate.
        }
    });
}

/// Orchestrates one "Open…" command: show the dialog, precheck the
/// file, hand the load off to the background pipeline. Each step
/// drops the ticket on its bailout path and the gate releases itself.
async fn run_open_file_flow(ticket: LoadTicket, loader: Rc<SceneLoader>) {
    let Some(path) = pick_mesh_file().await else {
        // User cancelled the dialog.
        return;
    };
    if let Err(err) = mesh_loader::check_file_size(&path) {
        ticket.deposit(path, Err(err));
        return;
    }
    let Some((device, _queue)) = loader.gpu_snapshot() else {
        // RenderingSetup hasn't fired (or a teardown cleared the
        // slot between begin_load and now). `begin_load` already
        // gates on this in the steady state, so this only catches a
        // narrow race; bail with a synthetic error so the user sees
        // *something* rather than a stuck `loading=true`.
        ticket.deposit(
            path,
            Err(anyhow::anyhow!("Graphics device is not available yet")),
        );
        return;
    };
    mesh_loader::spawn_load(ticket, path, device);
}

async fn pick_mesh_file() -> Option<PathBuf> {
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
    dialog.pick_file().await.map(|h| h.path().to_path_buf())
}
