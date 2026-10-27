/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Per-window driver for Tools > "Generate Supports (DragonFruit)":
//! runs `sc_geom::df_supports` (the native port of DragonFruit's
//! auto-support: island scan, placement, bracing, raft) on the scene.
//!
//! Like `GenSupportRun`, the job runs on its own worker thread behind a
//! `JobHandle`, and a `slint::Timer` polls its progress into the shared
//! support-generation panel, which shows a Cancel button while this run is in
//! flight. On completion the supports and raft, already tessellated and
//! welded into one mesh, are pasted into the scene through
//! `SceneDocument::paste_fragment`: one undoable object, saved and exported
//! like any other. A later run leaves that object out of its models.
//!
//! **Prescan (BS-66, BS-67, BS-68).** The island and overhang scans depend only on
//! the geometry, its pose and the scan settings, so a second, lower-key job
//! runs them ahead of time (`start_df_prescan`, in a pool of half the cores)
//! and the next run starts from them. It starts when the Hollow or Supports
//! tab is open (the pose is settled there), or, with the Preferences >
//! Slicing switch, whenever the scene has been still for [`PRESCAN_SETTLE`].
//! Nothing of it is on the transform path: a scene event only stamps the
//! time; a poll starts the prescan, and cancels it when the scene moves on.
//! A Generate press while a prescan of the same scene is running waits for
//! it rather than scanning twice. Reuse is safe by construction: the core
//! keys the scans on the baked world geometry, so a stale scan never
//! matches.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sc_geom::JobError;
use sc_geom::JobHandle;
use sc_geom::df_supports::{
    DfPrescan, DfSupportOptions, DfSupportOutcome, DfSupportRequest, DfSupportSession, EditPreview,
    RaftMode, ScanCache, start_df_prescan, start_df_supports,
};
use sc_mesh_core::transform::Transform3D;
use sc_print_profiles::resolve_layer_height;
use sc_scene::NodeId;
use sc_scene_edit::SceneFragment;
use slint::ComponentHandle;

use crate::error_dialog::{DialogManager, ErrorTexts, InfoTexts};
use crate::i18n::ctr;
use crate::mesh_render::{OverlayMesh, OverlayTint};
use crate::preferences_bus::PreferencesBus;
use crate::print_selection::PrintDefaults;
use crate::scene_document::{
    SceneDocument, SceneEvent, SceneRevision, SceneSubscription, leaf_mesh_swap,
};
use crate::slice_projection::load_selected_profiles;
use crate::viewport::ViewportController;
use crate::{MainWindow, WorkspaceSection};

/// An overlay piece of a mesh in world coordinates.
pub(crate) fn overlay_of(mesh: &sc_mesh_core::IndexedMesh, tint: OverlayTint) -> OverlayMesh {
    OverlayMesh {
        positions: mesh.vertices().iter().map(|v| *v.as_array()).collect(),
        indices: mesh
            .faces()
            .iter()
            .flat_map(|f| f.vertices.map(u32::from))
            .collect(),
        tint,
    }
}

const PROGRESS_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long the scene must sit still before the prescan starts.
const PRESCAN_SETTLE: Duration = Duration::from_millis(500);

/// How long the scene must sit still (and the Hollow tab idle) before the
/// supports are generated ahead of a Generate press.
const SPECULATE_SETTLE: Duration = Duration::from_millis(1500);

/// Supports generated ahead of a Generate press, for the scene and request
/// they were made for. A request is told apart by its printer figures and
/// options (`ahead_key`).
#[derive(Default)]
struct Ahead {
    running: Option<(JobHandle<DfSupportOutcome>, SceneRevision, String)>,
    ready: Option<(SceneRevision, String, DfSupportOutcome)>,
}

/// What tells two requests apart for [`Ahead`].
fn ahead_key(request: &DfSupportRequest) -> String {
    format!(
        "{:?}|{:?}|{:?}",
        request.layer_height_mm, request.pixel_pitch_mm, request.options
    )
}

/// How often the prescan watcher looks at the scene.
const PRESCAN_POLL_INTERVAL: Duration = Duration::from_millis(150);

/// The background prescan's state.
#[derive(Default)]
struct Prescan {
    /// The job in flight, the scene it started on and when.
    running: Option<(JobHandle<DfPrescan>, SceneRevision, Instant)>,
    /// The last finished prescan and the scene it ran on.
    ready: Option<(SceneRevision, Arc<ScanCache>)>,
    /// A Generate press waiting for the running prescan: the run starts
    /// from its scans as soon as it finishes.
    run_waiting: bool,
}

pub(crate) struct DfSupportRun {
    ui: slint::Weak<MainWindow>,
    bus: Rc<PreferencesBus>,
    document: Rc<SceneDocument>,
    dialogs: Rc<DialogManager>,
    handle: RefCell<Option<JobHandle<DfSupportOutcome>>>,
    // The scene the run was started on: a run that finishes on edited
    // geometry is discarded rather than pasted.
    started_on: RefCell<Option<SceneRevision>>,
    poll_timer: Rc<slint::Timer>,
    prescan: RefCell<Prescan>,
    /// Supports computed while the person works on something else.
    ahead: RefCell<Ahead>,
    /// When the scene last changed, live drag ticks included. Stamped by the
    /// scene subscription; that stamp is all the prescan costs an edit.
    last_change: Rc<Cell<Instant>>,
    prescan_timer: slint::Timer,
    _scene_events: SceneSubscription,
    /// The options the next run uses (the Supports tab), saved across
    /// sessions.
    options: RefCell<DfSupportOptions>,
    /// Built-in presets first, then the person's own.
    presets: RefCell<Vec<Preset>>,
    /// The last run's supports and every hand edit since, each with the scene
    /// revision it was drawn into. A click edits the entry matching the
    /// current scene, so Undo and Redo keep the editor in step; a scene that
    /// moved on otherwise is refused.
    sessions: RefCell<Vec<(Arc<DfSupportSession>, SceneRevision)>>,
    /// The viewport, for the hover preview of the hand tools.
    ctrl: Rc<ViewportController>,
    /// Computes hand-tool previews off the UI thread, always the latest.
    hover_worker: HoverWorker,
    /// The latest hover asked for.
    hover_generation: Cell<u64>,
    /// The newest answer drawn (or the hover cleared at): an answer older
    /// than it is stale.
    hover_shown: Cell<u64>,
    hover_poll: slint::Timer,
    /// The last preview drawn, for a click on the same point to reuse.
    last_preview: RefCell<Option<HoverAnswer>>,
    /// The worker's last ghost, kept on screen while the next is computed.
    last_ghost: RefCell<Option<Arc<OverlayMesh>>>,
}

