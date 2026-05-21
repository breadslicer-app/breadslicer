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
use crate::error_dialog::{DialogManager, format_anyhow_chain};
use crate::i18n::ctr;
use crate::mesh_loader;
use crate::scene_document::SceneDocument;
use crate::scene_export;
use crate::scene_loader::LoadTicket;
use crate::viewport::{RESIZE_SETTLE, ViewportController};

mod export_flow_dispatch {
    use std::path::PathBuf;

    use sc_mesh_formats::filetypes::{WriteFormat, infer_mesh_file_format_from_extension};

    /// Classification of the file-save dialog's outcome for the
    /// "Export As..." flow. Pure: derived only from whether the user
    /// picked a path and whether that path's extension maps to a
    /// writeable format. The variants own the picked path (when
    /// present) so the caller can render it back into an error dialog
    /// or pass it straight to the exporter without re-cloning the
    /// `Option`.
    #[derive(Debug, PartialEq, Eq)]
    pub(crate) enum ExportDispatch {
        /// The user dismissed the dialog.
        Cancelled,
        /// The user picked a path, but its extension is either not
        /// inferrable or maps to a format the writer side does not
        /// support (e.g. `.ply`, `.amf`, `.3mf`).
        UnsupportedExtension(PathBuf),
        /// The user picked a path and its extension cleanly maps to a
        /// `WriteFormat` the exporter accepts.
        Ready {
            target: PathBuf,
            format: WriteFormat,
        },
    }

    pub(crate) fn decide_export_dispatch(target: Option<PathBuf>) -> ExportDispatch {
        let Some(target) = target else {
            return ExportDispatch::Cancelled;
        };
        match infer_mesh_file_format_from_extension(&target)
            .ok()
            .and_then(|f| WriteFormat::try_from(f).ok())
        {
            Some(format) => ExportDispatch::Ready { target, format },
            None => ExportDispatch::UnsupportedExtension(target),
        }
    }
}

use export_flow_dispatch::{ExportDispatch, decide_export_dispatch};

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
    wire_export_as(ui, ctrl.clone());
    wire_edit_menu(ui, ctrl.clone());
    wire_viewport_resized(ui, ctrl.clone());
    wire_viewport_input(ui, ctrl);
    Ok(())
}

fn wire_edit_menu(ui: &MainWindow, ctrl: Rc<ViewportController>) {
    {
        let ctrl = ctrl.clone();
        ui.on_undo_requested(move || ctrl.on_undo_requested());
    }
    ui.on_redo_requested(move || ctrl.on_redo_requested());
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
        ui.on_viewport_pointer_down(move |x, y, m| ctrl.on_pointer_down(x, y, m));
    }
    {
        let ctrl = ctrl.clone();
        ui.on_viewport_pointer_move(move |x, y, m| ctrl.on_pointer_move(x, y, m));
    }
    {
        let ctrl = ctrl.clone();
        ui.on_viewport_pointer_up(move |x, y, m| ctrl.on_pointer_up(x, y, m));
    }
    {
        let ctrl = ctrl.clone();
        ui.on_viewport_pointer_leave(move || ctrl.on_pointer_leave());
    }
    {
        let ctrl = ctrl.clone();
        ui.on_gizmo_focus_translate_pressed(move || ctrl.on_gizmo_focus_translate());
    }
    {
        let ctrl = ctrl.clone();
        ui.on_gizmo_focus_rotate_pressed(move || ctrl.on_gizmo_focus_rotate());
    }
    {
        let ctrl = ctrl.clone();
        ui.on_gizmo_focus_scale_pressed(move || ctrl.on_gizmo_focus_scale());
    }
    {
        let ctrl = ctrl.clone();
        ui.on_gizmo_arrow_pressed(move |code| ctrl.on_gizmo_arrow_pressed(code));
    }
    {
        let ctrl = ctrl.clone();
        ui.on_gizmo_arrow_released(move |code| ctrl.on_gizmo_arrow_released(code));
    }
    {
        let ctrl = ctrl.clone();
        ui.on_selection_cycle(move |delta| ctrl.on_selection_cycle(delta));
    }
    ui.on_selection_cleared(move || ctrl.on_clear_selection());
}

