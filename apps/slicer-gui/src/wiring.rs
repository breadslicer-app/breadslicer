/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Adapter layer that connects the Slint callbacks of a `MainWindow` to its
//! `ViewportController`, and runs the file dialogs and the worker threads of
//! the open, save and export commands.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use futures_channel::oneshot;
use slint::ComponentHandle;

use crate::error_dialog::{DialogManager, ErrorTexts, format_anyhow_chain};
use crate::export_progress_panel::ExportProgressPanel;
use crate::i18n::ctr;
use crate::mesh_loader::{self, LoadDispatch};
use crate::preferences_bus::{PreferencesBus, PreferencesChange};
use crate::print_selection::PrintDefaults;
use crate::project_versions::ProjectVersions;
use crate::save_progress_panel::SaveProgressPanel;
use crate::scene_document::{SaveRecorded, SceneDocument};
use crate::scene_export;
use crate::scene_loader::LoadTicket;
use crate::scene_save::{self, ProjectFormat, SaveDispatch, SaveTargetDispatch};
use crate::viewport::{RESIZE_SETTLE, ScreenDelta, ScreenPoint, ViewportController};
use crate::{MainWindow, StartOpen};

mod export_flow_dispatch {
    use std::path::PathBuf;

    use sc_mesh_formats::filetypes::{WriteFormat, infer_mesh_file_format_from_extension};

    /// Classification of the file-save dialog's outcome for the "Export As..."
    /// flow. Pure: derived only from whether the user picked a path and whether
    /// that path's extension maps to a writeable format. The variants own the
    /// picked path, so the caller can show it in an error dialog or pass it to
    /// the exporter.
    #[derive(Debug, PartialEq, Eq)]
    pub(crate) enum ExportDispatch {
        /// The user dismissed the dialog.
        Cancelled,
        /// The user picked a path, but its extension is either not inferrable
        /// or maps to a format the writer side does not support (for example
        /// `.ply` or `.amf`).
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

/// Installs the Slint callbacks of a `MainWindow` for rendering, the viewport,
/// the edit menu, the workspace sections, and the open, close, save and export
/// commands. `prefs` gives the open dialog its start directory and the save
/// commands the app-wide defaults. `confirm_open` opens a file in place of
/// the scene, asking first about unsaved changes; `open_files` opens files
/// as a drop does: models join the scene (#384).
pub(crate) fn wire_main_window(
    ui: &MainWindow,
    ctrl: Rc<ViewportController>,
    prefs: Rc<PreferencesBus>,
    confirm_open: Rc<dyn Fn(PathBuf)>,
    open_files: Rc<dyn Fn(Vec<PathBuf>)>,
) -> Result<(), slint::SetRenderingNotifierError> {
    install_rendering_notifier(ui, ctrl.clone())?;
    wire_open_file(
        ui,
        ctrl.clone(),
        prefs.clone(),
        confirm_open.clone(),
        open_files,
    );
    wire_save(ui, ctrl.clone(), prefs.clone());
    wire_save_as(ui, ctrl.clone(), prefs.clone());
    wire_save_as_3mf(ui, ctrl.clone(), prefs.clone());
    wire_previous_versions(ui, ctrl.clone(), confirm_open);
    wire_export_as(ui, ctrl.clone());
    wire_edit_menu(ui, ctrl.clone());
    wire_workspace_section(ui, ctrl.clone());
    wire_selected_layer(ui, ctrl.clone());
    wire_viewport_resized(ui, ctrl.clone());
    wire_viewport_input(ui, ctrl, prefs);
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
    {
        // Esc drops a Supports tab drag in flight (#3).
        let ctrl = ctrl.clone();
        ui.on_support_drag_cancel(move || ctrl.cancel_lasso() || ctrl.cancel_support_drag());
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
                slint::GraphicsAPI::WGPU30 { device, queue, .. } => {
                    crate::startup_timing::mark("window surface");
                    ctrl.handle_rendering_setup(device.clone(), queue.clone());
                    crate::startup_timing::mark("viewport renderer");
                }
                // Slint hands over whatever API the surface it built runs on,
                // and the viewport is drawn with wgpu, so any other API leaves
                // this window without a device for good. Recording which API
                // arrived lets the user be told and a report name it.
                other => ctrl.handle_unsupported_graphics_api(format!("{other:?}")),
            },
            slint::RenderingState::BeforeRendering => {
                crate::startup_timing::frame_starting();
                ctrl.handle_before_rendering();
            }
            slint::RenderingState::AfterRendering => {
                crate::startup_timing::frame_presented();
            }
            slint::RenderingState::RenderingTeardown => {
                ctrl.handle_rendering_teardown();
            }
            _ => {}
        })
}

fn wire_viewport_resized(ui: &MainWindow, ctrl: Rc<ViewportController>) {
    // A single `Timer` captured by the closure below acts as the resize
    // debounce: each `viewport-resized` restarts it, and when no event arrives
    // for `RESIZE_SETTLE`, it fires `on_resize_settled` to snap the renderer
    // allocations back to exact pixel size.
    let settle_timer = slint::Timer::default();
    ui.on_viewport_resized(move |w, h| {
        ctrl.on_viewport_resized(w, h);
        let ctrl_settled = ctrl.clone();
        settle_timer.start(slint::TimerMode::SingleShot, RESIZE_SETTLE, move || {
            ctrl_settled.on_resize_settled()
        });
    });
}