/// How often the UI looks for a finished preview.
const HOVER_POLL: Duration = Duration::from_millis(16);

/// A preview to compute: the session to edit, where, and with which tool.
struct HoverAsk {
    generation: u64,
    /// Shared, not copied: a copy of a busy model's supports costs ~8 ms,
    /// paid on the worker instead of on every mouse move.
    base: Arc<DfSupportSession>,
    /// Which session (its index and the scene it was drawn on) `base` is.
    from: (usize, SceneRevision),
    point: [f32; 3],
    tool: i32,
}

/// A computed preview.
struct HoverAnswer {
    generation: u64,
    from: (usize, SceneRevision),
    point: [f32; 3],
    tool: i32,
    preview: Option<EditPreview>,
    /// For an Add that goes through: the session the click leaves.
    edited: Option<DfSupportSession>,
}

/// One background thread answering the newest ask; asks that queue up
/// behind a slow one are skipped.
struct HoverWorker {
    asks: std::sync::mpsc::Sender<HoverAsk>,
    answer: Arc<std::sync::Mutex<Option<HoverAnswer>>>,
}

impl HoverWorker {
    fn spawn() -> Self {
        let (asks, inbox) = std::sync::mpsc::channel::<HoverAsk>();
        let answer = Arc::new(std::sync::Mutex::new(None));
        let out = answer.clone();
        let _ = std::thread::Builder::new()
            .name("support-hover-preview".into())
            // DragonFruit's router recurses; room to spare on dense models.
            .stack_size(16 << 20)
            .spawn(move || {
                while let Ok(mut ask) = inbox.recv() {
                    while let Ok(newer) = inbox.try_recv() {
                        ask = newer;
                    }
                    if ask.tool == 0 {
                        // Warm-up after an edit: no answer to draw.
                        let base = ask.base.clone();
                        let _ = std::panic::catch_unwind(move || base.warm_removal());
                        continue;
                    }
                    let p = ask.point.map(f64::from);
                    let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        if ask.tool == 1 {
                            ask.base.preview_add(p)
                        } else {
                            ask.base.preview_remove_at(p).map(|v| (v, None))
                        }
                    }));
                    let (preview, edited) = match done {
                        Ok(Ok((preview, edited))) => (Some(preview), edited),
                        _ => (None, None),
                    };
                    if let Ok(mut slot) = out.lock() {
                        *slot = Some(HoverAnswer {
                            generation: ask.generation,
                            from: ask.from,
                            point: ask.point,
                            tool: ask.tool,
                            preview,
                            edited,
                        });
                    }
                }
            });
        HoverWorker { asks, answer }
    }
}

/// A named set of options.
#[derive(Clone)]
struct Preset {
    name: String,
    options: DfSupportOptions,
    built_in: bool,
}

/// The presets that ship with SoulCrafted: DragonFruit's three sizing tiers.
fn built_in_presets() -> Vec<Preset> {
    let tier = |name: &str, sizing: &str| Preset {
        name: name.to_owned(),
        options: DfSupportOptions {
            sizing: sizing.to_owned(),
            ..Default::default()
        },
        built_in: true,
    };
    vec![
        tier("Structure (default)", "structure"),
        tier("Detail (small miniatures)", "detail"),
        tier("Anchor (heavy parts)", "anchor"),
    ]
}

/// Where the options and presets live: the app's local data folder. Tests
/// never touch it.
fn support_data_dir() -> Option<std::path::PathBuf> {
    if cfg!(test) {
        return None;
    }
    Some(
        sc_app_paths::project_dirs()?
            .data_local_dir()
            .join("supports"),
    )
}

fn load_saved_options() -> DfSupportOptions {
    support_data_dir()
        .and_then(|d| std::fs::read_to_string(d.join("options.toml")).ok())
        .and_then(|text| toml::from_str::<DfSupportOptions>(&text).ok())
        .map(|o| o.clamped())
        .unwrap_or_default()
}

fn load_user_presets() -> Vec<Preset> {
    let Some(dir) = support_data_dir().map(|d| d.join("presets")) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut presets: Vec<Preset> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
        .filter_map(|e| {
            let name = e.path().file_stem()?.to_string_lossy().into_owned();
            let text = std::fs::read_to_string(e.path()).ok()?;
            let options = toml::from_str::<DfSupportOptions>(&text).ok()?.clamped();
            Some(Preset {
                name,
                options,
                built_in: false,
            })
        })
        .collect();
    presets.sort_by_key(|p| p.name.to_lowercase());
    presets
}

/// A preset name made safe as a file name.
fn preset_file_stem(name: &str) -> Option<String> {
    let stem: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " -_().".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let stem = stem.trim().trim_matches('.').to_owned();
    (!stem.is_empty()).then_some(stem)
}

