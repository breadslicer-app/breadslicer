/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Adapter layer that glues Slint UI callbacks of a `MainWindow` to the
//! `ViewportController` domain object. Kept out of `main.rs` and out of the
//! controller itself so neither gets polluted with `rfd` / file-dialog /
//! worker-thread plumbing.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use futures_channel::oneshot;
use sc_print_primitives::{MaterialId, PrinterId};
use slint::ComponentHandle;

use crate::MainWindow;
use crate::error_dialog::{DialogManager, format_anyhow_chain};
use crate::i18n::ctr;
use crate::mesh_loader::{self, LoadDispatch};
use crate::preferences_bus::{PreferencesBus, PreferencesChange};
use crate::save_progress_panel::SaveProgressPanel;
use crate::scene_document::SceneDocument;
use crate::scene_export;
use crate::scene_loader::LoadTicket;
use crate::scene_save::{self, SaveDispatch, SaveTargetDispatch};
use crate::viewport::{RESIZE_SETTLE, ScreenDelta, ScreenPoint, ViewportController};

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
/// handlers, file open/close/save, resize debounce). Called once per
/// `MainWindow` from `AppState::spawn_main_window`. `prefs` feeds the
/// save flow the app-wide default profiles at save time.
pub(crate) fn wire_main_window(
    ui: &MainWindow,
    ctrl: Rc<ViewportController>,
    prefs: Rc<PreferencesBus>,
) -> Result<(), slint::SetRenderingNotifierError> {
    install_rendering_notifier(ui, ctrl.clone())?;
    wire_close_scene(ui, ctrl.clone());
    wire_open_file(ui, ctrl.clone(), prefs.clone());
    wire_save(ui, ctrl.clone(), prefs.clone());
    wire_save_as(ui, ctrl.clone(), prefs);
    wire_export_as(ui, ctrl.clone());
    wire_edit_menu(ui, ctrl.clone());
    wire_workspace_section(ui, ctrl.clone());
    wire_selected_layer(ui, ctrl.clone());
    wire_viewport_resized(ui, ctrl.clone());
    wire_viewport_input(ui, ctrl);
    Ok(())
}

fn wire_workspace_section(ui: &MainWindow, ctrl: Rc<ViewportController>) {
    ui.on_section_change_requested(move |section| ctrl.set_section(section));
}

fn wire_selected_layer(ui: &MainWindow, ctrl: Rc<ViewportController>) {
    ui.on_selected_layer_changed(move || ctrl.on_selected_layer_changed());
}

fn wire_edit_menu(ui: &MainWindow, ctrl: Rc<ViewportController>) {
    {
        let ctrl = ctrl.clone();
        ui.on_undo_requested(move || ctrl.on_undo_requested());
    }
    {
        let ctrl = ctrl.clone();
        ui.on_redo_requested(move || ctrl.on_redo_requested());
    }
    ui.on_delete_requested(move || ctrl.on_delete_requested());
}