fn wire_open_file(ui: &MainWindow, ctrl: Rc<ViewportController>) {
    {
        let ctrl = ctrl.clone();
        ui.on_scene_load_finished(move || ctrl.on_scene_load_finished());
    }

    ui.on_open_file_requested(move || {
        // The Slint event loop keeps processing callbacks while the native
        // file dialog is open, and `enabled: !root.loading` on the menu item
        // doesn't block that. Gate in Rust via `begin_load` *before* touching
        // the dialog, so `loading` flips immediately (not after pick_file
        // awaits) and a ticket is held for the duration of the load.
        let Some(ticket) = ctrl.loader().begin_load() else {
            return;
        };
        if let Err(err) = slint::spawn_local(run_open_file_flow(ticket, ctrl.clone())) {
            eprintln!("Failed to spawn file dialog task: {err}");
            // `ticket` was moved into the future; when `spawn_local`
            // returns `Err`, the future is dropped and the ticket with
            // it, releasing the gate.
        }
    });
}

/// Orchestrates one "Open…" command: show the dialog, hand the picked
/// path to the shared post-pick pipeline. The ticket has already been
/// acquired before the dialog so `loading` flips immediately rather
/// than after the await; if the user cancels, the ticket drops here
/// and the gate releases.
///
/// The build envelope is read *after* the dialog so the placement
/// reflects whatever the user has set by the time they finished
/// picking; a mid-dialog settings edit lands on the next file.
async fn run_open_file_flow(ticket: LoadTicket, ctrl: Rc<ViewportController>) {
    let Some(path) = pick_mesh_file().await else {
        // User cancelled the dialog.
        return;
    };
    mesh_loader::start_load_with_ticket(ticket, path, &ctrl.loader(), ctrl.build_volume());
}

async fn pick_mesh_file() -> Option<PathBuf> {
    let dialog = rfd::AsyncFileDialog::new()
        .add_filter(
            ctr!("FileDialog", "All supported files").to_string(),
            mesh_loader::SUPPORTED_MESH_EXTENSIONS,
        )
        .add_filter(ctr!("FileDialog", "OBJ files").to_string(), &["obj"])
        .add_filter(
            ctr!("FileDialog", "STL files").to_string(),
            &["stl", "stla", "stlb"],
        )
        .add_filter(ctr!("FileDialog", "All files").to_string(), &["*"]);
    dialog.pick_file().await.map(|h| h.path().to_path_buf())
}

fn wire_export_as(ui: &MainWindow, ctrl: Rc<ViewportController>) {
    ui.on_export_as_requested(move || {
        let doc = ctrl.document();
        let dialogs = ctrl.dialogs();
        let default_stem = scene_export::default_export_stem(&doc);
        let default_extension = scene_export::default_export_extension(&doc);
        if let Err(err) = slint::spawn_local(async move {
            let _ = run_export_as_flow(doc, dialogs, default_stem, default_extension).await;
        }) {
            eprintln!("Failed to spawn export dialog task: {err}");
        }
    });
}

/// Orchestrates one "Export As" command: show the file-save dialog,
/// infer the format from the chosen extension, and call into
/// `scene_export`. Returns `true` only when a file was actually
/// written (so a chained "Export & Open" caller knows it's safe to
/// discard the current scene); user cancellation, unknown extensions,
/// and write failures all return `false`. Errors and unknown
/// extensions surface dialogs from this function so callers don't
/// have to.
pub(crate) async fn run_export_as_flow(
    doc: Rc<SceneDocument>,
    dialogs: Rc<DialogManager>,
    default_stem: Option<String>,
    default_extension: &'static str,
) -> bool {
    // rfd 0.17 doesn't expose the filter the user actually picked, so
    // the path's extension is the portable source of truth for the
    // format. `.stl` -> binary, `.stla` -> ASCII, `.obj` -> OBJ. The
    // dialog seeds the active filter's extension into the filename so
    // the user gets the format they implicitly chose.
    let dispatch =
        decide_export_dispatch(pick_export_target(default_stem, default_extension).await);
    match dispatch {
        ExportDispatch::Cancelled => false,
        ExportDispatch::UnsupportedExtension(target) => {
            let err = anyhow::anyhow!(
                "Unable to infer file format from filename. Use one of: \
                 .stl (binary STL), .stla (ASCII STL), or .obj."
            );
            dialogs.show_error(
                ctr!("ErrorDialog", "Failed to export").as_str(),
                Some(ctr!("ErrorDialog", "File: {}", target.display()).to_string()),
                err.to_string(),
                format_anyhow_chain(&err),
            );
            false
        }
        ExportDispatch::Ready { target, format } => {
            match scene_export::export_scene_as(&doc, &target, format) {
                Ok(warnings) => {
                    for w in &warnings {
                        eprintln!("WARNING [{}]: {}", target.display(), w.user_message());
                    }
                    true
                }
                Err(err) => {
                    dialogs.show_error(
                        ctr!("ErrorDialog", "Failed to export").as_str(),
                        Some(ctr!("ErrorDialog", "File: {}", target.display()).to_string()),
                        err.to_string(),
                        format_anyhow_chain(&err),
                    );
                    false
                }
            }
        }
    }
}

