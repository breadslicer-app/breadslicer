/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The Orient tool (#182): auto-orientation in the Transform tab.
//!
//! The tool (the rail's Orient button, `O`, or Auto-orient on the ROTATE
//! card) finds the best way up of the selected model for a resin printer
//! ([`sc_orient`]) and offers three poses with their numbers: support tips,
//! peak peel force, height. Quick runs the staged search; Full searches a
//! denser sphere and judges more finalists. Both run on a worker with
//! progress and Cancel; the UI never waits.
//!
//! The model's analysis (normals, histogram, hull, simplified copy,
//! adjacency) is built when the tool opens and kept: Quick, Full and a
//! change of objective reuse it. A result worked out for a scene that has
//! changed since is dropped.
//!
//! Hovering an option shows it on the model, uncommitted, like a gizmo drag
//! ([`SceneDocument::set_live_preview`]: Undo, Redo and a save put the pose
//! back first, and autosave waits). Apply records one transform edit, so one
//! Undo takes it back; the supports follow the usual rules
//! ([`crate::support_follow`]): a turn about the vertical takes them along, a
//! tilt makes them again. When the current pose is already the best, the
//! tool says so and moves nothing.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use sc_geom::df_supports::is_support_mesh;
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_orient::{
    Cancelled, Candidate, Note, OrientAnalysis, OrientContext, Preset, SearchMode, SearchOutcome,
    Stage, kept_on_plate, turned_about_footprint,
};
use sc_scene::NodeId;
use sc_scene_edit::SceneEdit;
use slint::ComponentHandle;

use crate::df_support_run::DfSupportRun;
use crate::error_dialog::{DialogManager, ErrorTexts};
use crate::i18n::ctr;
use crate::mesh_loader::panic_message;
use crate::scene_document::{
    EditNotApplied, SceneDocument, SceneEvent, SceneRevision, SceneSubscription,
};
use crate::viewport::ViewportController;
use crate::{MainWindow, OrientOption};

/// What the tool's settings set, beside the mode of each search.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct OrientSettings {
    pub(crate) preset: Preset,
    /// The most an option may tilt from the current pose, degrees; 180
    /// allows any pose.
    pub(crate) max_tilt_deg: f64,
    pub(crate) align_to_plate: bool,
    /// Factors over the preset's weights: support tips and their length,
    /// peel force, height.
    pub(crate) weight_supports: f64,
    pub(crate) weight_force: f64,
    pub(crate) weight_height: f64,
}

impl Default for OrientSettings {
    fn default() -> Self {
        Self {
            preset: Preset::Balanced,
            max_tilt_deg: 180.0,
            align_to_plate: false,
            weight_supports: 1.0,
            weight_force: 1.0,
            weight_height: 1.0,
        }
    }
}

impl OrientSettings {
    /// The search's context: the support engine's overhang angle and area
    /// per tip, the printer's build volume, and these settings.
    pub(crate) fn context(
        &self,
        mode: SearchMode,
        overhang_angle_deg: f64,
        area_per_tip_mm2: f64,
        build_volume_mm: Option<[f64; 3]>,
    ) -> OrientContext {
        let mut weights = self.preset.weights();
        weights.tip *= self.weight_supports;
        weights.island *= self.weight_supports;
        weights.support_length_per_mm *= self.weight_supports;
        weights.peel_per_n *= self.weight_force;
        weights.area_jump_per_n *= self.weight_force;
        weights.height_per_mm *= self.weight_height;
        OrientContext {
            overhang_angle_deg,
            area_per_tip_mm2,
            build_volume_mm,
            weights,
            mode,
            max_tilt_deg: (self.max_tilt_deg < 180.0).then_some(self.max_tilt_deg),
            align_to_plate: self.align_to_plate,
            ..OrientContext::new(self.preset)
        }
    }
}

const PRESETS: [Preset; 4] = [
    Preset::Balanced,
    Preset::FewerSupports,
    Preset::LowerForce,
    Preset::Faster,
];

/// The model a search is for, as the scene had it when the search started.
#[derive(Clone)]
pub(crate) struct Target {
    leaf: NodeId,
    mesh: Arc<IndexedMesh>,
    local: Transform3D,
    world: AffineTransform3D,
    parent_inverse: Transform3D,
    revision: SceneRevision,
    /// The plate, its corners in world X/Y: an option that would reach past
    /// an edge is shifted back onto it.
    plate: Option<([f64; 2], [f64; 2])>,
}