fn install_rendering_notifier(
    ui: &MainWindow,
    ctrl: Rc<ViewportController>,
) -> Result<(), slint::SetRenderingNotifierError> {
    ui.window()
        .set_rendering_notifier(move |state, api| match state {
            slint::RenderingState::RenderingSetup => match api {
                slint::GraphicsAPI::WGPU29 { device, queue, .. } => {
                    ctrl.handle_rendering_setup(device.clone(), queue.clone());
                }
                // Slint hands over whatever the surface it built runs
                // on, and the viewport is drawn with wgpu, so anything
                // else leaves this window without a device for good.
                // Recording which API arrived turns that into something
                // the user can be told and a report can name, rather
                // than a viewport that silently stays empty.
                other => ctrl.handle_unsupported_graphics_api(format!("{other:?}")),
            },
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
        ui.on_rotate_drag(move |dx, dy| ctrl.on_rotate(ScreenDelta::new(dx, dy)));
    }
    {
        let ctrl = ctrl.clone();
        ui.on_pan_drag(move |dx, dy| ctrl.on_pan(ScreenDelta::new(dx, dy)));
    }
    {
        let ctrl = ctrl.clone();
        ui.on_zoom_at(move |delta, x, y| ctrl.on_zoom(delta, ScreenPoint::new(x, y)));
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
        ui.on_viewport_pointer_down(move |x, y, m| ctrl.on_pointer_down(ScreenPoint::new(x, y), m));
    }
    {
        let ctrl = ctrl.clone();
        ui.on_viewport_pointer_move(move |x, y, m| ctrl.on_pointer_move(ScreenPoint::new(x, y), m));
    }
    {
        let ctrl = ctrl.clone();
        ui.on_viewport_pointer_up(move |x, y, m| ctrl.on_pointer_up(ScreenPoint::new(x, y), m));
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

fn wire_open_file(ui: &MainWindow, ctrl: Rc<ViewportController>, prefs: Rc<PreferencesBus>) {
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
        if let Err(err) =
            slint::spawn_local(run_open_file_flow(ticket, ctrl.clone(), prefs.clone()))
        {
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
/// The dialog starts in the directory of the last opened file (falling
/// back to the home directory), and that directory is remembered after
/// a successful pick. The build envelope is read *after* the dialog so
/// the placement reflects whatever the user has set by the time they
/// finished picking; a mid-dialog settings edit lands on the next file.
async fn run_open_file_flow(
    ticket: LoadTicket,
    ctrl: Rc<ViewportController>,
    prefs: Rc<PreferencesBus>,
) {
    let start_dir =
        resolve_open_dialog_dir(prefs.snapshot().last_open_dir, sc_app_paths::home_dir());
    let Some(path) = pick_mesh_file(start_dir).await else {
        // User cancelled the dialog.
        return;
    };
    remember_open_directory(&prefs, &path);
    let loader = ctrl.loader();
    match mesh_loader::decide_load_dispatch(loader.graphics_unavailable()) {
        LoadDispatch::Start => {
            mesh_loader::start_load_with_ticket(ticket, path, &loader, ctrl.build_volume());
        }
        LoadDispatch::Queue => {
            // The viewport has not started yet, which on a cold window
            // is a matter of the first paint rather than a failure.
            // Park the pick where `handle_rendering_setup` drains it,
            // and let the ticket drop so the loader gate reopens; the
            // drain takes a fresh one when it dispatches.
            ctrl.pending_load().queue(path);
            drop(ticket);
        }
        LoadDispatch::Report(reason) => {
            // Depositing rather than dropping keeps the failure on the
            // one path that already reports load errors, so the user
            // gets the usual dialog naming the file they picked.
            ticket.deposit(path, Err(anyhow::Error::msg(reason.message())));
        }
    }
}

/// Picks the directory the open dialog should start in: the last opened
/// file's directory when it still exists, otherwise the home directory,
/// otherwise the platform default (no explicit directory).
fn resolve_open_dialog_dir(
    last_open_dir: Option<PathBuf>,
    home_dir: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(dir) = last_open_dir
        && dir.is_dir()
    {
        return Some(dir);
    }
    home_dir.filter(|home| home.is_dir())
}

/// Records the directory an opened file lived in so the next open dialog
/// starts there. No-op when the path has no parent or the directory is
/// already the remembered one (avoids a redundant settings write).
fn remember_open_directory(prefs: &Rc<PreferencesBus>, opened_file: &Path) {
    let Some(dir) = opened_file.parent().map(Path::to_path_buf) else {
        return;
    };
    if prefs.snapshot().last_open_dir.as_deref() == Some(dir.as_path()) {
        return;
    }
    prefs.update(move |snap| {
        snap.last_open_dir = Some(dir);
        PreferencesChange::LastOpenDir
    });
}

async fn pick_mesh_file(start_dir: Option<PathBuf>) -> Option<PathBuf> {
    let mut dialog = rfd::AsyncFileDialog::new()
        .add_filter(
            ctr!("FileDialog", "All supported files").to_string(),
            mesh_loader::SUPPORTED_MESH_EXTENSIONS,
        )
        .add_filter(ctr!("FileDialog", "3MF files").to_string(), &["3mf"])
        .add_filter(ctr!("FileDialog", "OBJ files").to_string(), &["obj"])
        .add_filter(
            ctr!("FileDialog", "STL files").to_string(),
            &["stl", "stla", "stlb"],
        )
        .add_filter(ctr!("FileDialog", "All files").to_string(), &["*"]);
    if let Some(dir) = start_dir {
        dialog = dialog.set_directory(dir);
    }
    dialog.pick_file().await.map(|h| h.path().to_path_buf())
}

fn wire_save(ui: &MainWindow, ctrl: Rc<ViewportController>, prefs: Rc<PreferencesBus>) {
    ui.on_save_requested(move || {
        let ctrl = ctrl.clone();
        let snapshot = prefs.snapshot();
        if let Err(err) = slint::spawn_local(async move {
            let saved = run_save_flow(
                ctrl.document(),
                ctrl.dialogs(),
                ctrl.save_progress(),
                snapshot.default_printer,
                snapshot.default_resin,
            )
            .await;
            if saved {
                // A first "Save" falls back to "Save As", which
                // relocates the document; refresh the title to the new
                // file name.
                ctrl.refresh_document_title();
            }
        }) {
            eprintln!("Failed to spawn save task: {err}");
        }
    });
}

/// Orchestrates one "Save" command: overwrite the document's project
/// file, fall back to the Save As flow when there is none, and treat
/// a clean project as already saved without touching the disk.
/// Returns `true` only when the document's current state is on disk
/// by the time the flow ends (so a chained "Save & Open" caller knows
/// it's safe to discard the current scene); cancellation of the Save
/// As dialog and write failures return `false` and surface their own
/// dialogs.
pub(crate) async fn run_save_flow(
    doc: Rc<SceneDocument>,
    dialogs: Rc<DialogManager>,
    save_progress: Rc<SaveProgressPanel>,
    default_printer: Option<PrinterId>,
    default_resin: Option<MaterialId>,
) -> bool {
    match scene_save::decide_save_dispatch(&doc) {
        SaveDispatch::NoScene => false,
        SaveDispatch::AlreadySaved => true,
        SaveDispatch::OverwriteProject(target) => {
            save_to_target(
                &doc,
                &dialogs,
                &save_progress,
                &target,
                default_printer,
                default_resin,
            )
            .await
        }
        SaveDispatch::NeedsSaveAs => {
            let default_stem = scene_export::default_export_stem(&doc);
            run_save_as_flow(
                doc,
                dialogs,
                save_progress,
                default_stem,
                default_printer,
                default_resin,
            )
            .await
        }
    }
}

/// Writes a project to `target` off the UI thread, driving the
/// save-progress panel. Snapshots the scene and payload on the UI
/// thread, spawns a worker for the serialize + compress + disk write,
/// and awaits its result over a oneshot delivered back through the event
/// loop, so the event loop stays responsive throughout. On success the
/// document is recorded on the UI thread and `true` is returned; a write
/// failure surfaces the error dialog and returns `false`.
async fn save_to_target(
    doc: &Rc<SceneDocument>,
    dialogs: &Rc<DialogManager>,
    save_progress: &Rc<SaveProgressPanel>,
    target: &Path,
    default_printer: Option<PrinterId>,
    default_resin: Option<MaterialId>,
) -> bool {
    let Some((bundle, payload)) = scene_save::prepare_save_for_target(
        doc,
        target.to_path_buf(),
        default_printer,
        default_resin,
    ) else {
        // The menu gates this, but a stale callback could still race a
        // close: no scene means nothing to write.
        return false;
    };

    save_progress.begin();
    let sink = save_progress.sink();
    let (tx, rx) = oneshot::channel();
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            scene_save::write_bundle(&bundle, |phase, done, total| sink.set(phase, done, total))
        }))
        .unwrap_or_else(|payload| Err(anyhow::anyhow!(mesh_loader::panic_message(&payload))));
        // Deliver on the UI thread so the awaiting task wakes there. If
        // the event loop is gone (shutdown), the sender drops and the
        // receiver resolves to a cancellation.
        let _ = slint::invoke_from_event_loop(move || {
            let _ = tx.send(result);
        });
    });

    let outcome = rx.await;
    save_progress.end();
    match outcome {
        Ok(Ok(())) => {
            scene_save::record_save(doc, payload, target);
            true
        }
        Ok(Err(err)) => {
            show_save_error(dialogs, target, &err);
            false
        }
        // The sender dropped without sending (event loop gone at
        // shutdown): treat as not saved.
        Err(_canceled) => false,
    }
}