/// Installs the camera and selection callbacks of the viewport. A projection
/// switch is stored in `prefs`, so later windows and launches start with it.
pub(crate) fn wire_viewport_input(
    ui: &MainWindow,
    ctrl: Rc<ViewportController>,
    prefs: Rc<PreferencesBus>,
) {
    {
        let ctrl = ctrl.clone();
        ui.on_rotate_drag(move |dx, dy| ctrl.on_rotate(ScreenDelta::new(dx, dy)));
    }
    {
        let ctrl = ctrl.clone();
        ui.on_orbit_begin(move |x, y| ctrl.on_orbit_begin(x, y));
    }
    {
        let ctrl = ctrl.clone();
        ui.on_orbit_end(move || ctrl.on_orbit_end());
    }
    {
        let ctrl = ctrl.clone();
        ui.on_orbit_step(move |h, v| ctrl.on_orbit_step(h, v));
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
        ui.on_frame_selected_requested(move || ctrl.on_frame_selected());
    }
    {
        let ctrl = ctrl.clone();
        ui.on_toggle_projection_requested(move || {
            remember_projection(&prefs, ctrl.on_toggle_projection())
        });
    }
    {
        let ctrl = ctrl.clone();
        ui.on_frame_view_requested(move || ctrl.on_frame_view());
    }
    {
        let ctrl = ctrl.clone();
        ui.on_plate_add_requested(move || ctrl.add_plate());
    }
    {
        let ctrl = ctrl.clone();
        ui.on_plate_send_requested({
            let ctrl = ctrl.clone();
            move |plate| ctrl.send_selection_to_plate(u32::try_from(plate).unwrap_or(0))
        });
        ui.on_plate_picked(move |plate| {
            ctrl.document()
                .set_active_plate(u32::try_from(plate).unwrap_or(0));
        });
    }
    {
        let ctrl = ctrl.clone();
        ui.on_plate_remove_requested(move |with_models| ctrl.remove_active_plate_with(with_models));
    }
    {
        let ctrl = ctrl.clone();
        ui.on_plate_name_edited(move |text, caret, anchor| {
            ctrl.plate_name_edited(
                text.as_str(),
                usize::try_from(caret).unwrap_or(0),
                usize::try_from(anchor).unwrap_or(0),
            );
        });
    }
    {
        let ctrl = ctrl.clone();
        ui.on_plate_name_done(move |keep| ctrl.plate_name_done(keep));
    }
    {
        let ctrl = ctrl.clone();
        ui.on_axis_view_requested(move |code| ctrl.on_axis_view(code));
    }
    {
        let ctrl = ctrl.clone();
        ui.on_object_list_selection_changed(move |idx| {
            // Slint passes an i32. A negative index means "no selection", which
            // the controller clears through `on_clear_selection`.
            if idx >= 0 {
                ctrl.on_select_by_index(idx as usize);
            } else {
                ctrl.on_clear_selection();
            }
        });
    }
    {
        let ctrl = ctrl.clone();
        ui.on_object_list_toggle(move |idx| {
            if idx >= 0 {
                ctrl.on_toggle_by_index(idx as usize);
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
        ui.on_tab_tools_changed(move || ctrl.on_tab_tools_changed());
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
    {
        let ctrl = ctrl.clone();
        ui.on_numeric_edit(move |code, value| ctrl.on_numeric_edit(code, value));
    }
    {
        let ctrl = ctrl.clone();
        ui.on_modal_transform_start(move |code, x, y| ctrl.on_modal_transform_start(code, x, y));
    }
    {
        let ctrl = ctrl.clone();
        ui.on_modal_transform_axis(move |axis, x, y| ctrl.on_modal_transform_axis(axis, x, y));
    }
    {
        let ctrl = ctrl.clone();
        ui.on_modal_transform_finish(move |confirm| ctrl.on_modal_transform_finish(confirm));
    }
    ui.on_selection_cleared(move || ctrl.on_clear_selection());
}

fn wire_open_file(
    ui: &MainWindow,
    ctrl: Rc<ViewportController>,
    prefs: Rc<PreferencesBus>,
    confirm_open: Rc<dyn Fn(PathBuf)>,
    open_files: Rc<dyn Fn(Vec<PathBuf>)>,
) {
    {
        let ctrl = ctrl.clone();
        ui.on_scene_load_finished(move || ctrl.on_scene_load_finished());
    }
    {
        let ctrl = ctrl.clone();
        ui.on_import_notes_requested(move || ctrl.loader().open_import_notes());
    }

    let open = move |kind: OpenKind| {
        ctrl.on_modal_transform_finish(false);
        // The Slint event loop keeps processing callbacks while the native file
        // dialog is open, and `enabled: !root.loading` on the menu item cannot
        // stop them. `begin_load` gates the command in Rust before the dialog
        // opens, so `loading` flips at once and the ticket holds the gate for
        // the whole load.
        let Some(ticket) = ctrl.loader().begin_load() else {
            return;
        };
        if let Err(err) = slint::spawn_local(run_open_file_flow(
            ticket,
            ctrl.clone(),
            prefs.clone(),
            confirm_open.clone(),
            open_files.clone(),
            kind,
        )) {
            eprintln!("Failed to spawn file dialog task: {err}");
            // `ticket` was moved into the future; when `spawn_local` returns
            // `Err`, the future is dropped and the ticket with it, releasing
            // the gate.
        }
    };
    let open = Rc::new(open);
    {
        let open = open.clone();
        ui.on_open_file_requested(move || open(OpenKind::Project));
    }
    {
        let open = open.clone();
        ui.on_import_model_requested(move || open(OpenKind::Model));
    }
    // The start dialog's Open project and Import model (#381).
    ui.on_start_open_file(move |kind| {
        open(match kind {
            StartOpen::Project => OpenKind::Project,
            StartOpen::Model => OpenKind::Model,
        });
    });
}

/// What the open dialog is for. Its first filter is the one shown when it
/// opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OpenKind {
    /// A file in place of the scene (File > Open project), led by the
    /// Breadslicer projects.
    Project,
    /// Models to add to the scene (File > Import model, #384): STL, OBJ and
    /// 3MF, several at once.
    Model,
}

/// Orchestrates 1 "Open..." command: show the dialog, hand the picked path to
/// the shared post-pick pipeline. The caller takes `ticket` before the dialog
/// opens, so `loading` flips at once; if the user cancels, the ticket drops
/// here and the gate releases.
///
/// The dialog starts in the directory of the last opened file (falling back to
/// the home directory), and that directory is remembered after a successful
/// pick. The build envelope is read after the dialog closes, so the placement
/// uses the envelope that the window holds when the user finishes picking.
async fn run_open_file_flow(
    ticket: LoadTicket,
    ctrl: Rc<ViewportController>,
    prefs: Rc<PreferencesBus>,
    confirm_open: Rc<dyn Fn(PathBuf)>,
    open_files: Rc<dyn Fn(Vec<PathBuf>)>,
    kind: OpenKind,
) {
    let start_dir =
        resolve_open_dialog_dir(prefs.snapshot().last_open_dir, sc_app_paths::home_dir());
    if kind == OpenKind::Model {
        let paths = pick_model_files(ctrl.dialogs().file_dialog(), start_dir).await;
        // The import takes a ticket of its own: the dialog's goes first.
        drop(ticket);
        if let Some(first) = paths.first() {
            remember_open_directory(&prefs, first);
            open_files(paths);
        }
        return;
    }
    let Some(path) = pick_mesh_file(ctrl.dialogs().file_dialog(), start_dir, kind).await else {
        // User cancelled the dialog.
        return;
    };
    remember_open_directory(&prefs, &path);
    let Some((ticket, path)) = route_picked_open(
        ticket,
        path,
        ctrl.document().is_dirty(),
        confirm_open.as_ref(),
    ) else {
        return;
    };
    let loader = ctrl.loader();
    match mesh_loader::decide_load_dispatch(loader.graphics_unavailable()) {
        LoadDispatch::Start => {
            mesh_loader::start_load_with_ticket(ticket, path, &loader, ctrl.build_volume());
        }
        LoadDispatch::Queue => {
            // The viewport has not started yet; on a new window it starts at
            // the first paint. Park the pick where `handle_rendering_setup`
            // drains it, and let the ticket drop so the loader gate reopens;
            // the drain takes a fresh one when it dispatches.
            ctrl.pending_load().queue(path);
            drop(ticket);
        }
        LoadDispatch::Report(reason) => {
            // The deposit sends the failure down the path that reports load
            // errors, so the user gets the usual dialog that names the picked
            // file.
            ticket.deposit(path, Err(anyhow::Error::new(reason)));
        }
    }
}

/// Release the picker gate BEFORE a dirty-document continuation can save/open.
/// A clean document retains its ticket and the existing failure-preserving load.
fn route_picked_open(
    ticket: LoadTicket,
    path: PathBuf,
    dirty: bool,
    confirm_open: &dyn Fn(PathBuf),
) -> Option<(LoadTicket, PathBuf)> {
    if dirty {
        drop(ticket);
        confirm_open(path);
        None
    } else {
        Some((ticket, path))
    }
}

#[cfg(test)]
mod projection_tests {
    use super::*;
    use crate::mesh_render::DeviceLoss;
    use crate::pending_load::PendingLoad;
    use crate::preferences_bus::PreferencesSnapshot;
    use crate::scene_document::SceneDocument;
    use crate::test_support::slint_thread::on_fresh_slint_thread;

    fn window_on(bus: &Rc<PreferencesBus>) -> (MainWindow, Rc<ViewportController>) {
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let ctrl = Rc::new(ViewportController::new(
            &window,
            SceneDocument::new(),
            Rc::new(PendingLoad::new()),
            DeviceLoss::default(),
            bus,
        ));
        wire_viewport_input(&window, ctrl.clone(), bus.clone());
        (window, ctrl)
    }

    #[test]
    fn a_projection_switch_is_stored_and_starts_the_next_window() {
        on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let (first, _first_ctrl) = window_on(&bus);
            assert!(!first.get_orthographic_view());

            first.invoke_toggle_projection_requested();
            assert!(first.get_orthographic_view());
            assert!(bus.snapshot().orthographic_view);

            let (second, second_ctrl) = window_on(&bus);
            assert!(second.get_orthographic_view());
            assert!(second_ctrl.orthographic_view());

            // Switching back is stored too, and the other open window keeps
            // its own view.
            first.invoke_toggle_projection_requested();
            assert!(!first.get_orthographic_view());
            assert!(!bus.snapshot().orthographic_view);
            assert!(second.get_orthographic_view());
        });
    }
}

#[cfg(test)]
mod guarded_open_tests {
    use super::*;
    use crate::preferences_bus::PreferencesSnapshot;
    use crate::scene_loader::SceneLoader;
    use std::cell::Cell;

    fn loader() -> SceneLoader {
        SceneLoader::new_for_test(DialogManager::new(&PreferencesBus::new(
            PreferencesSnapshot::for_tests(),
        )))
    }

    #[test]
    fn dirty_menu_open_releases_gate_before_confirmation_continuation() {
        let loader = loader();
        let ticket = loader.begin_load().unwrap();
        let called = Cell::new(false);
        let result = route_picked_open(ticket, PathBuf::from("next.3mf"), true, &|path| {
            assert_eq!(path, PathBuf::from("next.3mf"));
            // Save & Open must be able to acquire a fresh load ticket.
            let next = loader
                .begin_load()
                .expect("picker gate still held inside continuation");
            called.set(true);
            drop(next);
        });
        assert!(called.get());
        assert!(result.is_none());
        assert!(loader.begin_load().is_some());
    }

    #[test]
    fn clean_menu_open_retains_gate_and_never_calls_destructive_confirmation() {
        let loader = loader();
        let ticket = loader.begin_load().unwrap();
        let result = route_picked_open(ticket, PathBuf::from("broken.3mf"), false, &|_| {
            panic!("clean load must preserve the current scene on failure")
        });
        assert!(result.is_some());
        assert!(loader.begin_load().is_none());
        drop(result);
        assert!(loader.begin_load().is_some());
    }
}

/// Picks the directory the open dialog should start in: the last opened file's
/// directory when it still exists, otherwise the home directory, otherwise the
/// platform default (no explicit directory).
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

/// Records the directory an opened file lived in so the next open dialog starts
/// there. No-op when the path has no parent or the directory is already the
/// remembered one (avoids a redundant settings write).
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

/// Stores the projection that the viewport switched to.
fn remember_projection(prefs: &PreferencesBus, orthographic: bool) {
    prefs.update(move |snap| {
        snap.orthographic_view = orthographic;
        PreferencesChange::Projection
    });
}

/// The extensions of the models the app imports: every one it opens but the
/// project's.
const MODEL_EXTENSIONS: &[&str] = &["stl", "stla", "stlb", "obj", "3mf"];

async fn pick_mesh_file(
    dialog: rfd::AsyncFileDialog,
    start_dir: Option<PathBuf>,
    kind: OpenKind,
) -> Option<PathBuf> {
    open_dialog(dialog, start_dir, kind)
        .pick_file()
        .await
        .map(|h| h.path().to_path_buf())
}

/// The models picked to import, several at once; none when cancelled.
async fn pick_model_files(
    dialog: rfd::AsyncFileDialog,
    start_dir: Option<PathBuf>,
) -> Vec<PathBuf> {
    open_dialog(dialog, start_dir, OpenKind::Model)
        .pick_files()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|h| h.path().to_path_buf())
        .collect()
}