impl Target {
    /// The selected model; `None` without one, or when a support set is
    /// selected.
    pub(crate) fn selected(document: &SceneDocument) -> Option<Self> {
        let leaf = document.selection_primary_node_id()?;
        let revision = document.scene_revision();
        document
            .with_scene(|spatial| {
                let query = spatial.leaf(leaf)?;
                if is_support_mesh(query.mesh()) {
                    return None;
                }
                Some(Target {
                    leaf,
                    mesh: spatial.graph().leaf_mesh_arc(leaf)?.clone(),
                    local: query.local_transform(),
                    world: affine(&query.world_transform()),
                    parent_inverse: query.parent_world().inverse().ok()?,
                    revision,
                    plate: None,
                })
            })
            .flatten()
    }

    fn analysis_key(&self) -> AnalysisKey {
        AnalysisKey {
            leaf: self.leaf,
            mesh: self.mesh.clone(),
            linear: *self.world.linear(),
        }
    }

    /// The leaf's local transform that puts the model at `world`.
    fn local_for(&self, world: AffineTransform3D) -> Transform3D {
        self.parent_inverse.compose(&Transform3D::Affine(world))
    }
}

fn affine(t: &Transform3D) -> AffineTransform3D {
    match t {
        Transform3D::Identity => AffineTransform3D::identity(),
        Transform3D::Affine(a) => *a,
    }
}

/// What an analysis was built from: the leaf, its mesh, and the rotation
/// and scale of its pose. Moving it keeps the analysis; turning it does not.
#[derive(Clone)]
struct AnalysisKey {
    leaf: NodeId,
    mesh: Arc<IndexedMesh>,
    linear: [[f64; 3]; 3],
}

impl PartialEq for AnalysisKey {
    fn eq(&self, other: &Self) -> bool {
        self.leaf == other.leaf
            && Arc::ptr_eq(&self.mesh, &other.mesh)
            && self.linear == other.linear
    }
}

/// A search's options, with the world pose each one puts the model in.
pub(crate) struct Found {
    pub(crate) outcome: SearchOutcome,
    pub(crate) worlds: Vec<AffineTransform3D>,
}

/// What a worker hands back.
pub(crate) struct Worked {
    pub(crate) analysis: Result<Arc<OrientAnalysis>, Cancelled>,
    /// `None` for a run that only prepared the analysis.
    pub(crate) found: Option<Result<Found, Cancelled>>,
}

/// The progress steps a worker reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    Preparing,
    Searching,
    Refining,
    Judging,
}

/// A worker's job: builds the analysis unless `cached` has it, then
/// searches with `ctx` (when given) and works out each option's pose: turned
/// about the centre of its footprint, the lift kept, and kept on the plate.
pub(crate) fn work(
    target: &Target,
    cached: Option<Arc<OrientAnalysis>>,
    ctx: Option<OrientContext>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Step),
) -> Worked {
    let analysis = match cached {
        Some(analysis) => Ok(analysis),
        None => {
            progress(Step::Preparing);
            OrientAnalysis::new(&target.mesh, &target.world, None, cancel).map(Arc::new)
        }
    };
    let found = match (&analysis, ctx) {
        (Ok(analysis), Some(ctx)) => {
            progress(Step::Searching);
            Some(
                analysis
                    .search(&ctx, cancel, &mut |stage, _| {
                        progress(match stage {
                            Stage::Coarse => Step::Refining,
                            Stage::Refined => Step::Judging,
                        })
                    })
                    .map(|outcome| {
                        let worlds = outcome
                            .options
                            .iter()
                            .map(|c| {
                                let turned = turned_about_footprint(
                                    &target.mesh,
                                    &target.world,
                                    &c.rotation,
                                );
                                match target.plate {
                                    Some((lo, hi)) => {
                                        kept_on_plate(&target.mesh, &target.world, &turned, lo, hi)
                                    }
                                    None => turned,
                                }
                            })
                            .collect();
                        Found { outcome, worlds }
                    }),
            )
        }
        (Ok(_), None) => None,
        (Err(Cancelled), _) => Some(Err(Cancelled)),
    };
    Worked { analysis, found }
}

/// A run in flight.
struct Job {
    id: u64,
    target: Target,
    /// `None`: it only prepares the analysis.
    mode: Option<SearchMode>,
    cancel: Arc<AtomicBool>,
    started: Instant,
}

type Slot = Arc<Mutex<Option<(u64, Result<Worked, String>)>>>;

/// The options on show, for the model and the scene they were found for.
struct Shown {
    leaf: NodeId,
    revision: SceneRevision,
    /// The model's local transform when the search started: what Apply
    /// records as `before`, and where a preview goes back to.
    before: Transform3D,
    /// Per option, the local transform that takes the model there.
    afters: Vec<Transform3D>,
}