impl DfSupportRun {
    /// Wires the Tools menu and the panel's Cancel button; the caller keeps
    /// the driver alive for the window's lifetime.
    pub(crate) fn install(
        ui: &MainWindow,
        bus: Rc<PreferencesBus>,
        document: Rc<SceneDocument>,
        dialogs: Rc<DialogManager>,
        ctrl: Rc<ViewportController>,
    ) -> Rc<Self> {
        let last_change = Rc::new(Cell::new(Instant::now()));
        let scene_events = document.subscribe({
            let last_change = last_change.clone();
            move |event| {
                if matches!(
                    event,
                    SceneEvent::Installed
                        | SceneEvent::Cleared
                        | SceneEvent::Mutated
                        | SceneEvent::Restructured
                ) {
                    last_change.set(Instant::now());
                }
            }
        });
        let run = Rc::new(Self {
            ui: ui.as_weak(),
            bus,
            document,
            dialogs,
            handle: RefCell::new(None),
            started_on: RefCell::new(None),
            poll_timer: Rc::new(slint::Timer::default()),
            prescan: RefCell::new(Prescan::default()),
            ahead: RefCell::new(Ahead::default()),
            last_change,
            prescan_timer: slint::Timer::default(),
            _scene_events: scene_events,
            options: RefCell::new(load_saved_options()),
            presets: RefCell::new(Vec::new()),
            sessions: RefCell::new(Vec::new()),
            ctrl,
            hover_worker: HoverWorker::spawn(),
            hover_generation: Cell::new(0),
            hover_shown: Cell::new(0),
            hover_poll: slint::Timer::default(),
            last_preview: RefCell::new(None),
            last_ghost: RefCell::new(None),
        });
        {
            let weak = Rc::downgrade(&run);
            ui.on_support_tool_hover(move |x, y, z, hit| {
                if let Some(run) = weak.upgrade() {
                    run.on_tool_hover(hit.then_some([x, y, z]));
                }
            });
        }
        {
            let run = run.clone();
            ui.on_support_tool_clicked(move |x, y, z| run.on_tool_click([x, y, z]));
        }
        run.reload_presets();
        run.push_options(ui);
        {
            let run = run.clone();
            ui.on_support_option_changed(move |code, value| run.on_option(code, value));
        }
        {
            let run = run.clone();
            ui.on_support_preset_picked(move |i| run.on_preset_picked(i));
        }
        {
            let run = run.clone();
            ui.on_support_preset_save(move |name| run.on_preset_save(&name));
        }
        {
            let run = run.clone();
            ui.on_support_preset_delete(move || run.on_preset_delete());
        }
        {
            let weak = Rc::downgrade(&run);
            run.prescan_timer.start(
                slint::TimerMode::Repeated,
                PRESCAN_POLL_INTERVAL,
                move || {
                    if let Some(run) = weak.upgrade() {
                        run.watch_prescan();
                    }
                },
            );
        }
        {
            let run = run.clone();
            ui.on_df_support_requested(move || run.start());
        }
        {
            let run = run.clone();
            ui.on_support_gen_cancel_requested(move || {
                if let Some(handle) = run.handle.borrow().as_ref() {
                    handle.cancel();
                }
                run.drop_waiting_run();
            });
        }
        run
    }