/// The open dialog for `kind`, its filters set.
fn open_dialog(
    dialog: rfd::AsyncFileDialog,
    start_dir: Option<PathBuf>,
    kind: OpenKind,
) -> rfd::AsyncFileDialog {
    let all_supported = (
        ctr!("FileDialog", "All supported files").to_string(),
        mesh_loader::SUPPORTED_MESH_EXTENSIONS,
    );
    let projects = (
        ctr!("FileDialog", "Breadslicer projects").to_string(),
        &["bread"][..],
    );
    let lead = match kind {
        OpenKind::Project => vec![projects, all_supported],
        OpenKind::Model => vec![
            (
                ctr!("FileDialog", "3D models").to_string(),
                MODEL_EXTENSIONS,
            ),
            all_supported,
            projects,
        ],
    };
    let mut dialog = lead
        .into_iter()
        .fold(dialog, |dialog, (name, extensions)| {
            dialog.add_filter(name, extensions)
        })
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
    dialog
}

fn wire_save(ui: &MainWindow, ctrl: Rc<ViewportController>, prefs: Rc<PreferencesBus>) {
    ui.on_save_requested(move || {
        ctrl.on_modal_transform_finish(false);
        let ctrl = ctrl.clone();
        let defaults = PrintDefaults::from(&prefs.snapshot());
        if let Err(err) = slint::spawn_local(async move {
            let _ = run_save_flow(
                ctrl.document(),
                ctrl.dialogs(),
                ctrl.save_progress(),
                defaults,
            )
            .await;
            // A first "Save" falls back to "Save As", which relocates the
            // document. So does a write that completed while the document moved
            // on, which reports itself as not saved, so mirror the name
            // whatever the outcome was.
            ctrl.refresh_document_title();
        }) {
            eprintln!("Failed to spawn save task: {err}");
        }
    });
}