pub(crate) struct OrientRun {
    me: Weak<Self>,
    ui: slint::Weak<MainWindow>,
    document: Rc<SceneDocument>,
    dialogs: Rc<DialogManager>,
    ctrl: Rc<ViewportController>,
    supports: Rc<DfSupportRun>,
    open: Cell<bool>,
    settings: Cell<OrientSettings>,
    analysis: RefCell<Option<(AnalysisKey, Arc<OrientAnalysis>)>>,
    job: RefCell<Option<Job>>,
    next_id: Cell<u64>,
    /// The run whose progress the card shows.
    live_id: Arc<AtomicU64>,
    slot: Slot,
    /// A search asked for while the analysis was still being built.
    queued: Cell<Option<SearchMode>>,
    /// The last search's mode: a change of objective runs it again.
    last_mode: Cell<Option<SearchMode>>,
    shown: RefCell<Option<Shown>>,
    selected: Cell<i32>,
    /// The option shown on the model now.
    previewing: Cell<Option<usize>>,
    _scene_events: SceneSubscription,
}

impl OrientRun {
    /// Wires the Orient tool; the caller keeps the driver alive for the
    /// window's lifetime.
    pub(crate) fn install(
        ui: &MainWindow,
        document: Rc<SceneDocument>,
        dialogs: Rc<DialogManager>,
        ctrl: Rc<ViewportController>,
        supports: Rc<DfSupportRun>,
    ) -> Rc<Self> {
        let run = Rc::new_cyclic(|me: &Weak<Self>| {
            let weak = me.clone();
            let scene_events = document.subscribe(move |event| {
                if matches!(
                    event,
                    SceneEvent::Installed
                        | SceneEvent::Cleared
                        | SceneEvent::SelectionChanged
                        | SceneEvent::UndoStackChanged
                        | SceneEvent::Restructured
                ) {
                    // Out of the notification: the check reads the scene.
                    let weak = weak.clone();
                    slint::Timer::single_shot(Duration::ZERO, move || {
                        if let Some(run) = weak.upgrade() {
                            run.on_scene_changed();
                        }
                    });
                }
            });
            Self {
                me: me.clone(),
                ui: ui.as_weak(),
                document,
                dialogs,
                ctrl,
                supports,
                open: Cell::new(false),
                settings: Cell::new(OrientSettings::default()),
                analysis: RefCell::new(None),
                job: RefCell::new(None),
                next_id: Cell::new(1),
                live_id: Arc::new(AtomicU64::new(0)),
                slot: Arc::new(Mutex::new(None)),
                queued: Cell::new(None),
                last_mode: Cell::new(None),
                shown: RefCell::new(None),
                selected: Cell::new(-1),
                previewing: Cell::new(None),
                _scene_events: scene_events,
            }
        });
        run.push_settings(ui);
        macro_rules! wire {
            ($on:ident, |$run:ident $(, $arg:ident)*| $body:expr) => {{
                let $run = run.clone();
                ui.$on(move |$($arg),*| $body);
            }};
        }
        wire!(on_orient_toggle_requested, |r| r.toggle());
        wire!(on_orient_close_requested, |r| r.close());
        wire!(on_orient_search, |r, mode| r.search(if mode == 1 {
            SearchMode::Full
        } else {
            SearchMode::Quick
        }));
        wire!(on_orient_cancel, |r| r.cancel());
        wire!(on_orient_finished, |r| r.finish());
        wire!(on_orient_option_changed, |r, code, value| r
            .on_option(code, value));
        wire!(on_orient_option_hovered, |r, i, on| r.hover(i, on));
        wire!(on_orient_option_picked, |r, i| r.pick(i));
        wire!(on_orient_apply, |r| {
            r.apply();
        });
        run
    }

    fn push_settings(&self, ui: &MainWindow) {
        let s = self.settings.get();
        ui.set_orient_preset(PRESETS.iter().position(|p| *p == s.preset).unwrap_or(0) as i32);
        ui.set_orient_max_tilt(s.max_tilt_deg as f32);
        ui.set_orient_align(s.align_to_plate);
        ui.set_orient_weight_supports((s.weight_supports * 100.0) as f32);
        ui.set_orient_weight_force((s.weight_force * 100.0) as f32);
        ui.set_orient_weight_height((s.weight_height * 100.0) as f32);
    }