    fn start(self: &Rc<Self>) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if ui.get_loading() || ui.get_support_gen_in_progress() || self.handle.borrow().is_some() {
            return;
        }
        if self.take_ahead(&ui) {
            return;
        }
        {
            let mut prescan = self.prescan.borrow_mut();
            let current = prescan
                .running
                .as_ref()
                .is_some_and(|(_, _, started)| self.last_change.get() <= *started);
            if current {
                // The scene has not moved since the prescan started: wait for
                // it rather than scan the same geometry twice.
                prescan.run_waiting = true;
                ui.set_support_gen_in_progress(true);
                ui.set_support_gen_cancelable(true);
                ui.set_support_gen_progress(0.0);
                ui.set_support_gen_phase(ctr!("DfSupportDialog", "Preparing scans…"));
                return;
            }
            // A prescan of an older scene would not match: free its cores.
            if let Some((job, _, _)) = prescan.running.take() {
                job.cancel();
            }
        }
        self.launch(&ui);
    }

    /// Starts the run, from the last prescan when it still matches.
    fn launch(self: &Rc<Self>, ui: &MainWindow) {
        crate::breadcrumbs::note("supports: run started");
        let Some(scene) = self.document.with_scene(|spatial| spatial.graph().clone()) else {
            ui.set_support_gen_in_progress(false);
            ui.set_support_gen_cancelable(false);
            return;
        };
        let mut request = self.request_for_selection();
        {
            let prescan = self.prescan.borrow();
            // A stale entry would simply not match; there is no point
            // handing it over.
            if let Some((revision, scans)) = &prescan.ready
                && !self.document.slice_geometry_changed(*revision)
            {
                request.prescanned = Some(scans.clone());
            }
        }
        match start_df_supports(&scene, request) {
            Ok(handle) => {
                ui.set_support_gen_in_progress(true);
                ui.set_support_gen_cancelable(true);
                ui.set_support_gen_progress(0.0);
                ui.set_support_gen_phase(slint::SharedString::default());
                *self.handle.borrow_mut() = Some(handle);
                *self.started_on.borrow_mut() = Some(self.document.scene_revision());
                self.start_poll();
            }
            Err(err) => {
                ui.set_support_gen_in_progress(false);
                ui.set_support_gen_cancelable(false);
                let message = err.to_string();
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("DfSupportDialog", "Support generation failed"),
                    context: None,
                    message: message.as_str().into(),
                    details: Default::default(),
                });
            }
        }
    }

    /// Cancels a Generate press that is waiting for the prescan, and the
    /// prescan with it.
    fn drop_waiting_run(&self) {
        let mut prescan = self.prescan.borrow_mut();
        if !prescan.run_waiting {
            return;
        }
        prescan.run_waiting = false;
        if let Some((job, _, _)) = prescan.running.take() {
            job.cancel();
        }
        if let Some(ui) = self.ui.upgrade() {
            ui.set_support_gen_in_progress(false);
            ui.set_support_gen_cancelable(false);
        }
    }

    /// The prescan watcher, every [`PRESCAN_POLL_INTERVAL`]: collects a
    /// finished prescan (starting a run that waited for it), cancels one the
    /// scene has moved on from, and starts one once the scene has been still
    /// for [`PRESCAN_SETTLE`] and holds no matching scans yet, in the Hollow
    /// and Supports tabs or anywhere with the preference on. Idle while a
    /// support run is in flight or a scene is loading.
    fn watch_prescan(self: &Rc<Self>) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let mut prescan = self.prescan.borrow_mut();
        if let Some((job, revision, _)) = &prescan.running
            && let Some(finished) = job.try_poll()
        {
            let revision = *revision;
            prescan.running = None;
            crate::breadcrumbs::note(&format!(
                "supports: prescan finished ok={}",
                finished.is_ok()
            ));
            if let Ok(done) = finished {
                prescan.ready = Some((revision, done.scans));
            }
            if prescan.run_waiting {
                prescan.run_waiting = false;
                drop(prescan);
                self.launch(&ui);
                return;
            }
        }
        let tab_wants = matches!(
            ui.get_workspace_section(),
            WorkspaceSection::Hollow | WorkspaceSection::Supports
        );
        if !tab_wants && !self.bus.snapshot().prescan_supports {
            if !prescan.run_waiting
                && let Some((job, _, _)) = prescan.running.take()
            {
                job.cancel();
            }
            return;
        }
        let last_change = self.last_change.get();
        if let Some((job, _, started)) = &prescan.running
            && last_change > *started
        {
            // The scene moved under it: its scans would not match. A run
            // waiting for it starts on its own instead.
            job.cancel();
            prescan.running = None;
            if prescan.run_waiting {
                prescan.run_waiting = false;
                drop(prescan);
                self.launch(&ui);
                return;
            }
        }
        let busy = self.handle.borrow().is_some()
            || ui.get_loading()
            || ui.get_support_gen_in_progress()
            || ui.get_slicing_in_progress();
        if ui.get_slicing_in_progress()
            && let Some((job, _, _)) = self.ahead.borrow_mut().running.take()
        {
            // A slice is waited on; supports ahead of time are not.
            job.cancel();
        }
        if busy || prescan.running.is_some() || last_change.elapsed() < PRESCAN_SETTLE {
            return;
        }
        if prescan
            .ready
            .as_ref()
            .is_some_and(|(revision, _)| !self.document.slice_geometry_changed(*revision))
        {
            // Scanned: with time to spare, generate ahead.
            let scans = prescan.ready.as_ref().map(|(_, scans)| scans.clone());
            drop(prescan);
            self.generate_ahead(&ui, scans);
            return;
        }
        let Some(scene) = self.document.with_scene(|spatial| spatial.graph().clone()) else {
            return;
        };
        let mut request = self.request_for_selection();
        // Models the last prescan already scanned are taken from it.
        request.prescanned = prescan.ready.as_ref().map(|(_, scans)| scans.clone());
        let job = start_df_prescan(&scene, request);
        crate::breadcrumbs::note("supports: prescan started");
        prescan.running = Some((job, self.document.scene_revision(), Instant::now()));
    }

    /// Keeps the supports generated ahead in step with the scene: drops them
    /// when it moves or the Hollow tab gets busy (its previews come first),
    /// collects a finished run, and starts one once the scene has been still
    /// for [`SPECULATE_SETTLE`] with nothing matching yet.
    fn generate_ahead(&self, ui: &MainWindow, scans: Option<Arc<ScanCache>>) {
        let mut ahead = self.ahead.borrow_mut();
        let hollow_busy = ui.get_hollow_busy();
        if let Some((job, revision, key)) = &ahead.running {
            if hollow_busy || self.document.slice_geometry_changed(*revision) {
                job.cancel();
                ahead.running = None;
            } else if let Some(finished) = job.try_poll() {
                crate::breadcrumbs::note(&format!(
                    "supports: ahead finished ok={}",
                    finished.is_ok()
                ));
                let (revision, key) = (*revision, key.clone());
                ahead.running = None;
                if let Ok(outcome) = finished {
                    ahead.ready = Some((revision, key, outcome));
                }
            }
        }
        // Supports already in the scene are being edited, not awaited.
        if ahead.running.is_some()
            || hollow_busy
            || self.last_change.get().elapsed() < SPECULATE_SETTLE
            || self.support_leaf().is_some()
        {
            return;
        }
        let mut request = self.request_for_selection();
        let key = ahead_key(&request);
        if ahead.ready.as_ref().is_some_and(|(revision, k, _)| {
            *k == key && !self.document.slice_geometry_changed(*revision)
        }) {
            return;
        }
        let Some(scene) = self.document.with_scene(|spatial| spatial.graph().clone()) else {
            return;
        };
        request.prescanned = scans;
        if let Ok(job) = start_df_supports(&scene, request) {
            crate::breadcrumbs::note("supports: ahead started");
            ahead.running = Some((job, self.document.scene_revision(), key));
        }
    }

    /// A Generate press finds the supports generated ahead: pasted at once
    /// when ready for this scene and request, or the run in flight is taken
    /// over (its progress shows). Returns whether it took them.
    fn take_ahead(self: &Rc<Self>, ui: &MainWindow) -> bool {
        let key = ahead_key(&self.request_for_selection());
        let mut ahead = self.ahead.borrow_mut();
        if let Some((revision, k, _)) = &ahead.ready
            && *k == key
            && !self.document.slice_geometry_changed(*revision)
        {
            let (revision, _, outcome) = ahead.ready.take().expect("checked above");
            drop(ahead);
            crate::breadcrumbs::note("supports: generate used the supports made ahead");
            *self.started_on.borrow_mut() = Some(revision);
            self.finish(Ok(outcome));
            return true;
        }
        let matches = ahead.running.as_ref().is_some_and(|(_, revision, k)| {
            *k == key && !self.document.slice_geometry_changed(*revision)
        });
        if !matches {
            return false;
        }
        let (job, revision, _) = ahead.running.take().expect("checked above");
        drop(ahead);
        crate::breadcrumbs::note("supports: generate took over the run made ahead");
        ui.set_support_gen_in_progress(true);
        ui.set_support_gen_cancelable(true);
        ui.set_support_gen_progress(0.0);
        ui.set_support_gen_phase(slint::SharedString::default());
        *self.handle.borrow_mut() = Some(job);
        *self.started_on.borrow_mut() = Some(revision);
        self.start_poll();
        true
    }

    /// The request for the printer and resin in force: the layer height the
    /// slice will use, and the printer's pixel pitch, which sets how far the
    /// contact tips sink into the model. The defaults (0.05 mm, tips at the
    /// surface) when no printer or resin is in force or a profile fails to
    /// load.
    fn request_for_selection(&self) -> DfSupportRequest {
        let mut request = self.request_for_printer();
        request.options = self.options.borrow().clone();
        request
    }

    /// The layer height and pixel pitch of the printer and resin in force.
    fn request_for_printer(&self) -> DfSupportRequest {
        let snapshot = self.bus.snapshot();
        let selection = self
            .document
            .print_selection()
            .in_force(&PrintDefaults::from(&snapshot));
        let (Some(printer), Some(resin)) =
            (selection.printer().in_force(), selection.resin().in_force())
        else {
            return DfSupportRequest::default();
        };
        let Ok((printer, resin)) = load_selected_profiles(printer, resin) else {
            return DfSupportRequest::default();
        };
        let defaults = DfSupportRequest::default();
        let layer_height_mm = resolve_layer_height(&printer, &resin, None)
            .map_or(defaults.layer_height_mm, |h| h.value());
        let pitch = printer.hardware.pixel_size_microns();
        DfSupportRequest {
            layer_height_mm,
            pixel_pitch_mm: Some([pitch.x.value() / 1000.0, pitch.y.value() / 1000.0]),
            ..defaults
        }
    }

    /// Pushes the options and the preset list to the Supports tab.
    fn push_options(&self, ui: &MainWindow) {
        let o = self.options.borrow().clone();
        let presets = self.presets.borrow();
        let names: Vec<slint::SharedString> = presets
            .iter()
            .map(|p| slint::SharedString::from(p.name.as_str()))
            .collect();
        let current = presets
            .iter()
            .position(|p| p.options == o)
            .map_or(-1, |i| i as i32);
        ui.set_support_preset_names(slint::ModelRc::new(slint::VecModel::from(names)));
        ui.set_support_preset_index(current);
        ui.set_support_sizing_index(match o.sizing.as_str() {
            "detail" => 0,
            "anchor" => 2,
            _ => 1,
        });
        ui.set_support_density(o.area_per_support_mm2 as f32);
        ui.set_support_angle(o.overhang_angle_deg as f32);
        ui.set_support_size_scale(o.size_scale as f32);
        ui.set_support_min_island(o.min_island_area_mm2 as f32);
        ui.set_support_coverage(o.coverage_percent as f32);
        ui.set_support_stabilization(o.stabilization);
        ui.set_support_bracing(o.bracing);
        ui.set_support_smooth(o.smooth_shafts);
        ui.set_support_raft_index(match o.raft {
            RaftMode::Solid => 0,
            RaftMode::Lines => 1,
            RaftMode::Off => 2,
        });
        ui.set_support_raft_thickness(o.raft_thickness_mm as f32);
        ui.set_support_raft_wall(o.raft_wall);
    }

    /// Replaces the options, saves them and shows them.
    fn set_options(&self, options: DfSupportOptions) {
        let options = options.clamped();
        if let Some(dir) = support_data_dir()
            && std::fs::create_dir_all(&dir).is_ok()
            && let Ok(text) = toml::to_string_pretty(&options)
        {
            let _ = std::fs::write(dir.join("options.toml"), text);
        }
        *self.options.borrow_mut() = options;
        if let Some(ui) = self.ui.upgrade() {
            self.push_options(&ui);
        }
    }

    /// An option edited in the Supports tab (codes in the order of its
    /// fields: sizing, density, angle, size scale, smallest island,
    /// coverage, stabilization, bracing, smooth, raft bottom, raft
    /// thickness, raft wall).
    fn on_option(&self, code: i32, value: f32) {
        let mut o = self.options.borrow().clone();
        let v = f64::from(value);
        let on = value != 0.0;
        match code {
            0 => {
                o.sizing = ["detail", "structure", "anchor"]
                    .get(value as usize)
                    .unwrap_or(&"structure")
                    .to_string();
            }
            1 => o.area_per_support_mm2 = v,
            2 => o.overhang_angle_deg = v,
            3 => o.size_scale = v,
            4 => o.min_island_area_mm2 = v,
            5 => o.coverage_percent = v,
            6 => o.stabilization = on,
            7 => o.bracing = on,
            8 => o.smooth_shafts = on,
            9 => {
                o.raft = match value as i32 {
                    1 => RaftMode::Lines,
                    2 => RaftMode::Off,
                    _ => RaftMode::Solid,
                };
            }
            10 => o.raft_thickness_mm = v,
            11 => o.raft_wall = on,
            _ => return,
        }
        self.set_options(o);
    }

    fn reload_presets(&self) {
        let mut presets = built_in_presets();
        presets.extend(load_user_presets());
        *self.presets.borrow_mut() = presets;
    }

    fn on_preset_picked(&self, index: i32) {
        let picked = self
            .presets
            .borrow()
            .get(index.max(0) as usize)
            .map(|p| p.options.clone());
        if let Some(options) = picked {
            self.set_options(options);
        }
    }

    /// Saves the current options as a preset of the person.
    fn on_preset_save(&self, name: &str) {
        let Some(stem) = preset_file_stem(name) else {
            return;
        };
        let Some(dir) = support_data_dir().map(|d| d.join("presets")) else {
            return;
        };
        let options = self.options.borrow().clone();
        if std::fs::create_dir_all(&dir).is_ok()
            && let Ok(text) = toml::to_string_pretty(&options)
        {
            let _ = std::fs::write(dir.join(format!("{stem}.toml")), text);
        }
        self.reload_presets();
        if let Some(ui) = self.ui.upgrade() {
            self.push_options(&ui);
        }
    }

    /// Deletes the selected preset, when it is one of the person's own.
    fn on_preset_delete(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let index = ui.get_support_preset_index();
        let target = self
            .presets
            .borrow()
            .get(index.max(0) as usize)
            .filter(|p| index >= 0 && !p.built_in)
            .map(|p| p.name.clone());
        let (Some(name), Some(dir)) = (target, support_data_dir()) else {
            return;
        };
        if let Some(stem) = preset_file_stem(&name) {
            let _ = std::fs::remove_file(dir.join("presets").join(format!("{stem}.toml")));
        }
        self.reload_presets();
        self.push_options(&ui);
    }

    /// The support mesh this driver pasted: its leaf and current mesh.
    fn support_leaf(&self) -> Option<(NodeId, Arc<sc_mesh_core::IndexedMesh>)> {
        self.document
            .with_scene(|spatial| {
                (0..)
                    .map_while(|i| spatial.leaf_id_at_index(i))
                    .find_map(|id| {
                        let mesh = spatial.graph().leaf_mesh_arc(id)?;
                        sc_geom::df_supports::is_support_mesh(mesh).then(|| (id, mesh.clone()))
                    })
            })
            .flatten()
    }

    /// A click in the viewport with the Add (1) or Delete (2) support tool
    /// of the Supports tab, at the world point it hit. The edit redraws the
    /// supports and swaps the support mesh as one undoable edit.
    /// The editable session for the scene as it stands: the last run or hand
    /// edit drawn into it (Undo and Redo land on earlier ones).
    fn current_session(&self) -> Option<usize> {
        self.sessions
            .borrow()
            .iter()
            .rposition(|(_, drawn_on)| !self.document.slice_geometry_changed(*drawn_on))
    }

    /// The cursor moved over the model (`Some`) or off it with a hand tool
    /// armed: the newest point goes to the preview worker; the UI never
    /// waits for it.
    fn on_tool_hover(self: &Rc<Self>, point: Option<[f32; 3]>) {
        let generation = self.hover_generation.get() + 1;
        self.hover_generation.set(generation);
        let ask = point.and_then(|point| {
            let ui = self.ui.upgrade()?;
            let tool = ui.get_support_tool();
            if tool == 0 || self.handle.borrow().is_some() {
                return None;
            }
            let current = self.current_session()?;
            let sessions = self.sessions.borrow();
            let (base, drawn_on) = &sessions[current];
            Some(HoverAsk {
                generation,
                base: base.clone(),
                from: (current, *drawn_on),
                point,
                tool,
            })
        });
        let Some(ask) = ask else {
            self.hover_poll.stop();
            self.hover_shown.set(generation);
            *self.last_ghost.borrow_mut() = None;
            self.ctrl.set_preview_overlay(None);
            return;
        };
        if self.hover_worker.asks.send(ask).is_err() {
            return;
        }
        let weak = Rc::downgrade(self);
        self.hover_poll
            .start(slint::TimerMode::Repeated, HOVER_POLL, move || {
                if let Some(run) = weak.upgrade() {
                    run.take_hover_answer();
                }
            });
    }

    /// Draws the worker's newest answer: the support a click adds (green, or
    /// red where nothing can hold that point) or what it removes (red). While
    /// the cursor moves the answer is for a point it has already left, and is
    /// drawn all the same: the preview follows the cursor a step behind
    /// instead of waiting for it to stop.
    fn take_hover_answer(&self) {
        let answer = {
            let Ok(mut slot) = self.hover_worker.answer.lock() else {
                return;
            };
            match slot.as_ref() {
                Some(a) if a.generation > self.hover_shown.get() => slot.take(),
                _ => return,
            }
        };
        let Some(answer) = answer else {
            return;
        };
        self.hover_shown.set(answer.generation);
        if answer.generation == self.hover_generation.get() {
            self.hover_poll.stop();
        }
        let piece = answer.preview.as_ref().map(|preview| {
            let tint = if preview.valid {
                OverlayTint::Valid
            } else {
                OverlayTint::Invalid
            };
            Arc::new(overlay_of(&preview.mesh, tint))
        });
        *self.last_ghost.borrow_mut() = piece;
        self.show_hover_overlay();
        *self.last_preview.borrow_mut() = Some(answer);
    }

    /// The worker's last ghost, the only hover overlay.
    fn show_hover_overlay(&self) {
        let pieces: Vec<Arc<OverlayMesh>> = self.last_ghost.borrow().iter().cloned().collect();
        self.ctrl
            .set_preview_overlay((!pieces.is_empty()).then(|| pieces.into()));
    }

    /// Has the worker build the removal index of the session now in force,
    /// so the first removal hover on it answers quickly.
    fn warm_worker(&self) {
        let Some(current) = self.current_session() else {
            return;
        };
        let sessions = self.sessions.borrow();
        let (base, drawn_on) = &sessions[current];
        let _ = self.hover_worker.asks.send(HoverAsk {
            generation: 0,
            base: base.clone(),
            from: (current, *drawn_on),
            point: [0.0; 3],
            tool: 0,
        });
    }

    fn on_tool_click(&self, point: [f32; 3]) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let tool = ui.get_support_tool();
        if tool == 0 || self.handle.borrow().is_some() {
            return;
        }
        let notice = |heading: slint::SharedString, message: String| {
            self.dialogs.show_info(move || InfoTexts {
                heading: heading.clone(),
                message: message.as_str().into(),
                details: Default::default(),
            });
        };
        // No borrow of the sessions is held across the edit: the scene events
        // it fires, and the hover preview, read them.
        if self.sessions.borrow().is_empty() {
            notice(
                ctr!("DfSupportDialog", "No supports to edit"),
                ctr!(
                    "DfSupportDialog",
                    "Generate supports first; then add or remove them by hand."
                )
                .to_string(),
            );
            return;
        }
        let Some(current) = self.current_session() else {
            notice(
                ctr!("DfSupportDialog", "Supports out of date"),
                ctr!(
                    "DfSupportDialog",
                    "The scene changed since the supports were made; generate them again to edit them."
                )
                .to_string(),
            );
            return;
        };
        crate::breadcrumbs::note(&format!("supports: click tool={tool}"));
        let p = point.map(f64::from);
        // The preview of this very point already placed it: take that.
        let previewed = self.last_preview.borrow_mut().take().and_then(|answer| {
            let drawn_on = self.sessions.borrow()[current].1;
            (answer.tool == tool && answer.point == point && answer.from == (current, drawn_on))
                .then_some(answer.edited)
                .flatten()
        });
        let reused = previewed.is_some();
        let mut edited = previewed
            .unwrap_or_else(|| DfSupportSession::clone(&self.sessions.borrow()[current].0));
        // The port is DragonFruit's code on hand-picked input: a panic in it
        // is an edit that failed, not a reason to lose the person's work.
        let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if !reused {
                if tool == 1 {
                    edited.add_trunk_at(p)?;
                } else {
                    edited.remove_near(p)?;
                }
            }
            edited.mesh()
        }));
        let mesh = match done {
            Err(_) => {
                ui.set_supports_last_run(ctr!(
                    "DfSupportDialog",
                    "That edit failed inside the support engine; nothing was changed."
                ));
                return;
            }
            Ok(Err(err)) => {
                ui.set_supports_last_run(err.to_string().into());
                return;
            }
            Ok(Ok(Some(mesh))) => mesh,
            Ok(Ok(None)) => {
                ui.set_supports_last_run(ctr!(
                    "DfSupportDialog",
                    "That was the last support; use Undo to bring it back."
                ));
                return;
            }
        };
        crate::breadcrumbs::note(&format!(
            "supports: edit drawn ({} triangles, reused preview {reused})",
            mesh.faces().len()
        ));
        let swap = self.support_leaf().and_then(|(node_id, _)| {
            self.document
                .with_scene(|spatial| leaf_mesh_swap(spatial.graph(), node_id, Arc::new(mesh)))
                .flatten()
        });
        let Some(swap) = swap else {
            ui.set_supports_last_run(ctr!(
                "DfSupportDialog",
                "The support mesh is not in the scene; generate supports again."
            ));
            return;
        };
        match self.document.apply_edit(swap.clone()) {
            Ok(_) => {
                crate::breadcrumbs::note("supports: edit applied");
                self.document.push_edit(swap);
                let roots = edited.state().collections.roots.len();
                let mut sessions = self.sessions.borrow_mut();
                sessions.truncate(current + 1);
                sessions.push((Arc::new(edited), self.document.scene_revision()));
                ui.set_supports_last_run(
                    format!("{roots} support root(s), edited by hand.").into(),
                );
                drop(sessions);
                self.warm_worker();
            }
            Err(err) => ui.set_supports_last_run(format!("The edit was refused: {err:?}").into()),
        }
    }

    fn start_poll(self: &Rc<Self>) {
        let run = self.clone();
        let timer_weak = Rc::downgrade(&self.poll_timer);
        self.poll_timer.start(
            slint::TimerMode::Repeated,
            PROGRESS_POLL_INTERVAL,
            move || {
                let stop = || {
                    if let Some(timer) = timer_weak.upgrade() {
                        timer.stop();
                    }
                };
                let Some(ui) = run.ui.upgrade() else {
                    stop();
                    return;
                };
                let finished = {
                    let slot = run.handle.borrow();
                    let Some(handle) = slot.as_ref() else {
                        stop();
                        return;
                    };
                    let progress = handle.progress();
                    ui.set_support_gen_progress(progress.fraction());
                    ui.set_support_gen_phase(progress.phase.into());
                    handle.try_poll()
                };
                if let Some(outcome) = finished {
                    stop();
                    run.handle.borrow_mut().take();
                    ui.set_support_gen_in_progress(false);
                    ui.set_support_gen_cancelable(false);
                    run.finish(outcome);
                }
            },
        );
    }

    fn finish(&self, outcome: Result<DfSupportOutcome, JobError>) {
        crate::breadcrumbs::note(&format!("supports: run finished ok={}", outcome.is_ok()));
        let started_on = self.started_on.borrow_mut().take();
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(JobError::Cancelled) => return,
            Err(err) => {
                let message = err.to_string();
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("DfSupportDialog", "Support generation failed"),
                    context: None,
                    message: message.as_str().into(),
                    details: Default::default(),
                });
                return;
            }
        };
        if started_on.is_none_or(|revision| self.document.slice_geometry_changed(revision)) {
            self.dialogs.show_info(|| InfoTexts {
                heading: ctr!("DfSupportDialog", "Supports discarded"),
                message: ctr!(
                    "DfSupportDialog",
                    "The scene changed while supports were being generated; nothing was added."
                ),
                details: Default::default(),
            });
            return;
        }
        let summary = summary_message(&outcome);
        if let Some(ui) = self.ui.upgrade() {
            let seconds = outcome.timings.total.as_secs_f64();
            ui.set_supports_last_run(format!("{summary} {seconds:.1} s.").into());
        }
        let mut details = details_message(&outcome);
        // A release build has no console (Windows GUI subsystem): the full
        // breakdown goes to a log file instead, named in the details.
        if let Some(path) = append_timing_log(&summary, &outcome.timings.report()) {
            details.push_str(&format!("\n\nFull timings: {}", path.display()));
        }
        let Some(mesh) = outcome.mesh else {
            self.dialogs.show_info(move || InfoTexts {
                heading: ctr!("DfSupportDialog", "No supports needed"),
                message: summary.as_str().into(),
                details: details.as_str().into(),
            });
            return;
        };
        let fragment = SceneFragment::from_leaf_snapshot(Arc::new(mesh), Transform3D::Identity);
        let session = outcome.session;
        let pasted = self.document.paste_fragment(&fragment, [0.0, 0.0, 0.0]);
        if pasted.is_ok() {
            *self.sessions.borrow_mut() = vec![(Arc::new(session), self.document.scene_revision())];
            self.warm_worker();
        }
        if let Err(error) = pasted {
            let details = error.to_string();
            self.dialogs.show_error(move || ErrorTexts {
                heading: ctr!("DfSupportDialog", "Support generation failed"),
                context: None,
                message: ctr!(
                    "DfSupportDialog",
                    "The scene rejected the generated supports; nothing was changed."
                ),
                details: details.as_str().into(),
            });
            return;
        }
        self.dialogs.show_info(move || InfoTexts {
            heading: ctr!("DfSupportDialog", "Supports generated"),
            message: summary.as_str().into(),
            details: details.as_str().into(),
        });
    }
}