/// Result of 1 Save or Save As command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SaveOutcome {
    /// The document's current state is on disk. The only outcome that lets a
    /// chained caller discard the scene.
    Saved,
    /// A file was written, but from a snapshot the document has since left
    /// behind. The event loop stays live during a write, so an edit, a printer
    /// change or an Open can land after the snapshot was taken; the file holds
    /// none of it.
    MovedOn,
    /// Nothing reached the disk: no scene, a cancelled dialog, a wrong
    /// extension, or a write failure that surfaced its own dialog.
    NotSaved,
}

impl SaveOutcome {
    /// Reads a completed write's recording as a save outcome.
    fn from_recorded(recorded: SaveRecorded) -> Self {
        match recorded {
            SaveRecorded::Clean => Self::Saved,
            // Both mean the file does not hold what the document holds now,
            // whether it drifted or was swapped outright.
            SaveRecorded::Drifted | SaveRecorded::Replaced => Self::MovedOn,
        }
    }
}

/// Orchestrates 1 "Save" command: overwrite the document's project file, fall
/// back to the Save As flow when there is none, and treat a clean project as
/// already saved without touching the disk. Reports [`SaveOutcome::Saved`] only
/// when the document's current state is on disk by the time the flow ends. A
/// cancelled dialog, a wrong extension and a failed write report nothing
/// written; the last 2 show an error dialog.
///
/// `defaults` holds the app-wide defaults from when the command fired, so a
/// default changed while the Save As dialog is open does not reach the file.
pub(crate) async fn run_save_flow(
    doc: Rc<SceneDocument>,
    dialogs: Rc<DialogManager>,
    save_progress: Rc<SaveProgressPanel>,
    defaults: PrintDefaults,
) -> SaveOutcome {
    match scene_save::decide_save_dispatch(&doc) {
        SaveDispatch::NoScene => SaveOutcome::NotSaved,
        SaveDispatch::AlreadySaved => SaveOutcome::Saved,
        SaveDispatch::OverwriteProject(target) => {
            save_to_target(&doc, &dialogs, &save_progress, &target, &defaults).await
        }
        SaveDispatch::NeedsSaveAs => {
            let default_stem = scene_export::default_export_stem(&doc);
            run_save_as_flow(
                doc,
                dialogs,
                save_progress,
                default_stem,
                defaults,
                ProjectFormat::Bread,
            )
            .await
        }
    }
}