    fn on_option(&self, code: i32, value: f32) {
        let mut s = self.settings.get();
        let value = f64::from(value);
        let factor = |v: f64| (v / 100.0).clamp(0.0, 5.0);
        match code {
            0 => s.preset = PRESETS[(value as usize).min(PRESETS.len() - 1)],
            1 => s.max_tilt_deg = value.clamp(0.0, 180.0),
            2 => s.align_to_plate = value != 0.0,
            3 => s.weight_supports = factor(value),
            4 => s.weight_force = factor(value),
            5 => s.weight_height = factor(value),
            _ => return,
        }
        let changed = s != self.settings.get();
        self.settings.set(s);
        if let Some(ui) = self.ui.upgrade() {
            self.push_settings(&ui);
        }
        // A new objective runs the last search again, on the kept analysis.
        if changed
            && let Some(mode) = self.last_mode.get()
            && self.shown.borrow().is_some()
        {
            self.search(mode);
        }
    }

    /// The rail button, `O` and Auto-orient: opens the tool, or closes it.
    pub(crate) fn toggle(&self) {
        if self.open.get() {
            self.close();
        } else {
            self.open_tool();
        }
    }

    fn open_tool(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let Some(target) = Target::selected(&self.document) else {
            return;
        };
        self.open.set(true);
        ui.set_orient_open(true);
        self.set_status(String::new());
        ui.set_orient_offer_arrange(false);
        self.refresh_warning();
        // The analysis is built now, while the person reads the card: the
        // first search then only searches.
        if self.job.borrow().is_none() && !self.has_analysis(&target) {
            self.start(target, None);
        }
    }

    /// Closes the tool: a pose an option shows goes back, a search in
    /// flight stops. The analysis is kept.
    pub(crate) fn close(&self) {
        self.end_preview();
        if !self.open.get() {
            return;
        }
        self.open.set(false);
        self.cancel();
        self.queued.set(None);
        self.clear_shown();
        if let Some(ui) = self.ui.upgrade() {
            ui.set_orient_open(false);
            ui.set_orient_status("".into());
            ui.set_orient_warning("".into());
            ui.set_orient_offer_arrange(false);
        }
    }

    fn has_analysis(&self, target: &Target) -> bool {
        self.analysis
            .borrow()
            .as_ref()
            .is_some_and(|(key, _)| *key == target.analysis_key())
    }

    fn editable(&self) -> bool {
        self.ui.upgrade().is_some_and(|ui| {
            !ui.get_loading()
                && !ui.get_slicing_in_progress()
                && !ui.get_repairing_in_progress()
                && !ui.get_arranging()
        }) && self.ctrl.allows_layout_tools()
    }

    /// Quick or Full on the selected model.
    pub(crate) fn search(&self, mode: SearchMode) {
        self.end_preview();
        if !self.open.get() || !self.editable() {
            return;
        }
        let Some(target) = Target::selected(&self.document) else {
            return;
        };
        self.last_mode.set(Some(mode));
        self.clear_shown();
        if let Some(ui) = self.ui.upgrade() {
            ui.set_orient_offer_arrange(false);
        }
        self.set_status(String::new());
        // The analysis being built for this model is waited for, not built
        // again.
        let preparing = self.job.borrow().as_ref().is_some_and(|job| {
            job.mode.is_none() && job.target.analysis_key() == target.analysis_key()
        });
        if preparing {
            self.queued.set(Some(mode));
            self.show_running(Step::Preparing);
            return;
        }
        self.cancel();
        self.start(target, Some(mode));
    }