fn wire_save_as(ui: &MainWindow, ctrl: Rc<ViewportController>, prefs: Rc<PreferencesBus>) {
    ui.on_save_as_requested(move || {
        let snapshot = prefs.snapshot();
        spawn_save_as_flow(
            ctrl.clone(),
            snapshot.default_printer,
            snapshot.default_resin,
        );
    });
}

fn spawn_save_as_flow(
    ctrl: Rc<ViewportController>,
    default_printer: Option<PrinterId>,
    default_resin: Option<MaterialId>,
) {
    let default_stem = scene_export::default_export_stem(&ctrl.document());
    if let Err(err) = slint::spawn_local(async move {
        let saved = run_save_as_flow(
            ctrl.document(),
            ctrl.dialogs(),
            ctrl.save_progress(),
            default_stem,
            default_printer,
            default_resin,
        )
        .await;
        if saved {
            // "Save As" relocates the document; refresh the title to
            // the new file name.
            ctrl.refresh_document_title();
        }
    }) {
        eprintln!("Failed to spawn save dialog task: {err}");
    }
}

/// Orchestrates one "Save As" command: show the file-save dialog
/// (seeded with the source stem and the `.3mf` extension), validate
/// the picked name, and write the project. Returns `true` only when
/// a file was actually written; cancellation, wrong extensions, and
/// write failures all return `false` and surface their own dialogs.
async fn run_save_as_flow(
    doc: Rc<SceneDocument>,
    dialogs: Rc<DialogManager>,
    save_progress: Rc<SaveProgressPanel>,
    default_stem: Option<String>,
    default_printer: Option<PrinterId>,
    default_resin: Option<MaterialId>,
) -> bool {
    let dispatch = scene_save::decide_save_target(pick_save_target(default_stem).await);
    match dispatch {
        SaveTargetDispatch::Cancelled => false,
        SaveTargetDispatch::WrongExtension(target) => {
            let err = anyhow::anyhow!("Project files are saved as .3mf packages.");
            show_save_error(&dialogs, &target, &err);
            false
        }
        SaveTargetDispatch::Ready(target) => {
            save_to_target(
                &doc,
                &dialogs,
                &save_progress,
                &target,
                default_printer,
                default_resin,
            )
            .await
        }
    }
}