/// Writes a project to `target` off the UI thread, driving the save-progress
/// panel. Snapshots the scene and payload on the UI thread, spawns a worker for
/// the serialize + compress + disk write, and awaits its result over a oneshot
/// delivered back through the event loop, so the event loop stays responsive
/// throughout.
///
/// Reports [`SaveOutcome::Saved`] only when the document's current state is the
/// one that reached the disk. The event loop keeps running during the write, so
/// the user can edit, change the printer, or open another file in that window;
/// the file then holds none of that, the document stays dirty, and the outcome
/// is [`SaveOutcome::MovedOn`]. A write failure surfaces the error dialog.
async fn save_to_target(
    doc: &Rc<SceneDocument>,
    dialogs: &Rc<DialogManager>,
    save_progress: &Rc<SaveProgressPanel>,
    target: &Path,
    defaults: &PrintDefaults,
) -> SaveOutcome {
    let Some((bundle, payload, stamp)) =
        scene_save::prepare_save_for_target(doc, target.to_path_buf(), defaults)
    else {
        // The menu gates this, but a stale callback could still race a close:
        // no scene means nothing to write.
        return SaveOutcome::NotSaved;
    };

    let supports_as_mesh = bundle.supports_as_mesh();
    let paint_left_out = bundle.leaves_paint_out();
    crate::breadcrumbs::started("save");
    crate::breadcrumbs::note(&format!(
        "save: a {} file",
        mesh_loader::extension_of(target)
    ));
    save_progress.begin();
    let sink = save_progress.sink();
    let (tx, rx) = oneshot::channel();
    // A `.bread` about to be replaced is kept as an earlier version first
    // (#18).
    let versions = ProjectVersions::platform_default();
    let replaced = target.to_path_buf();
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if let Some(versions) = &versions
                && let Err(err) = versions.keep_previous(&replaced)
            {
                // The save itself goes on: the version is a spare copy.
                crate::breadcrumbs::note(&format!("versions: not kept: {err:#}"));
            }
            scene_save::write_bundle(&bundle, |phase, done, total| sink.set(phase, done, total))
        }))
        .unwrap_or_else(|payload| Err(anyhow::anyhow!(mesh_loader::panic_message(&payload))));
        // Deliver on the UI thread so the awaiting task wakes there. If the
        // event loop is gone (shutdown), the sender drops and the receiver
        // resolves to a cancellation.
        let _ = slint::invoke_from_event_loop(move || {
            let _ = tx.send(result);
        });
    });

    let outcome = rx.await;
    save_progress.end();
    let ended = match &outcome {
        Ok(result) => crate::breadcrumbs::outcome(result),
        Err(_canceled) => "stopped: the app is closing".to_owned(),
    };
    crate::breadcrumbs::finished("save", &ended);
    match outcome {
        Ok(Ok(())) => {
            // The file on disk is a project to reopen later (#381).
            crate::recent_files::note(target);
            if let Some(why) = supports_as_mesh {
                dialogs.show_info(move || scene_save::supports_as_mesh_texts(why));
            }
            if paint_left_out {
                dialogs.show_info(scene_save::paint_left_out_texts);
            }
            SaveOutcome::from_recorded(doc.record_project_save_for(
                payload,
                target.to_path_buf(),
                stamp,
            ))
        }
        Ok(Err(err)) => {
            show_save_error(dialogs, target, err);
            SaveOutcome::NotSaved
        }
        // The sender dropped without sending (event loop gone at shutdown):
        // treat as not saved.
        Err(_canceled) => SaveOutcome::NotSaved,
    }
}