    fn start(&self, mut target: Target, mode: Option<SearchMode>) {
        let volume = self.ctrl.build_volume();
        let (lo, hi) = (volume.envelope_min_mm(), volume.envelope_max_mm());
        target.plate = Some((
            [f64::from(lo.x), f64::from(lo.y)],
            [f64::from(hi.x), f64::from(hi.y)],
        ));
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let cached = self
            .analysis
            .borrow()
            .as_ref()
            .filter(|(key, _)| *key == target.analysis_key())
            .map(|(_, a)| a.clone());
        let ctx = mode.map(|mode| self.context(mode));
        let cancel = Arc::new(AtomicBool::new(false));
        *self.job.borrow_mut() = Some(Job {
            id,
            target: target.clone(),
            mode,
            cancel: cancel.clone(),
            started: Instant::now(),
        });
        self.live_id.store(id, Ordering::Relaxed);
        if mode.is_some() {
            self.show_running(if cached.is_some() {
                Step::Searching
            } else {
                Step::Preparing
            });
        }
        crate::breadcrumbs::note(&format!(
            "orient: start {mode:?}, cached {}",
            cached.is_some()
        ));
        let slot = self.slot.clone();
        let live = self.live_id.clone();
        let ui_weak = self.ui.clone();
        let searching = mode.is_some();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                work(&target, cached, ctx, &cancel, &mut |step| {
                    if !searching {
                        return;
                    }
                    let live = live.clone();
                    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                        if live.load(Ordering::Relaxed) == id {
                            show_step(&ui, step);
                        }
                    });
                })
            }))
            .map_err(|payload| panic_message(&payload));
            *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some((id, result));
            let _ = ui_weak.upgrade_in_event_loop(|ui| ui.invoke_orient_finished());
        });
    }

    fn context(&self, mode: SearchMode) -> OrientContext {
        let (angle, area) = self.supports.overhang_terms();
        let volume = self.ctrl.build_volume();
        let (lo, hi) = (volume.envelope_min_mm(), volume.envelope_max_mm());
        let size = [
            f64::from(hi.x - lo.x),
            f64::from(hi.y - lo.y),
            f64::from(hi.z - lo.z),
        ];
        let volume = size.iter().all(|s| *s > 0.0).then_some(size);
        self.settings.get().context(mode, angle, area, volume)
    }

    fn show_running(&self, step: Step) {
        if let Some(ui) = self.ui.upgrade() {
            ui.set_orient_running(true);
            show_step(&ui, step);
        }
    }

    /// Stops the run in flight; its result is dropped.
    pub(crate) fn cancel(&self) {
        if let Some(job) = self.job.borrow().as_ref() {
            job.cancel.store(true, Ordering::Relaxed);
        }
        self.queued.set(None);
    }

    /// Takes a worker's result on the UI thread.
    fn finish(&self) {
        let taken = self
            .slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some((id, result)) = taken else {
            return;
        };
        let job = {
            let mut slot = self.job.borrow_mut();
            if slot.as_ref().is_none_or(|job| job.id != id) {
                // A run that was replaced: its work is dropped.
                return;
            }
            slot.take().expect("checked above")
        };
        let worked = match result {
            Ok(worked) => worked,
            Err(panic) => {
                self.show_idle();
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("OrientDialog", "Auto-orient failed"),
                    context: None,
                    message: ctr!(
                        "OrientDialog",
                        "The search stopped unexpectedly. The model was not moved."
                    ),
                    details: panic.as_str().into(),
                });
                return;
            }
        };
        if let Ok(analysis) = &worked.analysis {
            crate::breadcrumbs::note(&format!(
                "orient: analysis of {} triangles in {:.0} ms",
                analysis.triangle_counts().0,
                analysis.prepare_ms()
            ));
            *self.analysis.borrow_mut() = Some((job.target.analysis_key(), analysis.clone()));
        }
        let Some(mode) = job.mode else {
            // The analysis alone: a search asked for meanwhile goes now.
            match self.queued.take() {
                Some(mode) if self.open.get() => self.search(mode),
                _ => self.show_idle(),
            }
            return;
        };
        self.show_idle();
        let found = match worked.found {
            Some(Ok(found)) => found,
            Some(Err(Cancelled)) | None => {
                self.set_status(
                    ctr!("Inspector", "Search cancelled. The model was not moved.").to_string(),
                );
                return;
            }
        };
        let seconds = job.started.elapsed().as_secs_f64();
        let t = &found.outcome.timings;
        crate::breadcrumbs::note(&format!(
            "orient: {mode:?} in {:.0} ms (stage 1 {:.0}, stage 2 {:.0}, refine {:.0}, verdict {:.0}), keep {}",
            seconds * 1e3,
            t.stage1_ms,
            t.stage2_ms,
            t.refine_ms,
            t.verdict_ms,
            found.outcome.keep_current
        ));
        self.land(job.target, mode, found, seconds);
    }

    /// Shows a search's options, when the scene is still the one they were
    /// found for.
    fn land(&self, target: Target, mode: SearchMode, found: Found, seconds: f64) {
        let still = self.document.scene_revision() == target.revision
            && self.document.selection_primary_node_id() == Some(target.leaf)
            && self.local_of(target.leaf).as_ref() == Some(&target.local);
        if !still || !self.open.get() {
            self.set_status(
                ctr!(
                    "Inspector",
                    "The scene changed while searching; the result was dropped. Search again."
                )
                .to_string(),
            );
            return;
        }
        let outcome = &found.outcome;
        let afters: Vec<Transform3D> = found.worlds.iter().map(|w| target.local_for(*w)).collect();
        *self.shown.borrow_mut() = Some(Shown {
            leaf: target.leaf,
            revision: target.revision,
            before: target.local.clone(),
            afters,
        });
        self.selected.set(if outcome.keep_current { -1 } else { 0 });
        let time = format!("{seconds:.1}");
        let status = if outcome.keep_current {
            ctr!(
                "Inspector",
                "Your orientation is already the best I found ({} s). Nothing was moved.",
                time
            )
        } else {
            ctr!(
                "Inspector",
                "Searched in {} s. Hover an option to see it.",
                time
            )
        };
        self.set_status(status.to_string());
        if let Some(ui) = self.ui.upgrade() {
            ui.set_orient_options(slint::ModelRc::new(slint::VecModel::from(option_rows(
                outcome, mode,
            ))));
            ui.set_orient_selected(self.selected.get());
            ui.set_orient_current(current_row(&outcome.current).into());
        }
        self.refresh_warning_with(Some(outcome));
    }

    fn local_of(&self, leaf: NodeId) -> Option<Transform3D> {
        self.document
            .with_scene(|spatial| spatial.leaf(leaf).map(|q| q.local_transform()))
            .flatten()
    }

    fn show_idle(&self) {
        if let Some(ui) = self.ui.upgrade() {
            ui.set_orient_running(false);
        }
    }

    fn set_status(&self, text: String) {
        if let Some(ui) = self.ui.upgrade() {
            ui.set_orient_status(text.into());
        }
    }

    fn clear_shown(&self) {
        self.end_preview();
        *self.shown.borrow_mut() = None;
        self.selected.set(-1);
        if let Some(ui) = self.ui.upgrade() {
            ui.set_orient_options(slint::ModelRc::default());
            ui.set_orient_selected(-1);
            ui.set_orient_current("".into());
        }
    }

    /// Whether the options on show still describe the scene.
    fn shown_is_current(&self) -> bool {
        self.shown.borrow().as_ref().is_some_and(|shown| {
            self.document.scene_revision() == shown.revision
                && self.document.selection_primary_node_id() == Some(shown.leaf)
        })
    }

    /// The pointer entered (`on`) or left option `i`.
    pub(crate) fn hover(&self, i: i32, on: bool) {
        let Ok(i) = usize::try_from(i) else {
            return;
        };
        if on {
            self.preview(i);
        } else if self.previewing.get() == Some(i) {
            self.end_preview();
        }
    }

    /// Shows option `i` on the model, uncommitted.
    pub(crate) fn preview(&self, i: usize) {
        if !self.shown_is_current() || !self.editable() {
            return;
        }
        let (leaf, after) = {
            let shown = self.shown.borrow();
            let Some(shown) = shown.as_ref() else {
                return;
            };
            let Some(after) = shown.afters.get(i) else {
                return;
            };
            (shown.leaf, after.clone())
        };
        if !self.set_pose(leaf, after) {
            return;
        }
        if self.previewing.replace(Some(i)).is_none() {
            let me = self.me.clone();
            self.document.set_live_preview(Some(Rc::new(move || {
                if let Some(run) = me.upgrade() {
                    run.put_back();
                }
            })));
        }
    }

    /// Puts back the pose an option shows, if one does.
    pub(crate) fn end_preview(&self) {
        if self.previewing.get().is_some() {
            self.document.set_live_preview(None);
            self.put_back();
        }
    }

    /// The model back where the history has it.
    fn put_back(&self) {
        if self.previewing.take().is_none() {
            return;
        }
        let back = self
            .shown
            .borrow()
            .as_ref()
            .map(|shown| (shown.leaf, shown.before.clone()));
        if let Some((leaf, before)) = back {
            self.set_pose(leaf, before);
        }
    }

    /// Puts `leaf` at `local`, uncommitted. False when the scene refused.
    fn set_pose(&self, leaf: NodeId, local: Transform3D) -> bool {
        let Some(current) = self.local_of(leaf) else {
            return false;
        };
        if current == local {
            return true;
        }
        self.document
            .apply_edit(SceneEdit::SetLocalTransform {
                node_id: leaf,
                before: current,
                after: local,
            })
            .is_ok()
    }

    pub(crate) fn pick(&self, i: i32) {
        let count = self.shown.borrow().as_ref().map_or(0, |s| s.afters.len());
        if i < 0 || i as usize >= count {
            return;
        }
        self.selected.set(i);
        if let Some(ui) = self.ui.upgrade() {
            ui.set_orient_selected(i);
        }
    }

    /// Applies the picked option: one transform edit, one Undo step. False
    /// when nothing was applied.
    pub(crate) fn apply(&self) -> bool {
        self.end_preview();
        let Ok(i) = usize::try_from(self.selected.get()) else {
            return false;
        };
        if !self.shown_is_current() || !self.editable() {
            self.clear_shown();
            self.set_status(
                ctr!(
                    "Inspector",
                    "The model changed since the search. Search again."
                )
                .to_string(),
            );
            return false;
        }
        let (leaf, before, after) = {
            let shown = self.shown.borrow();
            let Some(shown) = shown.as_ref() else {
                return false;
            };
            let Some(after) = shown.afters.get(i) else {
                return false;
            };
            (shown.leaf, shown.before.clone(), after.clone())
        };
        let edit = SceneEdit::SetLocalTransform {
            node_id: leaf,
            before,
            after,
        };
        match self.document.apply_edit(edit.clone()) {
            Ok(()) => self.document.push_edit(edit),
            Err(EditNotApplied::Rejected(error)) => {
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("OrientDialog", "Auto-orient failed"),
                    context: None,
                    message: ctr!(
                        "OrientDialog",
                        "The scene rejected the new pose; nothing was moved."
                    ),
                    details: error.to_string().into(),
                });
                return false;
            }
            Err(EditNotApplied::NoSceneLoaded) => return false,
        }
        crate::breadcrumbs::note(&format!("orient: applied option {}", letter(i)));
        self.clear_shown();
        self.set_status(
            ctr!(
                "Inspector",
                "Applied option {}. Undo puts the model back.",
                letter(i)
            )
            .to_string(),
        );
        if let Some(ui) = self.ui.upgrade() {
            // A tilted model has a new footprint.
            ui.set_orient_offer_arrange(self.model_count() > 1);
        }
        self.refresh_warning();
        true
    }

    /// Opens the tool and runs a search to its end, taking each result as
    /// the event loop would: for tests. The options on show, or none.
    #[cfg(test)]
    pub(crate) fn search_now(&self, mode: SearchMode) -> Vec<Candidate> {
        let wait = || {
            let start = Instant::now();
            while self
                .slot
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_none()
            {
                assert!(
                    start.elapsed() < Duration::from_secs(120),
                    "the worker hangs"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        if !self.open.get() {
            self.toggle();
            if self.job.borrow().is_some() {
                wait();
                self.finish();
            }
        }
        self.search(mode);
        wait();
        let options = match self.slot.lock().unwrap().as_ref() {
            Some((
                _,
                Ok(Worked {
                    found: Some(Ok(found)),
                    ..
                }),
            )) => found.outcome.options.clone(),
            _ => Vec::new(),
        };
        self.finish();
        if self.shown.borrow().is_some() {
            options
        } else {
            Vec::new()
        }
    }

    fn model_count(&self) -> usize {
        self.document
            .with_scene(|spatial| {
                (0..)
                    .map_while(|i| spatial.leaf_id_at_index(i))
                    .filter(|&id| spatial.leaf(id).is_some_and(|q| !is_support_mesh(q.mesh())))
                    .count()
            })
            .unwrap_or(0)
    }

    /// After an edit, Undo, another selection or another scene: options for
    /// a scene that is gone go, and a search for another model stops.
    fn on_scene_changed(&self) {
        let selected = self.document.selection_primary_node_id();
        let other_model = self
            .job
            .borrow()
            .as_ref()
            .is_some_and(|job| Some(job.target.leaf) != selected);
        if other_model {
            self.cancel();
        }
        if self.shown.borrow().is_some() && !self.shown_is_current() {
            // Not put back: the scene the preview stood on is gone.
            self.previewing.set(None);
            self.document.set_live_preview(None);
            self.clear_shown();
            self.set_status(String::new());
        }
        if self.open.get() && Target::selected(&self.document).is_none() {
            self.close();
            return;
        }
        if self.open.get() {
            self.refresh_warning();
        }
    }

    fn refresh_warning(&self) {
        self.refresh_warning_with(None);
    }

    /// The card's warning: supports to make again, a cavity made for the
    /// old pose, an open mesh, nothing that fits.
    fn refresh_warning_with(&self, outcome: Option<&SearchOutcome>) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let Some(target) = Target::selected(&self.document) else {
            ui.set_orient_warning("".into());
            return;
        };
        let mut lines: Vec<String> = Vec::new();
        let sessions = self.document.support_sessions();
        let supported = self
            .document
            .with_scene(|spatial| {
                !crate::support_persist::sets_of_model(spatial, &sessions, target.leaf).is_empty()
            })
            .unwrap_or(false);
        if supported {
            lines.push(
                ctr!(
                    "Inspector",
                    "This model has supports: an option that tilts it makes them again; a turn about the vertical takes them along."
                )
                .to_string(),
            );
        }
        let hollowed = self
            .document
            .hollows()
            .borrow()
            .iter()
            .any(|h| Arc::ptr_eq(&h.hollow, &target.mesh));
        if hollowed {
            lines.push(
                ctr!(
                    "Inspector",
                    "This model is hollowed: its cavity was made for the current pose. Orienting before hollowing works best."
                )
                .to_string(),
            );
        }
        if let Some(outcome) = outcome {
            if outcome.notes.contains(&Note::OpenMesh) {
                lines.push(
                    ctr!(
                        "Inspector",
                        "The mesh is not closed, so peel force was left out. Repair it for a full answer."
                    )
                    .to_string(),
                );
            }
            if !outcome.options.is_empty() && outcome.options.iter().all(|c| !c.cost.fit.ok()) {
                lines.push(
                    ctr!(
                        "Inspector",
                        "No option fits the printer. Scale the model down or pick a larger printer."
                    )
                    .to_string(),
                );
            }
        }
        ui.set_orient_warning(lines.join("\n").into());
    }
}

fn show_step(ui: &MainWindow, step: Step) {
    let (phase, progress) = match step {
        Step::Preparing => (0, 0.08),
        Step::Searching => (1, 0.3),
        Step::Refining => (2, 0.6),
        Step::Judging => (3, 0.85),
    };
    ui.set_orient_running(true);
    ui.set_orient_phase(phase);
    ui.set_orient_progress(progress);
}

fn letter(i: usize) -> &'static str {
    ["A", "B", "C", "D", "E"].get(i).copied().unwrap_or("?")
}