async fn pick_export_target(
    default_stem: Option<String>,
    default_extension: &'static str,
) -> Option<PathBuf> {
    // Filters in source-format-first order. Most rfd backends pick
    // the first filter as the active one, which is also the
    // extension we seed the filename with, so the dialog opens on a
    // consistent format pair.
    let stl_bin = (
        ctr!("FileDialog", "STL (binary)").to_string(),
        ["stl"].as_slice(),
    );
    let stl_ascii = (
        ctr!("FileDialog", "STL (ASCII)").to_string(),
        ["stla"].as_slice(),
    );
    let obj = (
        ctr!("FileDialog", "OBJ files").to_string(),
        ["obj"].as_slice(),
    );
    let ordered: [&(String, &[&str]); 3] = match default_extension {
        "obj" => [&obj, &stl_bin, &stl_ascii],
        "stla" => [&stl_ascii, &stl_bin, &obj],
        // "stl" or fallback.
        _ => [&stl_bin, &stl_ascii, &obj],
    };
    let mut dialog = rfd::AsyncFileDialog::new();
    for (name, exts) in ordered {
        dialog = dialog.add_filter(name.clone(), exts);
    }
    if let Some(stem) = default_stem {
        dialog = dialog.set_file_name(format!("{stem}.{default_extension}"));
    }
    dialog.save_file().await.map(|h| h.path().to_path_buf())
}

#[cfg(test)]
mod export_flow_dispatch_tests {
    use std::path::PathBuf;

    use sc_mesh_formats::filetypes::WriteFormat;

    use super::export_flow_dispatch::{ExportDispatch, decide_export_dispatch};

    #[test]
    fn none_target_classifies_as_cancelled() {
        assert_eq!(decide_export_dispatch(None), ExportDispatch::Cancelled);
    }

    #[test]
    fn unknown_extension_classifies_as_unsupported() {
        let path = PathBuf::from("/tmp/out.dat");
        assert_eq!(
            decide_export_dispatch(Some(path.clone())),
            ExportDispatch::UnsupportedExtension(path)
        );
    }

    #[test]
    fn missing_extension_classifies_as_unsupported() {
        let path = PathBuf::from("/tmp/noext");
        assert_eq!(
            decide_export_dispatch(Some(path.clone())),
            ExportDispatch::UnsupportedExtension(path)
        );
    }

    #[test]
    fn readable_but_unwriteable_extensions_classify_as_unsupported() {
        // PLY / AMF / 3MF are inferrable as `MeshFileType` but
        // `WriteFormat::try_from` rejects them, so the dispatch must
        // collapse them into `UnsupportedExtension` rather than letting
        // them escape as a "Ready" variant the exporter can't handle.
        for name in ["model.ply", "model.amf", "model.3mf"] {
            let path = PathBuf::from(name);
            assert_eq!(
                decide_export_dispatch(Some(path.clone())),
                ExportDispatch::UnsupportedExtension(path),
                "{name} should be flagged unsupported"
            );
        }
    }

    #[test]
    fn writeable_extensions_classify_as_ready_with_the_matching_write_format() {
        // `.stl` (no ASCII/binary hint) maps to binary STL by
        // long-standing project convention; see
        // `WriteFormat::try_from(MeshFileType::Stl)`.
        let cases = [
            ("/tmp/out.stl", WriteFormat::StlBin),
            ("/tmp/out.stlb", WriteFormat::StlBin),
            ("/tmp/out.stla", WriteFormat::StlAscii),
            ("/tmp/out.obj", WriteFormat::Obj),
        ];
        for (name, expected_format) in cases {
            let path = PathBuf::from(name);
            assert_eq!(
                decide_export_dispatch(Some(path.clone())),
                ExportDispatch::Ready {
                    target: path,
                    format: expected_format,
                },
                "{name} should be Ready({expected_format:?})"
            );
        }
    }
}