/// File > "Previous Versions…" (#18): picks 1 of the kept versions of the open
/// project and opens it through `open`, the external open path (which asks
/// first about a scene in the window). The version opens as the project itself
/// with unsaved changes (see `crate::autosave::recovered_identity`), so Save
/// puts it back as the project.
fn wire_previous_versions(
    ui: &MainWindow,
    ctrl: Rc<ViewportController>,
    open: Rc<dyn Fn(PathBuf)>,
) {
    ui.on_previous_versions_requested(move || {
        ctrl.on_modal_transform_finish(false);
        let Some(dir) = crate::autosave::versions_dir_of(&ctrl.document()) else {
            return;
        };
        let dialog = ctrl
            .dialogs()
            .file_dialog()
            .set_title(ctr!("FileDialog", "Previous versions").to_string())
            .add_filter(
                ctr!("FileDialog", "Breadslicer projects").to_string(),
                &["bread"],
            )
            .set_directory(dir);
        let open = open.clone();
        if let Err(err) = slint::spawn_local(async move {
            if let Some(picked) = dialog.pick_file().await {
                open(picked.path().to_path_buf());
            }
        }) {
            eprintln!("Failed to spawn previous versions dialog task: {err}");
        }
    });
}

fn wire_save_as(ui: &MainWindow, ctrl: Rc<ViewportController>, prefs: Rc<PreferencesBus>) {
    ui.on_save_as_requested(move || {
        ctrl.on_modal_transform_finish(false);
        spawn_save_as_flow(
            ctrl.clone(),
            PrintDefaults::from(&prefs.snapshot()),
            ProjectFormat::Bread,
        );
    });
}

/// "Save As 3MF": the same flow as "Save As", for a 3MF project. Projects are
/// `.bread` for now (#439): no menu or shortcut reaches it, and it stays for
/// the day one does.
fn wire_save_as_3mf(ui: &MainWindow, ctrl: Rc<ViewportController>, prefs: Rc<PreferencesBus>) {
    ui.on_save_as_3mf_requested(move || {
        ctrl.on_modal_transform_finish(false);
        spawn_save_as_flow(
            ctrl.clone(),
            PrintDefaults::from(&prefs.snapshot()),
            ProjectFormat::ThreeMf,
        );
    });
}

fn spawn_save_as_flow(
    ctrl: Rc<ViewportController>,
    defaults: PrintDefaults,
    format: ProjectFormat,
) {
    let default_stem = scene_export::default_export_stem(&ctrl.document());
    if let Err(err) = slint::spawn_local(async move {
        let _ = run_save_as_flow(
            ctrl.document(),
            ctrl.dialogs(),
            ctrl.save_progress(),
            default_stem,
            defaults,
            format,
        )
        .await;
        // "Save As" relocates the document even when the write completed
        // against a moved-on document and reported itself as not saved, so
        // mirror the name whatever the outcome was.
        ctrl.refresh_document_title();
    }) {
        eprintln!("Failed to spawn save dialog task: {err}");
    }
}

/// Orchestrates 1 "Save As" command: show the file-save dialog (seeded with the
/// source stem and the extension of `format`), validate the picked name, and
/// write the project. A cancelled dialog, a wrong extension and a failed write
/// report nothing written; the last 2 show an error dialog.
///
/// `defaults` holds the app-wide defaults from when the command fired, so a
/// default changed while the Save As dialog is open does not reach the file.
async fn run_save_as_flow(
    doc: Rc<SceneDocument>,
    dialogs: Rc<DialogManager>,
    save_progress: Rc<SaveProgressPanel>,
    default_stem: Option<String>,
    defaults: PrintDefaults,
    format: ProjectFormat,
) -> SaveOutcome {
    let dispatch = scene_save::decide_save_target(
        pick_save_target(dialogs.file_dialog(), default_stem, format).await,
        format,
    );
    match dispatch {
        SaveTargetDispatch::Cancelled => SaveOutcome::NotSaved,
        SaveTargetDispatch::WrongExtension(target) => {
            let err = anyhow::Error::new(scene_save::WrongProjectExtension(format));
            show_save_error(&dialogs, &target, err);
            SaveOutcome::NotSaved
        }
        SaveTargetDispatch::Ready(target) => {
            save_to_target(&doc, &dialogs, &save_progress, &target, &defaults).await
        }
    }
}