/// The option cards' rows.
fn option_rows(outcome: &SearchOutcome, mode: SearchMode) -> Vec<OrientOption> {
    let now = outcome.current.cost.estimated_tips();
    let mode = match mode {
        SearchMode::Quick => ctr!("Inspector", "Quick"),
        SearchMode::Full => ctr!("Inspector", "Full"),
    };
    outcome
        .options
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let tips = c.cost.estimated_tips();
            let change = if now >= 0.5 {
                let pct = (tips - now) / now * 100.0;
                if pct.abs() < 0.5 {
                    "0 %".to_owned()
                } else if pct < 0.0 {
                    format!("\u{2212}{:.0} %", -pct)
                } else {
                    format!("+{pct:.0} %")
                }
            } else {
                String::new()
            };
            OrientOption {
                letter: letter(i).into(),
                mode: mode.clone(),
                tips: ctr!("Inspector", "≈ {} tips", format!("{tips:.0}")),
                change: change.into(),
                better: tips < now - 0.5,
                peak: peak_text(c),
                height: ctr!(
                    "Inspector",
                    "{} mm tall",
                    format!("{:.0}", c.cost.height_mm)
                ),
                tilt: tilt_text(c),
                fits: c.cost.fit.ok(),
            }
        })
        .collect()
}