async fn pick_save_target(default_stem: Option<String>) -> Option<PathBuf> {
    let mut dialog = rfd::AsyncFileDialog::new()
        .add_filter(ctr!("FileDialog", "3MF files").to_string(), &["3mf"]);
    if let Some(stem) = default_stem {
        dialog = dialog.set_file_name(format!("{stem}.3mf"));
    }
    dialog.save_file().await.map(|h| h.path().to_path_buf())
}

fn show_save_error(dialogs: &Rc<DialogManager>, target: &std::path::Path, err: &anyhow::Error) {
    dialogs.show_error(
        ctr!("ErrorDialog", "Failed to save").as_str(),
        Some(ctr!("ErrorDialog", "File: {}", target.display()).to_string()),
        err.to_string(),
        format_anyhow_chain(err),
    );
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
                 .stl (binary STL), .stla (ASCII STL), .obj, or .3mf."
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
    let threemf = (
        ctr!("FileDialog", "3MF files").to_string(),
        ["3mf"].as_slice(),
    );
    let ordered: [&(String, &[&str]); 4] = match default_extension {
        "obj" => [&obj, &stl_bin, &stl_ascii, &threemf],
        "stla" => [&stl_ascii, &stl_bin, &obj, &threemf],
        "3mf" => [&threemf, &stl_bin, &stl_ascii, &obj],
        // "stl" or fallback.
        _ => [&stl_bin, &stl_ascii, &obj, &threemf],
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
        // PLY / AMF are inferrable as `MeshFileType` but
        // `WriteFormat::try_from` rejects them, so the dispatch must
        // collapse them into `UnsupportedExtension` rather than letting
        // them escape as a "Ready" variant the exporter can't handle.
        for name in ["model.ply", "model.amf"] {
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
            ("/tmp/out.3mf", WriteFormat::ThreeMf),
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

#[cfg(test)]
mod open_dialog_dir_tests {
    use tempfile::tempdir;

    use super::resolve_open_dialog_dir;

    #[test]
    fn prefers_an_existing_last_open_dir() {
        let last = tempdir().unwrap();
        let home = tempdir().unwrap();
        let resolved = resolve_open_dialog_dir(
            Some(last.path().to_path_buf()),
            Some(home.path().to_path_buf()),
        );
        assert_eq!(resolved.as_deref(), Some(last.path()));
    }

    #[test]
    fn falls_back_to_home_when_the_last_dir_is_gone() {
        // A remembered directory that no longer exists is skipped in
        // favour of the home directory.
        let missing = tempdir().unwrap();
        let missing_path = missing.path().to_path_buf();
        drop(missing);
        let home = tempdir().unwrap();
        let resolved = resolve_open_dialog_dir(Some(missing_path), Some(home.path().to_path_buf()));
        assert_eq!(resolved.as_deref(), Some(home.path()));
    }

    #[test]
    fn falls_back_to_home_when_no_last_dir_is_set() {
        let home = tempdir().unwrap();
        let resolved = resolve_open_dialog_dir(None, Some(home.path().to_path_buf()));
        assert_eq!(resolved.as_deref(), Some(home.path()));
    }

    #[test]
    fn yields_nothing_when_neither_dir_is_usable() {
        // No remembered directory and no resolvable home leaves the
        // dialog at the platform default.
        assert_eq!(resolve_open_dialog_dir(None, None), None);
    }

    #[test]
    fn skips_a_home_dir_that_does_not_exist() {
        let missing = tempdir().unwrap();
        let missing_path = missing.path().to_path_buf();
        drop(missing);
        assert_eq!(resolve_open_dialog_dir(None, Some(missing_path)), None);
    }
}