async fn pick_save_target(
    dialog: rfd::AsyncFileDialog,
    default_stem: Option<String>,
    format: ProjectFormat,
) -> Option<PathBuf> {
    let name = match format {
        ProjectFormat::Bread => ctr!("FileDialog", "Breadslicer projects"),
        ProjectFormat::ThreeMf => ctr!("FileDialog", "3MF files"),
    };
    let extension = format.extension();
    let mut dialog = dialog.add_filter(name.to_string(), &[extension]);
    if let Some(stem) = default_stem {
        dialog = dialog.set_file_name(format!("{stem}.{extension}"));
    }
    dialog.save_file().await.map(|h| h.path().to_path_buf())
}

fn show_save_error(dialogs: &Rc<DialogManager>, target: &Path, err: anyhow::Error) {
    let target = target.to_path_buf();
    dialogs.show_error(move || save_error_texts(&target, &err));
}

/// The texts of the dialog that reports a project save with no file written.
fn save_error_texts(target: &Path, err: &anyhow::Error) -> ErrorTexts {
    ErrorTexts {
        heading: ctr!("ErrorDialog", "Failed to save"),
        context: Some(ctr!("ErrorDialog", "File: {}", target.display())),
        message: err.to_string().into(),
        details: format_anyhow_chain(err).into(),
    }
}

fn wire_export_as(ui: &MainWindow, ctrl: Rc<ViewportController>) {
    ui.on_export_as_requested(move || {
        ctrl.on_modal_transform_finish(false);
        let doc = ctrl.document();
        let dialogs = ctrl.dialogs();
        let export_progress = ctrl.export_progress();
        let default_stem = scene_export::default_export_stem(&doc);
        let default_extension = scene_export::default_export_extension(&doc);
        if let Err(err) = slint::spawn_local(async move {
            let _ = run_export_as_flow(
                doc,
                dialogs,
                export_progress,
                default_stem,
                default_extension,
            )
            .await;
        }) {
            eprintln!("Failed to spawn export dialog task: {err}");
        }
    });
}

/// Orchestrates 1 "Export As" command: show the file-save dialog, infer the
/// format from the chosen extension, snapshot the scene on the UI thread, and
/// write the file on a worker thread while the export-progress panel polls, so
/// the event loop stays responsive throughout. Returns `true` only when a file
/// was written (so a chained "Export & Open" caller knows it is safe to discard
/// the current scene); a cancelled dialog, an unknown extension and a failed
/// write return `false`. Errors and unknown extensions show their dialogs here.
pub(crate) async fn run_export_as_flow(
    doc: Rc<SceneDocument>,
    dialogs: Rc<DialogManager>,
    export_progress: Rc<ExportProgressPanel>,
    default_stem: Option<String>,
    default_extension: &'static str,
) -> bool {
    // rfd 0.17 doesn't expose the filter the user actually picked, so the
    // path's extension is the portable source of truth for the format. `.stl`
    // -> binary, `.stla` -> ASCII, `.obj` -> OBJ. The dialog seeds the active
    // filter's extension into the filename so the user gets the format they
    // implicitly chose.
    let dispatch = decide_export_dispatch(
        pick_export_target(dialogs.file_dialog(), default_stem, default_extension).await,
    );
    match dispatch {
        ExportDispatch::Cancelled => false,
        ExportDispatch::UnsupportedExtension(target) => {
            let err = anyhow::Error::new(scene_export::ExportRefusal::UnknownExtension);
            show_export_error(&dialogs, &target, err);
            false
        }
        ExportDispatch::Ready { target, format } => {
            // Snapshot on the UI thread: 1 borrow, 1 cheap clone that the
            // worker owns outright, so the written bytes match the on-screen
            // scene at this instant. The menu gates exports without a scene,
            // but a stale callback could still race the close path.
            let Some(scene) = scene_export::prepare_export(&doc) else {
                let err = anyhow::Error::new(scene_export::ExportRefusal::NoScene);
                show_export_error(&dialogs, &target, err);
                return false;
            };

            export_progress.begin();
            let sink = export_progress.sink();
            let (tx, rx) = oneshot::channel();
            let worker_target = target.clone();
            std::thread::spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    scene_export::write_export(
                        &scene,
                        &worker_target,
                        format,
                        |phase, done, total| sink.set(phase, done, total),
                    )
                }))
                .unwrap_or_else(|payload| {
                    Err(anyhow::anyhow!(mesh_loader::panic_message(&payload)))
                });
                // Deliver on the UI thread so the awaiting task wakes there. If
                // the event loop is gone (shutdown), the sender drops and the
                // receiver resolves to a cancellation.
                let _ = slint::invoke_from_event_loop(move || {
                    let _ = tx.send(result);
                });
            });

            let outcome = rx.await;
            export_progress.end();
            match outcome {
                Ok(Ok(warnings)) => {
                    for w in &warnings {
                        eprintln!("WARNING [{}]: {}", target.display(), w.user_message());
                    }
                    true
                }
                Ok(Err(err)) => {
                    show_export_error(&dialogs, &target, err);
                    false
                }
                // The sender dropped without sending (event loop gone at
                // shutdown): treat as not exported.
                Err(_canceled) => false,
            }
        }
    }
}