/// One line: models, islands, roots and triangles.
fn summary_message(outcome: &DfSupportOutcome) -> String {
    format!(
        "{} model(s), {} island(s), {} support root(s), {} triangle(s).",
        outcome.models,
        outcome.islands,
        outcome.support.collections.roots.len(),
        outcome.triangles
    )
}

/// Appends one run's summary and full timing report to
/// `<local data dir>/logs/df-supports-timings.log`, returning its path, or
/// `None` when there is no data directory or the write fails (the log is a
/// diagnostic: a failure never stops the run).
fn append_timing_log(summary: &str, report: &str) -> Option<std::path::PathBuf> {
    use std::io::Write;
    // Written only while the diagnostic log is on.
    if !crate::breadcrumbs::enabled() {
        return None;
    }
    let path = crate::breadcrumbs::logs_dir()?.join("df-supports-timings.log");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    writeln!(file, "=== run at unix {stamp}: {summary}\n{report}").ok()?;
    Some(path)
}

/// The placed supports by type, then where the time went: the stages, the
/// plan's phases and the costliest inner measurements.
fn details_message(outcome: &DfSupportOutcome) -> String {
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    let mut lines: Vec<String> = outcome
        .placed
        .iter()
        .map(|(kind, count)| format!("{kind}: {count}"))
        .collect();
    let t = &outcome.timings;
    lines.push(String::new());
    lines.push(format!("Time: {:.0} ms", ms(t.total)));
    if t.entries.iter().any(|e| e.label == "scan:prescanned") {
        lines.push("  scans prepared in the background".to_owned());
    }
    for prefix in ["stage:", "auto:"] {
        for e in t.by_cost(prefix) {
            lines.push(format!("  {:<24} {:>8.0} ms", e.label, ms(e.total)));
        }
    }
    let mut inner: Vec<_> = t
        .entries
        .iter()
        .filter(|e| !e.label.starts_with("stage:") && !e.label.starts_with("auto:"))
        .collect();
    inner.sort_by_key(|e| std::cmp::Reverse(e.total));
    for e in inner.iter().take(6) {
        lines.push(format!(
            "  {:<24} {:>8.0} ms ({}x)",
            e.label,
            ms(e.total),
            e.count
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod hand_edit_tests {
    use std::path::PathBuf;

    use sc_mesh_core::transform::AffineTransform3D;
    use sc_scene::Scene;
    use sc_scene::node::LeafPayload;

    use super::*;
    use crate::preferences_bus::PreferencesSnapshot;
    use crate::test_support::slint_thread::on_fresh_slint_thread;

    /// The click path end to end, through the window callbacks' handler: a
    /// generated support set, then an Add click and a Remove click under
    /// the model. A click used to panic (a sessions borrow held across the
    /// lookup) and close the app.
    /// With the scene still, the supports are generated ahead; Generate then
    /// pastes them without running again.
    #[test]
    fn supports_generated_ahead_are_pasted_on_generate() {
        let pasted = on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let mut graph = Scene::new();
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([20.0, 20.0, 8.0])),
                    LeafPayload::Mesh(Arc::new(sc_mesh_fixtures::unit_cube_mesh())),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph, PathBuf::from("cube.stl"));
            let run = DfSupportRun::install(&window, bus, document, ctrl.dialogs(), ctrl.clone());
            run.last_change.set(Instant::now() - SPECULATE_SETTLE);
            let waited = Instant::now();
            while run.ahead.borrow().ready.is_none() && waited.elapsed() < Duration::from_secs(20) {
                run.generate_ahead(&window, None);
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(run.ahead.borrow().ready.is_some(), "generated ahead");
            run.start();
            assert!(run.handle.borrow().is_none(), "no second run");
            run.support_leaf().is_some()
        });
        assert!(pasted);
    }

    #[test]
    fn hand_clicks_edit_the_supports_without_a_panic() {
        let (after_add, after_remove, edits) = on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let mut graph = Scene::new();
            let cube = sc_mesh_fixtures::unit_cube_mesh();
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([20.0, 20.0, 8.0])),
                    LeafPayload::Mesh(Arc::new(cube)),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph.clone(), PathBuf::from("cube.stl"));
            let run =
                DfSupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            let outcome = start_df_supports(&graph, DfSupportRequest::default())
                .unwrap()
                .join();
            *run.started_on.borrow_mut() = Some(document.scene_revision());
            run.finish(outcome);
            let before = document.scene_revision();

            window.set_support_tool(1);
            // The hover preview comes from the worker; the click on the same
            // point then takes the placement it computed.
            run.on_tool_hover(Some([20.2, 20.2, 7.5]));
            let waited = Instant::now();
            while run.last_preview.borrow().is_none() && waited.elapsed() < Duration::from_secs(10)
            {
                std::thread::sleep(Duration::from_millis(5));
                run.take_hover_answer();
            }
            assert!(
                run.last_preview.borrow().as_ref().is_some_and(|a| a
                    .preview
                    .as_ref()
                    .is_some_and(|p| p.valid)
                    && a.edited.is_some()),
                "a green preview with its placement"
            );
            run.on_tool_click([20.2, 20.2, 7.5]);
            let after_add = window.get_supports_last_run().to_string();
            window.set_support_tool(2);
            run.on_tool_click([20.2, 20.2, 7.4]);
            let after_remove = window.get_supports_last_run().to_string();
            (after_add, after_remove, document.scene_revision() != before)
        });
        assert!(after_add.contains("edited by hand"), "{after_add}");
        assert!(after_remove.contains("edited by hand"), "{after_remove}");
        assert!(edits);
    }

    /// The GUI click path on a real model, not a gate: `SC_STRESS_STL=...
    /// cargo test --release -- --ignored --nocapture gui_clicks_on_a_real_model`.
    #[test]
    #[ignore = "needs SC_STRESS_STL"]
    fn gui_clicks_on_a_real_model() {
        let Ok(path) = std::env::var("SC_STRESS_STL") else {
            return;
        };
        let log = on_fresh_slint_thread(move || {
            let mut log = Vec::new();
            let bytes = std::fs::read(&path).unwrap();
            let (loaded, _) =
                sc_mesh_formats::stl::read_stl(&mut std::io::Cursor::new(bytes)).unwrap();
            let leaf = loaded
                .roots()
                .iter()
                .copied()
                .find(|id| loaded.leaf_mesh_arc(*id).is_some())
                .unwrap();
            let mesh = loaded.leaf_mesh_arc(leaf).unwrap().clone();
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for v in mesh.vertices() {
                for k in 0..3 {
                    lo[k] = lo[k].min(v.as_array()[k]);
                    hi[k] = hi[k].max(v.as_array()[k]);
                }
            }
            let shift = [
                60.0 - f64::from(lo[0] + hi[0]) / 2.0,
                40.0 - f64::from(lo[1] + hi[1]) / 2.0,
                5.0 - f64::from(lo[2]),
            ];
            let points: Vec<[f32; 3]> = mesh
                .faces()
                .iter()
                .step_by(997)
                .filter_map(|f| {
                    let [a, b, c] = f
                        .vertices
                        .map(|v| *mesh.vertices()[u32::from(v) as usize].as_array());
                    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                    let w = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                    let nz = u[0] * w[1] - u[1] * w[0];
                    let len = ((u[1] * w[2] - u[2] * w[1]).powi(2)
                        + (u[2] * w[0] - u[0] * w[2]).powi(2)
                        + nz * nz)
                        .sqrt();
                    (len > 1e-9 && nz / len < -0.5).then(|| {
                        std::array::from_fn(|k| (a[k] + b[k] + c[k]) / 3.0 + shift[k] as f32)
                    })
                })
                .take(12)
                .collect();
            let window = MainWindow::new().unwrap();
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let mut graph = Scene::new();
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation(shift)),
                    LeafPayload::Mesh(mesh),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            document.install_graph(graph.clone(), PathBuf::from("model.stl"));
            let run =
                DfSupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            let outcome = start_df_supports(&graph, DfSupportRequest::default())
                .unwrap()
                .join();
            *run.started_on.borrow_mut() = Some(document.scene_revision());
            run.finish(outcome);
            log.push(format!("generated: {}", window.get_supports_last_run()));
            for (i, p) in points.iter().enumerate() {
                window.set_support_tool(if i % 3 == 2 { 2 } else { 1 });
                let t = Instant::now();
                run.on_tool_hover(Some(*p));
                while run
                    .last_preview
                    .borrow()
                    .as_ref()
                    .is_none_or(|a| a.point != *p)
                    && t.elapsed() < Duration::from_secs(30)
                {
                    std::thread::sleep(Duration::from_millis(2));
                    run.take_hover_answer();
                }
                let hover_ms = t.elapsed().as_millis();
                let t = Instant::now();
                run.on_tool_click(*p);
                log.push(format!(
                    "{p:?} tool {}: hover {hover_ms} ms, click {} ms: {}",
                    window.get_support_tool(),
                    t.elapsed().as_millis(),
                    window.get_supports_last_run()
                ));
            }
            log
        });
        for line in log {
            println!("{line}");
        }
    }
}