fn peak_text(c: &Candidate) -> slint::SharedString {
    if c.cost.peel_measured {
        ctr!(
            "Inspector",
            "Peak {} N",
            format!("{:.1}", c.cost.peel.peak_force_n)
        )
    } else {
        ctr!("Inspector", "Peak not measured")
    }
}

fn tilt_text(c: &Candidate) -> slint::SharedString {
    let (tilt, turn) = (c.tilt_deg, c.turn_deg);
    match (tilt >= 0.5, turn.abs() >= 0.5) {
        (false, false) => ctr!("Inspector", "As it stands"),
        (true, false) => ctr!("Inspector", "Tilts {}°", format!("{tilt:.0}")),
        (false, true) => ctr!("Inspector", "Turns {}°", format!("{turn:.0}")),
        (true, true) => ctr!(
            "Inspector",
            "Tilts {}°, turns {}°",
            format!("{tilt:.0}"),
            format!("{turn:.0}")
        ),
    }
}

/// The current pose's numbers, under the options.
fn current_row(c: &Candidate) -> String {
    let tips = format!("{:.0}", c.cost.estimated_tips());
    let height = format!("{:.0}", c.cost.height_mm);
    if c.cost.peel_measured {
        ctr!(
            "Inspector",
            "Now: ≈ {} tips · peak {} N · {} mm tall",
            tips,
            format!("{:.1}", c.cost.peel.peak_force_n),
            height
        )
        .to_string()
    } else {
        ctr!("Inspector", "Now: ≈ {} tips · {} mm tall", tips, height).to_string()
    }
}

#[cfg(test)]
#[path = "tests_orient_run.rs"]
mod tests;