fn show_export_error(dialogs: &Rc<DialogManager>, target: &Path, err: anyhow::Error) {
    let target = target.to_path_buf();
    dialogs.show_error(move || export_error_texts(&target, &err));
}

/// The texts of the dialog that reports a scene export with no file written.
fn export_error_texts(target: &Path, err: &anyhow::Error) -> ErrorTexts {
    ErrorTexts {
        heading: ctr!("ErrorDialog", "Failed to export"),
        context: Some(ctr!("ErrorDialog", "File: {}", target.display())),
        message: err.to_string().into(),
        details: format_anyhow_chain(err).into(),
    }
}

async fn pick_export_target(
    dialog: rfd::AsyncFileDialog,
    default_stem: Option<String>,
    default_extension: &'static str,
) -> Option<PathBuf> {
    // Filters in source-format-first order. Most rfd backends pick the first
    // filter as the active one, which is also the extension we seed the
    // filename with, so the dialog opens on a consistent format pair.
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
    let mut dialog = dialog;
    for (name, exts) in ordered {
        dialog = dialog.add_filter(name.clone(), exts);
    }
    if let Some(stem) = default_stem {
        dialog = dialog.set_file_name(format!("{stem}.{default_extension}"));
    }
    dialog.save_file().await.map(|h| h.path().to_path_buf())
}

/// The save flow's promise to a chained caller: only an undisturbed write lets
/// that caller discard the scene it just saved.
#[cfg(test)]
mod save_outcome_tests {
    use super::*;

    #[test]
    fn a_clean_recording_is_the_only_saved_outcome() {
        assert_eq!(
            SaveOutcome::from_recorded(SaveRecorded::Clean),
            SaveOutcome::Saved,
        );
    }

    #[test]
    fn a_drifted_recording_is_not_saved() {
        assert_eq!(
            SaveOutcome::from_recorded(SaveRecorded::Drifted),
            SaveOutcome::MovedOn,
        );
    }

    #[test]
    fn a_replaced_document_is_not_saved() {
        assert_eq!(
            SaveOutcome::from_recorded(SaveRecorded::Replaced),
            SaveOutcome::MovedOn,
        );
    }
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
        // `WriteFormat::try_from` rejects them, so the dispatch must collapse
        // them into `UnsupportedExtension`; a "Ready" variant would hand the
        // exporter a format it cannot write.
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
        // `.stl` (no ASCII/binary hint) maps to binary STL by long-standing
        // project convention; see `WriteFormat::try_from(MeshFileType::Stl)`.
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
        // A remembered directory that no longer exists is skipped in favour of
        // the home directory.
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
        // No remembered directory and no resolvable home leaves the dialog at
        // the platform default.
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

#[cfg(test)]
mod dialog_text_tests {
    use super::*;

    #[test]
    fn no_menu_offers_to_save_a_3mf_project() {
        let window = include_str!("../ui/main-window.slint");
        assert!(!window.contains("Save As 3MF"));
        assert_eq!(window.matches("root.save-as-3mf-requested").count(), 0);
    }

    #[test]
    fn a_save_under_another_extension_says_projects_are_bread_files() {
        let err = anyhow::Error::new(scene_save::WrongProjectExtension(ProjectFormat::Bread));
        let texts = save_error_texts(Path::new("/tmp/plate.stl"), &err);
        assert_eq!(texts.heading, "Failed to save");
        assert_eq!(texts.context.as_deref(), Some("File: /tmp/plate.stl"));
        assert_eq!(texts.message, "Project files are saved as .bread files.");
        assert_eq!(texts.details, "");
    }

    #[test]
    fn a_3mf_save_under_another_extension_says_projects_are_3mf_packages() {
        let err = anyhow::Error::new(scene_save::WrongProjectExtension(ProjectFormat::ThreeMf));
        let texts = save_error_texts(Path::new("/tmp/plate.bread"), &err);
        assert_eq!(texts.message, "Project files are saved as .3mf packages.");
    }

    #[test]
    fn a_failed_save_shows_its_error_chain() {
        let err = anyhow::Error::new(std::io::Error::other("disk full"))
            .context("could not write the file");
        let texts = save_error_texts(Path::new("/tmp/plate.3mf"), &err);
        assert_eq!(texts.message, "could not write the file");
        assert_eq!(
            texts.details,
            "could not write the file\n  caused by: disk full"
        );
    }

    #[test]
    fn an_export_under_an_unknown_extension_lists_the_formats() {
        let err = anyhow::Error::new(scene_export::ExportRefusal::UnknownExtension);
        let texts = export_error_texts(Path::new("/tmp/plate.ply"), &err);
        assert_eq!(texts.heading, "Failed to export");
        assert_eq!(texts.context.as_deref(), Some("File: /tmp/plate.ply"));
        assert_eq!(
            texts.message,
            "Unable to infer file format from filename. Use one of: .stl (binary STL), .stla \
             (ASCII STL), .obj, or .3mf."
        );
        assert_eq!(texts.details, "");
    }
}
