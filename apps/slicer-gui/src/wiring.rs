/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Adapter layer that glues Slint UI callbacks of a `MainWindow` to the
//! `ViewportController` domain object. Kept out of `main.rs` and out of the
//! controller itself so neither gets polluted with `rfd` / file-dialog /
//! worker-thread plumbing.

use std::rc::Rc;

use slint::ComponentHandle;

use crate::MainWindow;
use crate::i18n::ctr;
use crate::viewport_controller::{RESIZE_SETTLE, ViewportController};

// Reject mesh files larger than this before spawning the loader worker, so
// the UI doesn't sit in `loading=true` for hours (or OOM) on an accidental
// huge pick. 2 GiB is already well beyond anything a resin printer slices.
const MAX_MESH_FILE_SIZE_MIB: u64 = 2048;
const MAX_MESH_FILE_SIZE_BYTES: u64 = MAX_MESH_FILE_SIZE_MIB * 1024 * 1024;

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
    ui.on_axis_view_requested(move |code| ctrl.on_axis_view(code));
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

        if let Err(err) = slint::spawn_local(async move {
            let Some(handle) = dialog.pick_file().await else {
                // User cancelled: `ticket` drops here and releases the gate.
                return;
            };
            let path = handle.path().to_path_buf();

            // Pre-check the file size. If it exceeds the ceiling, deposit a
            // synthetic error so the UI finish-handler path surfaces it like
            // any other load failure. `metadata()` failures fall through to
            // the worker, which surfaces a more specific error when it opens
            // the file.
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
                ticket.deposit(path, Err(anyhow::Error::msg(message.to_string())));
                return;
            }

            std::thread::spawn(move || {
                // Catch parser panics and convert them to an `Err` result.
                // `AssertUnwindSafe` is sound because the parser returns an
                // owned `Result<LoadedMesh, anyhow::Error>`, so a mid-parse
                // panic drops any partial state on the stack. A panic
                // elsewhere in this closure (post-parse, pre-deposit) unwinds
                // out of the thread; `ticket` drops on the worker thread, its
                // `Drop` decrements inflight and schedules `loading = false`
                // via the event loop, so the UI still recovers.
                //
                // If `load_mesh_from_path` is ever changed to return a type
                // borrowing from a memory-mapped region, revisit this — the
                // assertion would then hide potential UB.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    sc_mesh_formats::load_mesh_from_path(&path, None)
                }))
                .unwrap_or_else(|payload| {
                    let msg = panic_message(&payload);
                    Err(anyhow::anyhow!("The mesh loader panicked: {msg}"))
                });
                ticket.deposit(path, result);
            });
        }) {
            eprintln!("Failed to spawn file dialog task: {err}");
            // `ticket` was moved into the async block; when `spawn_local`
            // returns `Err`, the block is dropped and the ticket with it,
            // releasing the gate.
        }
    });
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else if let Some(e) =
        payload.downcast_ref::<Box<dyn std::error::Error + Send + Sync + 'static>>()
    {
        e.to_string()
    } else if let Some(e) = payload.downcast_ref::<Box<dyn std::error::Error + Send + 'static>>() {
        e.to_string()
    } else {
        "unknown panic payload".to_string()
    }
}
