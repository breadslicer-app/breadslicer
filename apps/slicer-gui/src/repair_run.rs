/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Per-window driver for the Repair action.
//!
//! The Repair button clones the scene on the UI thread and runs the mesh-core
//! repair policy on a worker thread so the event loop stays responsive; the
//! finished result is deposited into the run's own slot and picked up on the
//! UI thread via `invoke_repair_finished`. While it runs, the MESH CHECK card
//! shows the step and how far the run is (polled from the worker's watch),
//! and Cancel (#253): the worker stops at its next step, and nothing it did
//! is applied. The UI is released at once; a cancelled worker that is still
//! finishing its step writes into a slot no one reads. The notice that follows
//! a run shows the report of `repair_report`.
//!
//! The window's `repairing-in-progress` property is the single gate: the button
//! and undo/redo are disabled while it is set (an undo landing mid-run would
//! invalidate the worker's Arc diff), and it is cleared on every exit path. The
//! scene revision captured at launch guards the apply: a result computed
//! against geometry that changed mid-run, or against a scene the user has
//! replaced since, is discarded, never merged.
//!
//! The applied result is a single `SceneEdit::Compound` of mesh-content replace
//! edits (1 undo unit): only meshes the policy actually changed appear in it,
//! because the driver forks storage on fix so clean and refused targets keep
//! their Arc identity.
//!
//! What a run leaves broken is marked on the model (`repair_marks`), for the
//! meshes the scene still holds, while the Transform tab shows and the card's
//! "Show what is left on the model" is on.
//!
//! The card of a slice that the slicer refused offers Repair (#407): the same
//! run, on the refused objects only, from the Transform tab, so its progress,
//! Cancel and report are the ones above. When it changed them and they all
//! slice now, the run goes back to the tab the slice came from and slices
//! again; otherwise its report says what is left, and nothing slices.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use sc_geom::df_supports::{DfSupportSession, scene_model_leaves};
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::indexed_mesh::RepairPasses;
use sc_mesh_core::indexed_mesh::repair::RepairEnd;
use sc_mesh_core::indexed_mesh::repair_progress::{RepairCancelled, RepairStep};
use sc_mesh_core::transform::Transform3D;
use sc_mesh_spatial::PrimedMesh;
use sc_scene::mesh_targets::MeshTarget;
use sc_scene::repair::{SceneRepairOutcome, SceneRepairProgress, SceneRepairWatch};
use sc_scene::{NodeId, Scene, SceneError};
use sc_scene_edit::SceneEdit;
use slint::ComponentHandle;

use crate::MainWindow;
use crate::WorkspaceSection;
use crate::error_dialog::{DialogManager, ErrorTexts, InfoTexts, MessageKey};
use crate::i18n::ctr;
use crate::mesh_check::{show_card, target_arc};
use crate::mesh_loader::panic_message;
use crate::mesh_render::Overlay;
use crate::repair_marks::RepairMarks;
use crate::repair_report::{
    details_text, left_reason, object_names_by_leaf, summary_message, target_names,
};
use crate::scene_document::{
    CarriedPaint, EditNotApplied, SceneDocument, SceneEvent, SceneRevision, SceneSubscription,
    carried_paint,
};
use crate::surface_brush::lost_paint_note;
use crate::viewport::ViewportController;
use crate::workspace_section::allows_repair;

/// How often the card reads the worker's progress.
const PROGRESS_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How far, in mm, a support's tip may be from the repaired model and still
/// hold it (#477, #526): the reach the supports keep in step with their
/// model by everywhere.
const TIP_STILL_TOUCHES_MM: f64 = crate::df_support_run::HEAL_REACH_MM;

/// A support set in force when the run started (#477).
struct HeldSet {
    leaf: NodeId,
    model: NodeId,
    session: Arc<DfSupportSession>,
}

/// A set whose model the run mended: its session on the mended model, or
/// `None` when a support no longer touches it.
struct MendedSet {
    leaf: NodeId,
    model: NodeId,
    session: Option<Arc<DfSupportSession>>,
}

/// A mesh the repair run rewrote: the storage place plus the exact Arcs before
/// and after, ready to become a replace edit.
struct MeshReplacement {
    target: MeshTarget,
    before: Arc<IndexedMesh>,
    after: Arc<IndexedMesh>,
}

impl MeshReplacement {
    fn as_edit(&self) -> SceneEdit {
        match self.target {
            MeshTarget::Resource(rid) => SceneEdit::ReplaceMeshResource {
                rid,
                before: Arc::clone(&self.before),
                after: Arc::clone(&self.after),
            },
            MeshTarget::InlineLeaf(node_id) => SceneEdit::ReplaceLeafMesh {
                node_id,
                before: Arc::clone(&self.before),
                after: Arc::clone(&self.after),
            },
        }
    }
}

/// The worker's finished payload: every per-target outcome plus the
/// replacements for the meshes that actually changed.
struct CompletedRepair {
    outcomes: Vec<SceneRepairOutcome>,
    replacements: Vec<MeshReplacement>,
    /// The caches of each repaired mesh (its pick structure among them),
    /// built on the worker, so the first click after the repair builds
    /// nothing on the window's thread (#395).
    primed: Vec<PrimedMesh>,
    /// The paint on the meshes it replaced, worked out on their new meshes
    /// here: a large model takes a moment the window's thread does not have.
    paint: Vec<CarriedPaint>,
    /// What each target that still does not slice has left, to mark on the
    /// model.
    marks: Vec<(MeshTarget, Arc<RepairMarks>)>,
    /// How long the whole run took on the worker.
    elapsed: Duration,
    /// The support sets on the models it mended, made over on their new
    /// meshes when every support still touches them (#477).
    mended_sets: Vec<MendedSet>,
}

/// What the worker deposits: the run, a cancelled run, or a panic.
type WorkerResult = Result<Result<CompletedRepair, RepairCancelled>, String>;

type RepairSlot = Arc<Mutex<Option<WorkerResult>>>;

/// What the worker tells and the UI thread reads: how far the run is, and
/// whether to stop.
#[derive(Default)]
struct RunWatch {
    progress: Mutex<Option<SceneRepairProgress>>,
    cancel: AtomicBool,
}

impl SceneRepairWatch for RunWatch {
    fn progress(&self, progress: SceneRepairProgress) {
        *self.progress.lock().unwrap_or_else(PoisonError::into_inner) = Some(progress);
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// 1 run in flight.
struct Job {
    slot: RepairSlot,
    watch: Arc<RunWatch>,
    /// Revision captured at launch.
    revision: SceneRevision,
    /// The object-list name of each target, in run order, for the card.
    names: Vec<String>,
    /// Set when a refused slice asked for the run (#407).
    for_slice: Option<ForSlice>,
}

/// What follows a run that a refused slice asked for (#407).
struct ForSlice {
    /// The tab the slice came from, to go back to before slicing again.
    back_to: WorkspaceSection,
    /// Slices again; run only once every repaired object slices.
    reslice: Box<dyn FnOnce()>,
}

/// What `settle` did with a deposited result.
#[derive(Debug, PartialEq, Eq)]
enum RepairSettlement {
    /// The scene changed while the worker ran; nothing was applied.
    DiscardedStale,
    /// Nothing needed applying (clean scene, or refusals only).
    NothingApplied,
    /// The replacements landed as a single undoable Compound edit.
    Applied,
    /// The scene rejected the replacement edit; nothing was recorded.
    Rejected(SceneError),
}

/// The marks drawn now: per object, its marks and its pose, so a scene event
/// that moves nothing marked builds nothing.
type ShownMarks = Vec<(NodeId, Arc<RepairMarks>, Transform3D)>;

pub(crate) struct RepairRun {
    ui: slint::Weak<MainWindow>,
    document: Rc<SceneDocument>,
    dialogs: Rc<DialogManager>,
    ctrl: Rc<ViewportController>,
    job: RefCell<Option<Job>>,
    poll_timer: slint::Timer,
    /// Whether the card's "Show what is left on the model" is on.
    marks_shown: Cell<bool>,
    shown_marks: RefCell<Option<ShownMarks>>,
    // Keeps the MESH CHECK card and the marks following the scene.
    scene_events: RefCell<Option<SceneSubscription>>,
}

impl RepairRun {
    /// Wires the Repair / Cancel / finished callbacks and returns the driver,
    /// which the caller keeps alive for the window's lifetime.
    pub(crate) fn install(
        ui: &MainWindow,
        document: Rc<SceneDocument>,
        dialogs: Rc<DialogManager>,
        ctrl: Rc<ViewportController>,
    ) -> Rc<Self> {
        let run = Rc::new(Self {
            ui: ui.as_weak(),
            document,
            dialogs,
            ctrl,
            job: RefCell::new(None),
            poll_timer: slint::Timer::default(),
            marks_shown: Cell::new(true),
            shown_marks: RefCell::new(None),
            scene_events: RefCell::new(None),
        });
        let scene_events = {
            let weak = Rc::downgrade(&run);
            run.document.subscribe(move |event| {
                if let Some(run) = weak.upgrade() {
                    run.follow_scene(event);
                }
            })
        };
        *run.scene_events.borrow_mut() = Some(scene_events);
        {
            let run = run.clone();
            ui.on_repair_requested(move || run.start());
        }
        {
            let run = run.clone();
            ui.on_repair_finished(move || run.finish());
        }
        {
            let run = run.clone();
            ui.on_repair_cancel(move || run.cancel());
        }
        {
            let run = run.clone();
            ui.on_repair_marks_toggled(move |on| {
                run.marks_shown.set(on);
                run.refresh_marks(true);
            });
        }
        ui.set_repair_marks_shown(true);
        run
    }

    /// Handles a Repair press: gate, clone, capture, spawn. The button gates
    /// most of this too; the checks keep the path safe against racing
    /// shortcuts.
    fn start(self: &Rc<Self>) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if busy(&ui) || !allows_repair(ui.get_workspace_section()) {
            return;
        }
        let Some((scene, targets, names)) = self.document.with_scene(|spatial| {
            let graph = spatial.graph();
            let targets = graph.mesh_targets();
            let names = names_in_run_order(graph, &targets);
            (graph.clone(), targets, names)
        }) else {
            return;
        };
        self.launch(&ui, scene, targets, names, None);
    }

    /// Repairs the meshes of `leaves`, the objects a slice refused (#407),
    /// from the Transform tab, as the Repair button does. `reslice` runs once
    /// the run changed them and every one slices, back in the tab the slice
    /// came from. Returns false, starting nothing, while something else runs
    /// or when the scene holds none of them any more.
    pub(crate) fn repair_for_slice(
        self: &Rc<Self>,
        leaves: &[NodeId],
        reslice: impl FnOnce() + 'static,
    ) -> bool {
        let Some(ui) = self.ui.upgrade() else {
            return false;
        };
        if busy(&ui) {
            return false;
        }
        let Some((scene, targets, names)) = self.document.with_scene(|spatial| {
            let graph = spatial.graph();
            let asked: Vec<MeshTarget> = leaves
                .iter()
                .filter_map(|&leaf| graph.leaf_mesh_target(leaf))
                .collect();
            // In the order of a full run, each once, live ones only.
            let targets: Vec<MeshTarget> = graph
                .mesh_targets()
                .into_iter()
                .filter(|target| asked.contains(target))
                .collect();
            let names = names_in_run_order(graph, &targets);
            (graph.clone(), targets, names)
        }) else {
            return false;
        };
        if targets.is_empty() {
            return false;
        }
        let back_to = ui.get_workspace_section();
        // The run's progress and Cancel are on the Transform tab's card.
        self.ctrl.set_section(WorkspaceSection::Transform);
        let for_slice = ForSlice {
            back_to,
            reslice: Box::new(reslice),
        };
        self.launch(&ui, scene, targets, names, Some(for_slice));
        true
    }

    /// Captures the run and spawns its worker over `targets` of `scene`.
    fn launch(
        self: &Rc<Self>,
        ui: &MainWindow,
        scene: Scene,
        targets: Vec<MeshTarget>,
        names: Vec<String>,
        for_slice: Option<ForSlice>,
    ) {
        let slot: RepairSlot = Arc::new(Mutex::new(None));
        let watch = Arc::new(RunWatch::default());
        let held = held_sets(&self.document);
        *self.job.borrow_mut() = Some(Job {
            slot: slot.clone(),
            watch: watch.clone(),
            revision: self.document.scene_revision(),
            names,
            for_slice,
        });
        ui.set_repair_progress(0.0);
        ui.set_repair_phase(Default::default());
        ui.set_repairing_in_progress(true);
        crate::breadcrumbs::started("repair");
        self.start_poll();

        let ui_weak = self.ui.clone();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_repair_watched(scene, &targets, watch.as_ref(), &held)
            }))
            .map_err(|payload| panic_message(&payload));
            *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(result);
            let _ = ui_weak.upgrade_in_event_loop(|ui| ui.invoke_repair_finished());
        });
    }

    /// Shows the worker's progress on the card while the run goes.
    fn start_poll(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.poll_timer.start(
            slint::TimerMode::Repeated,
            PROGRESS_POLL_INTERVAL,
            move || {
                if let Some(run) = weak.upgrade() {
                    run.show_progress();
                }
            },
        );
    }

    fn show_progress(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let job = self.job.borrow();
        let Some(job) = job.as_ref() else {
            return;
        };
        let progress = *job
            .watch
            .progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(progress) = progress {
            ui.set_repair_progress(progress.done);
            ui.set_repair_phase(phase_text(&progress, &job.names).into());
        }
    }

    /// Cancel on the card: the worker stops at its next step, and the scene
    /// stays exactly as it was (the run works on a clone and applies nothing
    /// until it is done). The UI is released at once.
    fn cancel(&self) {
        let Some(job) = self.job.borrow_mut().take() else {
            return;
        };
        job.watch.cancel.store(true, Ordering::Relaxed);
        self.poll_timer.stop();
        crate::breadcrumbs::finished("repair", "cancelled");
        if let Some(ui) = self.ui.upgrade() {
            ui.set_repairing_in_progress(false);
        }
    }

    /// Drains the deposited result on the UI thread, applies it when it is
    /// still current, and reports through the info dialog. The tab stays
    /// Transform; its repair card follows the scene's new health. A wake-up
    /// from a cancelled worker finds the run it belongs to gone, and does
    /// nothing.
    fn finish(&self) {
        let ready = self.job.borrow().as_ref().is_some_and(|job| {
            job.slot
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_some()
        });
        if !ready {
            return;
        }
        let Some(job) = self.job.borrow_mut().take() else {
            return;
        };
        self.poll_timer.stop();
        if let Some(ui) = self.ui.upgrade() {
            ui.set_repairing_in_progress(false);
        }
        let taken = job
            .slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some(result) = taken else {
            return;
        };
        let completed = match result {
            Ok(Ok(completed)) => completed,
            // Only Cancel stops a run, and it takes the job first.
            Ok(Err(RepairCancelled)) => {
                crate::breadcrumbs::finished("repair", "cancelled");
                return;
            }
            Err(panic) => {
                crate::breadcrumbs::finished("repair", "crashed");
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("RepairDialog", "Repair failed"),
                    context: None,
                    message: ctr!("RepairDialog", "The repair worker crashed."),
                    details: panic.as_str().into(),
                });
                return;
            }
        };
        crate::breadcrumbs::finished("repair", "ok");
        // The meshes the run made get their verdicts before they land, so
        // the card, which follows the scene, never checks them again.
        let check = self.document.mesh_check();
        let marks_of = |target: MeshTarget| {
            completed
                .marks
                .iter()
                .find(|(t, _)| *t == target)
                .map(|(_, marks)| marks.clone())
        };
        for replacement in &completed.replacements {
            if let Some(outcome) = completed
                .outcomes
                .iter()
                .find(|o| o.target == replacement.target)
            {
                check.remember(
                    &replacement.after,
                    outcome_slices(outcome),
                    left_reason(outcome),
                );
                check.remember_marks(&replacement.after, marks_of(replacement.target));
            }
        }
        let settlement = settle(&self.document, &completed, job.revision);
        let lost_paint = lost_paint_note(&self.document.take_paint_report());
        match settlement {
            RepairSettlement::DiscardedStale => {
                self.dialogs.show_info(|| InfoTexts {
                    heading: ctr!("RepairDialog", "Repair discarded"),
                    message: ctr!(
                        "RepairDialog",
                        "The scene changed while repairing; nothing was applied."
                    ),
                    details: Default::default(),
                });
            }
            RepairSettlement::NothingApplied | RepairSettlement::Applied => {
                // The Transform tab's repair card follows what the run left:
                // the live scene now holds the meshes the run ended with, and
                // a mesh the run left as it was gets what is left on it.
                let (names, card) = self
                    .document
                    .with_scene(|spatial| {
                        let graph = spatial.graph();
                        for outcome in &completed.outcomes {
                            if let Some(mesh) = target_arc(graph, outcome.target) {
                                check.remember(
                                    &mesh,
                                    outcome_slices(outcome),
                                    left_reason(outcome),
                                );
                                check.remember_marks(&mesh, marks_of(outcome.target));
                            }
                        }
                        (
                            target_names(&completed.outcomes, graph),
                            check.card(graph, true),
                        )
                    })
                    .unwrap_or_default();
                if let (Some(ui), Some(card)) = (self.ui.upgrade(), card) {
                    show_card(&ui, &card);
                }
                // A new run shows what it left.
                self.marks_shown.set(true);
                self.refresh_marks(true);
                let applied = settlement == RepairSettlement::Applied;
                // The supports whose model the repair moved from under their
                // tips are made again on their own, keeping the edits made by
                // hand (#526): no card stops for them.
                let CompletedRepair {
                    outcomes, elapsed, ..
                } = completed;
                // Never behind a notice someone may have turned off.
                if let Some(note) = lost_paint.clone() {
                    self.dialogs.show_info(move || InfoTexts {
                        heading: ctr!("RepairDialog", "Painted detail"),
                        message: note.clone(),
                        details: Default::default(),
                    });
                }
                match job.for_slice {
                    // A run that changed nothing would only slice into the
                    // same refusal again.
                    Some(_) if !(applied && outcomes.iter().all(slices_now)) => {
                        self.dialogs.show_info(move || InfoTexts {
                            heading: ctr!("RepairDialog", "Still can't slice"),
                            message: ctr!(
                                "RepairDialog",
                                "Repair couldn't fix everything, so slicing didn't start again. {}",
                                summary_message(&outcomes, &names)
                            ),
                            details: details_text(&outcomes, &names, elapsed).into(),
                        });
                    }
                    for_slice => {
                        self.dialogs
                            .show_notice(MessageKey::MeshRepairSummary, move || InfoTexts {
                                heading: ctr!("RepairDialog", "Mesh repair"),
                                message: summary_message(&outcomes, &names).into(),
                                details: details_text(&outcomes, &names, elapsed).into(),
                            });
                        if let Some(ForSlice { back_to, reslice }) = for_slice {
                            self.ctrl.set_section(back_to);
                            reslice();
                        }
                    }
                }
            }
            RepairSettlement::Rejected(error) => {
                let error = error.to_string();
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("RepairDialog", "Repair failed"),
                    context: None,
                    message: ctr!(
                        "RepairDialog",
                        "The scene rejected the repair edit; nothing was changed."
                    ),
                    details: error.as_str().into(),
                });
            }
        }
    }

    /// Keeps the MESH CHECK card and the marks on what the scene holds now:
    /// an undo that puts a broken mesh back, a deleted or moved object. A
    /// mesh with no verdict is checked only while the card shows a problem
    /// (see `mesh_check`).
    fn follow_scene(&self, event: SceneEvent) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        match event {
            SceneEvent::Mutated | SceneEvent::Restructured | SceneEvent::UndoStackChanged => {
                let check_unknown = ui.get_mesh_needs_repair();
                let card = self
                    .document
                    .with_scene(|spatial| {
                        self.document
                            .mesh_check()
                            .card(spatial.graph(), check_unknown)
                    })
                    .flatten();
                if let Some(card) = card {
                    show_card(&ui, &card);
                }
                self.refresh_marks(false);
            }
            SceneEvent::Cleared => {
                self.document.mesh_check().forget_all();
                show_card(&ui, &Default::default());
                self.refresh_marks(false);
            }
            // A new scene gets its card from the load worker's verdicts.
            SceneEvent::Installed => self.refresh_marks(false),
            SceneEvent::SelectionChanged
            | SceneEvent::SliceChanged
            | SceneEvent::DirtyChanged
            | SceneEvent::Painted => {}
        }
    }

    /// Hands the viewport the marks of the meshes the scene holds, posed per
    /// object; with `force` off, only when what they show moved or changed.
    fn refresh_marks(&self, force: bool) {
        let check = self.document.mesh_check();
        // Most scenes have nothing marked: following them walks nothing.
        let any = check.any_marks();
        if !any && !force && self.shown_marks.borrow().as_ref().is_none_or(Vec::is_empty) {
            if let Some(ui) = self.ui.upgrade() {
                ui.set_repair_marks_available(false);
            }
            return;
        }
        let marked: ShownMarks = self
            .document
            .with_scene(|spatial| {
                let graph = spatial.graph();
                let mut marked = Vec::new();
                for target in graph.mesh_targets() {
                    let Some(marks) = target_arc(graph, target).and_then(|m| check.marks_of(&m))
                    else {
                        continue;
                    };
                    for leaf in graph.leaves_referencing(target) {
                        if let Ok(world) = graph.world_transform_of(leaf) {
                            marked.push((leaf, marks.clone(), world));
                        }
                    }
                }
                marked
            })
            .unwrap_or_default();
        if let Some(ui) = self.ui.upgrade() {
            ui.set_repair_marks_available(!marked.is_empty());
            ui.set_repair_marks_shown(self.marks_shown.get());
        }
        let shown = self.marks_shown.get().then_some(marked);
        let unchanged = match (&*self.shown_marks.borrow(), &shown) {
            (Some(a), Some(b)) => same_marks(a, b),
            (None, None) => true,
            _ => false,
        };
        if unchanged && !force {
            return;
        }
        let overlay = shown
            .as_ref()
            .filter(|marked| !marked.is_empty())
            .map(|marked| {
                let pieces: Vec<_> = marked
                    .iter()
                    .flat_map(|(_, marks, world)| marks.overlay(world))
                    .collect();
                Overlay::from(pieces)
            });
        *self.shown_marks.borrow_mut() = shown;
        self.ctrl.set_repair_marks(overlay);
    }
}

/// Whether 2 lists of posed marks draw the same: the same marks, by
/// pointer, on the same objects at the same poses.
fn same_marks(a: &ShownMarks, b: &ShownMarks) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|((la, ma, wa), (lb, mb, wb))| la == lb && Arc::ptr_eq(ma, mb) && wa == wb)
}

/// The card's line for `progress`: the step, and with several objects which
/// one, by its object-list name.
fn phase_text(progress: &SceneRepairProgress, names: &[String]) -> String {
    let step = step_text(progress.step);
    if progress.targets < 2 {
        return step;
    }
    let name = names
        .get(progress.target)
        .cloned()
        .unwrap_or_else(|| ctr!("RepairDialog", "Mesh {}", progress.target + 1).to_string());
    ctr!(
        "RepairDialog",
        "{} — {} ({} of {})",
        step,
        name,
        progress.target + 1,
        progress.targets
    )
    .to_string()
}

fn step_text(step: RepairStep) -> String {
    match step {
        RepairStep::Checking => ctr!("RepairDialog", "Checking the mesh…"),
        RepairStep::RemovingBrokenFaces => ctr!("RepairDialog", "Removing broken faces…"),
        RepairStep::Welding => ctr!("RepairDialog", "Welding cracks…"),
        RepairStep::RemovingDuplicates => ctr!("RepairDialog", "Removing duplicate faces…"),
        RepairStep::SeparatingParts => ctr!("RepairDialog", "Separating parts that touch…"),
        RepairStep::FillingHoles => ctr!("RepairDialog", "Filling holes…"),
        RepairStep::TurningShellsOutward => {
            ctr!("RepairDialog", "Turning inside-out shells outward…")
        }
        RepairStep::CheckingRound => ctr!("RepairDialog", "Checking that nothing was lost…"),
        RepairStep::Orienting => ctr!("RepairDialog", "Orienting faces…"),
        RepairStep::RemovingSlivers => ctr!("RepairDialog", "Removing slivers…"),
        RepairStep::CheckingResult => ctr!("RepairDialog", "Checking the result…"),
    }
    .to_string()
}

/// Whether a load, a slice or a repair is going: no run starts then.
fn busy(ui: &MainWindow) -> bool {
    ui.get_loading() || ui.get_slicing_in_progress() || ui.get_repairing_in_progress()
}

/// The object-list name of each of `targets` of `graph`, in the order a
/// repair run walks them.
fn names_in_run_order(graph: &Scene, targets: &[MeshTarget]) -> Vec<String> {
    let by_leaf = object_names_by_leaf(graph);
    targets
        .iter()
        .copied()
        .enumerate()
        .map(|(i, target)| {
            graph
                .leaves_referencing(target)
                .into_iter()
                .find_map(|leaf| by_leaf.get(&leaf).cloned())
                .unwrap_or_else(|| ctr!("RepairDialog", "Mesh {}", i + 1).to_string())
        })
        .collect()
}

/// Repairs every stored mesh of the cloned scene, on the worker, and derives
/// the replacements from the Arc diff. The clone shares its mesh storage with
/// the live scene, so an Arc that survived the run unchanged proves the live
/// mesh needs no replacing.
#[cfg(test)]
fn run_repair(scene: Scene) -> CompletedRepair {
    let targets = scene.mesh_targets();
    match run_repair_watched(scene, &targets, &NoWatch, &[]) {
        Ok(completed) => completed,
        Err(RepairCancelled) => unreachable!("an unwatched repair was cancelled"),
    }
}

/// A watch that never cancels, for tests.
#[cfg(test)]
struct NoWatch;

#[cfg(test)]
impl SceneRepairWatch for NoWatch {
    fn progress(&self, _progress: SceneRepairProgress) {}

    fn cancelled(&self) -> bool {
        false
    }
}

/// [`run_repair`] over `targets` only, told to `watch`; a cancelled run
/// returns nothing.
fn run_repair_watched(
    mut scene: Scene,
    targets: &[MeshTarget],
    watch: &dyn SceneRepairWatch,
    held: &[HeldSet],
) -> Result<CompletedRepair, RepairCancelled> {
    let start = Instant::now();
    let before: Vec<(MeshTarget, Arc<IndexedMesh>)> = targets
        .iter()
        .filter_map(|&target| Some((target, target_arc(&scene, target)?)))
        .collect();
    let outcomes = scene.repair_targets_watched(targets, RepairPasses::default_repair(), watch)?;
    let replacements = before
        .into_iter()
        .filter_map(|(target, before_arc)| {
            let after = target_arc(&scene, target)?;
            if Arc::ptr_eq(&before_arc, &after) {
                None
            } else {
                Some(MeshReplacement {
                    target,
                    before: before_arc,
                    after,
                })
            }
        })
        .collect::<Vec<MeshReplacement>>();
    let primed = replacements
        .iter()
        .map(|replacement| PrimedMesh::new(Arc::clone(&replacement.after)))
        .collect();
    let paint = carried_paint(&scene);
    let marks = outcomes
        .iter()
        .filter_map(|outcome| {
            let leftovers = outcome.details.leftovers.as_ref()?;
            let mesh = target_arc(&scene, outcome.target)?;
            let marks = RepairMarks::of(&mesh, leftovers)?;
            Some((outcome.target, Arc::new(marks)))
        })
        .collect();
    let mended_sets = mended_sets(&scene, &replacements, held);
    Ok(CompletedRepair {
        outcomes,
        replacements,
        primed,
        paint,
        marks,
        elapsed: start.elapsed(),
        mended_sets,
    })
}

/// The support sets in force in `document`'s scene, as a run starts.
fn held_sets(document: &SceneDocument) -> Vec<HeldSet> {
    document
        .with_scene(|spatial| {
            crate::support_persist::sets_in_force(spatial, &document.support_sessions())
                .into_iter()
                .map(|drawn| HeldSet {
                    leaf: drawn.leaf,
                    model: drawn.model,
                    session: drawn.session,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Of `held`, the sets whose model's mesh is among `replacements` while
/// their own mesh is not, each made over on the model as `scene`, repaired,
/// holds it (#477): the supports stay where they are, and the set stays in
/// force when every one of them still touches the model.
fn mended_sets(
    scene: &Scene,
    replacements: &[MeshReplacement],
    held: &[HeldSet],
) -> Vec<MendedSet> {
    let replaced = |leaf: NodeId| {
        scene
            .leaf_mesh_target(leaf)
            .is_some_and(|target| replacements.iter().any(|r| r.target == target))
    };
    held.iter()
        .filter(|set| replaced(set.model) || replaced(set.leaf))
        .map(|set| {
            // Supports whose own mesh alone was repaired keep their data
            // (#526): the next edit draws them anew.
            if !replaced(set.model) {
                return MendedSet {
                    leaf: set.leaf,
                    model: set.model,
                    session: Some(set.session.clone()),
                };
            }
            let now = scene_model_leaves(scene, Some(&[set.model]))
                .ok()
                .and_then(|mut models| models.pop())
                .map(|(_, model)| model);
            // The port is DragonFruit's code: a panic in it leaves the set
            // out of date, as before, not a failed repair.
            let session = now.and_then(|now| {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    set.session.rebased(0, now, TIP_STILL_TOUCHES_MM)
                }))
                .ok()
                .flatten()
                .map(Arc::new)
            });
            MendedSet {
                leaf: set.leaf,
                model: set.model,
                session,
            }
        })
        .collect()
}

/// Whether the slicer takes the mesh an outcome ends with: unlike for the
/// MESH CHECK card, a mesh left without faces does not count.
fn slices_now(outcome: &SceneRepairOutcome) -> bool {
    outcome.details.after.sliceable
}

/// Whether the mesh an outcome ends with slices, as the MESH CHECK card
/// counts it: a mesh without faces is no problem.
fn outcome_slices(outcome: &SceneRepairOutcome) -> bool {
    outcome.report.end == RepairEnd::NoFaces || outcome.details.after.sliceable
}

/// Applies a still-current result to the document: stale results are discarded,
/// an empty replacement set applies nothing, and anything else lands as a
/// single undoable Compound edit.
fn settle(
    document: &Rc<SceneDocument>,
    completed: &CompletedRepair,
    captured: SceneRevision,
) -> RepairSettlement {
    if document.slice_geometry_changed(captured) {
        return RepairSettlement::DiscardedStale;
    }
    if completed.replacements.is_empty() {
        return RepairSettlement::NothingApplied;
    }
    let mut edits: Vec<SceneEdit> = completed
        .replacements
        .iter()
        .map(MeshReplacement::as_edit)
        .collect();
    edits.extend(document.paint_edits(&completed.paint));
    let edit = SceneEdit::Compound(edits);
    match document.apply_edit_primed(edit.clone(), &completed.primed) {
        Ok(()) => {
            document.push_edit(edit);
            // The sets still holding their mended models stay in force on
            // them; Undo brings back the drawings of before (#477).
            let sessions = document.support_sessions();
            for set in &completed.mended_sets {
                if let Some(session) = &set.session {
                    crate::support_persist::record(
                        document,
                        &sessions,
                        set.leaf,
                        set.model,
                        session.clone(),
                    );
                }
            }
            RepairSettlement::Applied
        }
        // The scene disappearing mid-settle is the stale case; the revision
        // guard above makes it unreachable in practice.
        Err(EditNotApplied::NoSceneLoaded) => RepairSettlement::DiscardedStale,
        Err(EditNotApplied::Rejected(error)) => RepairSettlement::Rejected(error),
    }
}

#[cfg(test)]
mod repair_run_tests {
    use std::path::PathBuf;

    use sc_mesh_core::indexed_mesh::repair::MeshStateAfterRepair;
    use sc_mesh_core::indexed_mesh::{MeshFix, MeshRepairReport};
    use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
    use sc_mesh_core::transform::Transform3D;
    use sc_mesh_fixtures::{
        cuboid_with_y_tunnel_and_flipped_faces, cuboid_with_z_tunnel_and_flipped_faces,
        unit_cube_mesh, unit_cube_with_collinear_faces, unit_cube_with_flipped_faces,
    };
    use sc_scene::node::LeafPayload;
    use sc_scene::{PAINTED, PaintKind, SurfaceTags, UNPAINTED};

    use super::*;
    use crate::mesh_health::scene_slices;
    use crate::test_support::slint_thread::on_fresh_slint_thread;

    /// Returns the shared broken trio as 1 scene: cube and Z-tunnel inline,
    /// Y-tunnel behind a shared resource, plus 1 clean inline cube.
    fn broken_trio_scene() -> Scene {
        let mut graph = Scene::new();
        let root = graph.add_group(None, Transform3D::Identity);
        graph.add_root(root).expect("group root");
        for mesh in [
            unit_cube_with_flipped_faces(),
            cuboid_with_z_tunnel_and_flipped_faces(),
        ] {
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Identity,
                    LeafPayload::Mesh(Arc::new(mesh)),
                )
                .expect("inline leaf");
            graph.add_child(root, leaf).expect("attach");
        }
        let rid = graph.add_mesh_resource(cuboid_with_y_tunnel_and_flipped_faces());
        for _ in 0..2 {
            let leaf = graph
                .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
                .expect("resource leaf");
            graph.add_child(root, leaf).expect("attach");
        }
        let clean = graph
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
            )
            .expect("clean leaf");
        graph.add_child(root, clean).expect("attach");
        graph
    }

    #[test]
    fn run_repair_replaces_exactly_the_defective_targets() {
        let scene = broken_trio_scene();
        let completed = run_repair(scene.clone());
        // 4 targets: shared Y-tunnel resource + 3 inline leaves.
        assert_eq!(completed.outcomes.len(), 4);
        // 3 defective targets forked; the clean cube kept its Arc.
        assert_eq!(completed.replacements.len(), 3);
        for replacement in &completed.replacements {
            let live = target_arc(&scene, replacement.target).expect("live target");
            assert!(Arc::ptr_eq(&live, &replacement.before));
            assert!(!Arc::ptr_eq(&replacement.before, &replacement.after));
        }
    }

    #[test]
    fn run_repair_on_a_clean_scene_replaces_nothing() {
        let mut graph = Scene::new();
        let leaf = graph
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
            )
            .expect("leaf");
        graph.add_root(leaf).expect("root");
        let completed = run_repair(graph);
        assert!(completed.replacements.is_empty());
        let clean = MeshRepairReport {
            fixes: Vec::new(),
            end: RepairEnd::Completed(MeshStateAfterRepair {
                edge_manifold: true,
                zero_area_face_count: 0,
                holes_left_open: 0,
            }),
        };
        assert!(completed.outcomes.iter().all(|o| o.report == clean));
        let names = target_names(&completed.outcomes, &Scene::new());
        assert_eq!(
            summary_message(&completed.outcomes, &names),
            "No problems found. Everything can be sliced."
        );
    }

    /// Pins the pass set of `run_repair`, which chooses it itself: the 2
    /// collinear faces of the cube go, and the report says so.
    #[test]
    fn run_repair_removes_the_collinear_faces_of_a_mesh() {
        let mut graph = Scene::new();
        let leaf = graph
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_cube_with_collinear_faces())),
            )
            .expect("leaf");
        graph.add_root(leaf).expect("root");
        let completed = run_repair(graph.clone());
        assert_eq!(completed.replacements.len(), 1);
        let [outcome] = completed.outcomes.as_slice() else {
            panic!("the scene has 1 target");
        };
        assert_eq!(outcome.report.fixes.len(), 2);
        assert!(
            outcome
                .report
                .fixes
                .iter()
                .all(|fix| matches!(fix, MeshFix::RemovedCollinearFaceByMerge { .. }))
        );
        assert_eq!(
            outcome.report.end,
            RepairEnd::Completed(MeshStateAfterRepair {
                edge_manifold: true,
                zero_area_face_count: 0,
                holes_left_open: 0,
            })
        );
        let names = target_names(&completed.outcomes, &graph);
        let details = details_text(&completed.outcomes, &names, completed.elapsed);
        assert!(details.starts_with("Object 1: repaired\n"), "{details}");
        assert!(
            details.contains("\n• Removed 2 zero-area slivers\n"),
            "{details}"
        );
    }

    /// A box of `n` x `n` quads a side and 1 mm quads, as a badly exported
    /// file has it: no vertex shared, a face turned inside out every so
    /// often, and a hole in its bottom.
    fn broken_box(n: u32) -> IndexedMesh {
        let s = n as f32;
        let mut vertices = Vec::new();
        let mut faces = Vec::new();
        let sides: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
            ([0.0, 0.0, s], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            ([0.0, s, 0.0], [1.0, 0.0, 0.0], [0.0, -1.0, 0.0]),
            ([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            ([s, s, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            ([s, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
            ([0.0, s, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]),
        ];
        let mut count = 0u32;
        for (side, (o, u, v)) in sides.iter().enumerate() {
            for j in 0..n {
                for i in 0..n {
                    // The hole: the middle of the bottom.
                    if side == 1 && i.abs_diff(n / 2) < 2 && j.abs_diff(n / 2) < 2 {
                        continue;
                    }
                    let at = |a: u32, b: u32| {
                        Vertex::new(std::array::from_fn(|k| {
                            o[k] + u[k] * a as f32 + v[k] * b as f32
                        }))
                    };
                    for t in [
                        [at(i, j), at(i + 1, j), at(i + 1, j + 1)],
                        [at(i, j), at(i + 1, j + 1), at(i, j + 1)],
                    ] {
                        let base = vertices.len() as u32;
                        vertices.extend(t);
                        count += 1;
                        let flip = count.is_multiple_of(37);
                        faces.push(IndexedTriangle::from_raw_indices(if flip {
                            [base, base + 2, base + 1]
                        } else {
                            [base, base + 1, base + 2]
                        }));
                    }
                }
            }
        }
        IndexedMesh {
            meta: None,
            vertices,
            faces,
        }
    }

    /// Repair rewrites this mesh: most faces get new corners, some turn over
    /// and a hole is filled. The paint follows the surface, the run says what
    /// it kept, and one Undo brings all of it back.
    #[test]
    fn the_paint_survives_a_real_repair_and_says_what_it_kept() {
        on_fresh_slint_thread(|| {
            let mesh = Arc::new(broken_box(24));
            let mut graph = Scene::new();
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Identity,
                    LeafPayload::Mesh(Arc::clone(&mesh)),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            let doc = SceneDocument::new();
            doc.install_graph(graph, PathBuf::from("/tmp/broken-box.stl"));
            // The top and the part of one wall near it.
            let values: Vec<u8> = mesh
                .faces()
                .iter()
                .map(|f| {
                    let low = f
                        .vertices
                        .iter()
                        .map(|&v| mesh.vertices()[usize::from(v)].coords()[2])
                        .fold(f32::INFINITY, f32::min);
                    if low >= 20.0 { PAINTED } else { UNPAINTED }
                })
                .collect();
            let paint = SurfaceTags::new(mesh.clone(), values).unwrap();
            let painted_area = paint.painted_area();
            let set = SceneEdit::SetSurfaceTags {
                node_id: leaf,
                kind: PaintKind::Detail,
                before: None,
                after: Some(paint),
            };
            doc.apply_edit(set.clone()).unwrap();
            doc.push_edit(set);

            let captured = doc.scene_revision();
            let completed = doc
                .with_scene(|spatial| run_repair(spatial.graph().clone()))
                .unwrap();
            let replacement = &completed.replacements[0];
            let repaired = Arc::clone(&replacement.after);
            let rewritten = repaired
                .faces()
                .iter()
                .zip(mesh.faces())
                .filter(|(a, b)| a.vertices != b.vertices)
                .count();
            assert!(
                rewritten > mesh.faces().len() / 2,
                "{rewritten} faces changed"
            );
            assert!(
                repaired.faces().len() > mesh.faces().len(),
                "the hole filled"
            );
            assert_eq!(
                settle(&doc, &completed, captured),
                RepairSettlement::Applied
            );

            // Worked out on the run's worker, not in the push.
            assert_eq!(completed.paint.len(), 1);
            let report = doc.take_paint_report();
            assert_eq!(report.len(), 1);
            let carried = doc
                .surface_tags(leaf, PaintKind::Detail)
                .expect("the paint came along");
            assert!(Arc::ptr_eq(carried.mesh(), &repaired));
            assert!(
                (carried.painted_area() - painted_area).abs() < painted_area * 0.005,
                "{} of {painted_area}",
                carried.painted_area()
            );
            assert!((report[0].area_before - painted_area).abs() < 1e-6);
            assert!((report[0].area_after - carried.painted_area()).abs() < 1e-6);
            assert!(lost_paint_note(&report).is_none());
            // On the same surface: nothing below where it was painted.
            for (face, value) in repaired.faces().iter().zip(carried.values()) {
                let high = face
                    .vertices
                    .iter()
                    .map(|&v| repaired.vertices()[usize::from(v)].coords()[2])
                    .fold(f32::NEG_INFINITY, f32::max);
                assert!(*value == UNPAINTED || high >= 20.0, "paint at z {high}");
            }
            // One step back: the mesh and its paint.
            doc.undo().unwrap();
            let back = doc.surface_tags(leaf, PaintKind::Detail).expect("back");
            assert!(Arc::ptr_eq(back.mesh(), &mesh));
            assert!((back.painted_area() - painted_area).abs() < 1e-9);
            doc.redo().unwrap();
            let again = doc.surface_tags(leaf, PaintKind::Detail).expect("again");
            assert!(Arc::ptr_eq(again.mesh(), &repaired));
        });
    }

    #[test]
    fn settle_applies_one_undo_unit_and_undo_restores_the_arcs() {
        let doc = SceneDocument::new();
        doc.install_graph(
            broken_trio_scene(),
            PathBuf::from("/tmp/repair-run-fixture.obj"),
        );
        let captured = doc.scene_revision();
        let originals: Vec<(MeshTarget, Arc<IndexedMesh>)> = doc
            .with_scene(|spatial| {
                let graph = spatial.graph();
                graph
                    .mesh_targets()
                    .into_iter()
                    .filter_map(|t| Some((t, target_arc(graph, t)?)))
                    .collect()
            })
            .unwrap();
        assert!(!doc.with_scene(|s| scene_slices(s.graph())).unwrap());

        let completed = doc
            .with_scene(|spatial| run_repair(spatial.graph().clone()))
            .unwrap();
        assert_eq!(
            settle(&doc, &completed, captured),
            RepairSettlement::Applied
        );
        assert!(doc.with_scene(|s| scene_slices(s.graph())).unwrap());
        // Each repaired mesh came with its pick structure built on the
        // worker (#395): the scene took it instead of building its own, and
        // it is the one a click would have built.
        assert_eq!(completed.primed.len(), completed.replacements.len());
        let taken = doc
            .with_scene(|spatial| {
                let mut taken = 0;
                for id in (0..).map_while(|i| spatial.leaf_id_at_index(i)) {
                    let query = spatial.leaf(id).expect("a live leaf");
                    let Some(primed) = completed
                        .primed
                        .iter()
                        .find(|p| std::ptr::eq(p.mesh().as_ref(), query.mesh()))
                    else {
                        continue;
                    };
                    let clusters = query.clusters();
                    assert!(std::ptr::eq(
                        clusters.expect("faces"),
                        primed.clusters().expect("faces")
                    ));
                    assert_eq!(
                        clusters,
                        sc_mesh_spatial::TriangleClusters::for_mesh(query.mesh()).as_ref()
                    );
                    taken += 1;
                }
                taken
            })
            .unwrap();
        // 3 meshes repaired; the resource one shows on 2 leaves.
        assert_eq!(taken, 4);
        // The apply moved the revision: a slice captured before the repair
        // reads as stale.
        assert!(doc.slice_geometry_changed(captured));

        assert!(doc.undo().unwrap());
        assert!(!doc.with_scene(|s| scene_slices(s.graph())).unwrap());
        doc.with_scene(|spatial| {
            let graph = spatial.graph();
            for (target, original) in &originals {
                let live = target_arc(graph, *target).expect("live target");
                assert!(
                    Arc::ptr_eq(&live, original),
                    "undo must restore the original Arc for {target:?}"
                );
            }
        })
        .unwrap();
    }

    #[test]
    fn settle_discards_a_stale_result_without_applying() {
        let doc = SceneDocument::new();
        doc.install_graph(
            broken_trio_scene(),
            PathBuf::from("/tmp/repair-run-fixture.obj"),
        );
        let captured = doc.scene_revision();
        let completed = doc
            .with_scene(|spatial| run_repair(spatial.graph().clone()))
            .unwrap();
        // The scene moves on while the worker runs.
        let leaf = doc.with_scene(|s| s.leaf_id_at_index(0).unwrap()).unwrap();
        let edit = SceneEdit::SetLocalTransform {
            node_id: leaf,
            before: Transform3D::Identity,
            after: Transform3D::Affine(
                sc_mesh_core::transform::AffineTransform3D::from_translation([1.0, 0.0, 0.0]),
            ),
        };
        doc.apply_edit(edit.clone()).unwrap();
        doc.push_edit(edit);

        assert_eq!(
            settle(&doc, &completed, captured),
            RepairSettlement::DiscardedStale
        );
        assert!(
            !doc.with_scene(|s| scene_slices(s.graph())).unwrap(),
            "a discarded result must not touch the scene"
        );
    }

    #[test]
    fn settle_discards_a_result_whose_scene_was_replaced() {
        let doc = SceneDocument::new();
        doc.install_graph(
            broken_trio_scene(),
            PathBuf::from("/tmp/repair-run-fixture.obj"),
        );
        let captured = doc.scene_revision();
        let completed = doc
            .with_scene(|spatial| run_repair(spatial.graph().clone()))
            .unwrap();
        // The user opens another file while the worker runs. The replacement
        // numbers its targets exactly like the outgoing scene, so nothing but
        // the scene's identity separates them.
        doc.install_graph(
            broken_trio_scene(),
            PathBuf::from("/tmp/repair-run-other.obj"),
        );
        let installed: Vec<(MeshTarget, Arc<IndexedMesh>)> = doc
            .with_scene(|spatial| {
                let graph = spatial.graph();
                graph
                    .mesh_targets()
                    .into_iter()
                    .filter_map(|t| Some((t, target_arc(graph, t)?)))
                    .collect()
            })
            .unwrap();

        assert_eq!(
            settle(&doc, &completed, captured),
            RepairSettlement::DiscardedStale
        );
        assert!(
            !doc.with_scene(|s| scene_slices(s.graph())).unwrap(),
            "the freshly opened scene must keep its own geometry"
        );
        doc.with_scene(|spatial| {
            let graph = spatial.graph();
            for (target, installed) in &installed {
                let live = target_arc(graph, *target).expect("live target");
                assert!(
                    Arc::ptr_eq(&live, installed),
                    "a discarded result must not touch {target:?}"
                );
            }
        })
        .unwrap();
        assert!(!doc.can_undo(), "nothing was recorded on the new scene");
    }

    /// A window with `graph` open and the Repair driver installed.
    struct Window {
        window: MainWindow,
        document: Rc<SceneDocument>,
        run: Rc<RepairRun>,
    }

    fn window_with(graph: Scene) -> Window {
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let ctrl = Rc::new(ViewportController::new_for_test());
        let document = ctrl.document();
        document.install_graph(graph, PathBuf::from("/tmp/repair-run-window.obj"));
        window.set_workspace_section(WorkspaceSection::Transform);
        let run = RepairRun::install(&window, document.clone(), ctrl.dialogs(), ctrl);
        Window {
            window,
            document,
            run,
        }
    }

    /// The slot of the run in flight.
    fn slot_of(run: &RepairRun) -> RepairSlot {
        run.job
            .borrow()
            .as_ref()
            .expect("a run in flight")
            .slot
            .clone()
    }

    /// Waits until the worker that writes `slot` is done.
    fn wait_for(slot: &RepairSlot) {
        let start = Instant::now();
        while slot
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
    }

    /// The `Arc` of every target of the live scene.
    fn live_arcs(document: &SceneDocument) -> Vec<Arc<IndexedMesh>> {
        document
            .with_scene(|spatial| {
                let graph = spatial.graph();
                graph
                    .mesh_targets()
                    .into_iter()
                    .filter_map(|t| target_arc(graph, t))
                    .collect()
            })
            .unwrap()
    }

    /// A flat plate: a hole over the fill cap, left open.
    fn plate() -> IndexedMesh {
        let vertices = [
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [0.0, 10.0, 0.0],
            [10.0, 10.0, 0.0],
        ]
        .map(Vertex::new)
        .to_vec();
        let faces = [[0, 1, 2], [2, 1, 3]]
            .map(IndexedTriangle::from_raw_indices)
            .to_vec();
        IndexedMesh::new(None, vertices, faces)
    }

    /// Pins Cancel: the card is released at once, the scene keeps every mesh
    /// it had and nothing reaches the history; the late wake-up of the
    /// cancelled worker does nothing, not even to the next run.
    #[test]
    fn a_cancelled_run_leaves_the_scene_as_it_was_and_its_wake_up_is_ignored() {
        on_fresh_slint_thread(|| {
            let w = window_with(broken_trio_scene());
            let before = live_arcs(&w.document);

            w.run.start();
            assert!(w.window.get_repairing_in_progress());
            let cancelled = slot_of(&w.run);
            w.run.cancel();
            assert!(!w.window.get_repairing_in_progress());
            assert!(w.run.job.borrow().is_none());

            // A new run starts while the cancelled worker may still go.
            w.run.start();
            assert!(w.window.get_repairing_in_progress());
            let next = slot_of(&w.run);
            wait_for(&cancelled);
            wait_for(&next);
            // The cancelled worker's wake-up, with the next run's result
            // held back: the next run goes on, the scene is untouched.
            let held = next.lock().unwrap().take();
            w.run.finish();
            assert!(w.window.get_repairing_in_progress());
            assert!(w.run.job.borrow().is_some());
            let after_cancel = live_arcs(&w.document);
            assert_eq!(after_cancel.len(), before.len());
            for (a, b) in after_cancel.iter().zip(&before) {
                assert!(Arc::ptr_eq(a, b), "a cancelled run changed a mesh");
            }
            assert!(!w.document.can_undo());

            // The next run's own wake-up applies it, as 1 undo step.
            *next.lock().unwrap() = held;
            w.run.finish();
            assert!(!w.window.get_repairing_in_progress());
            assert!(w.document.with_scene(|s| scene_slices(s.graph())).unwrap());
            assert!(w.document.can_undo());
        });
    }

    /// Pins the worker side of Cancel: a run told to stop returns no result
    /// and leaves the scene it was handed as it was.
    #[test]
    fn a_cancelled_worker_returns_nothing() {
        let scene = broken_trio_scene();
        let watch = RunWatch::default();
        watch.cancel.store(true, Ordering::Relaxed);
        let targets = scene.mesh_targets();
        assert!(run_repair_watched(scene.clone(), &targets, &watch, &[]).is_err());
        assert!(
            watch.progress.lock().unwrap().is_none(),
            "a stopped run tells no step"
        );
    }

    /// Pins the card's progress line: the step, and with several objects,
    /// which one.
    /// A box 20 × 10 × 2 mm, its underside `z` up in its own frame.
    fn box_at(z: f32) -> IndexedMesh {
        let cube = unit_cube_mesh();
        let vertices = cube
            .vertices()
            .iter()
            .map(|v| {
                let p = v.as_array();
                Vertex::new([
                    (p[0] + 0.5) * 20.0,
                    (p[1] + 0.5) * 10.0,
                    (p[2] + 0.5) * 2.0 + z,
                ])
            })
            .collect();
        IndexedMesh::new(None, vertices, cube.faces().to_vec())
    }

    /// The set of a box 9 mm up, and the model's leaf; then the mended sets
    /// a repair that gave the box `mended` leaves (#477).
    fn mended_with(mended: IndexedMesh) -> Vec<MendedSet> {
        let mut graph = Scene::new();
        let model = graph
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(box_at(9.0))),
            )
            .expect("leaf");
        graph.add_root(model).expect("root");
        let outcome = sc_geom::df_supports::start_df_supports(
            &graph,
            sc_geom::df_supports::DfSupportRequest::default(),
        )
        .expect("the run starts")
        .join()
        .expect("supports");
        let session = Arc::new(outcome.sets.into_iter().next().expect("a set").session);
        let before = graph.leaf_mesh_arc(model).expect("its mesh").clone();
        let after = Arc::new(mended);
        let mut repaired = graph.clone();
        repaired
            .replace_inline_leaf_mesh(model, &before, after.clone())
            .expect("the repaired mesh");
        // The supports' own leaf, which the repair left alone.
        let supports = repaired
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
            )
            .expect("leaf");
        repaired.add_root(supports).expect("root");
        let replacement = MeshReplacement {
            target: MeshTarget::InlineLeaf(model),
            before,
            after,
        };
        let held = [HeldSet {
            leaf: supports,
            model,
            session,
        }];
        mended_sets(&repaired, &[replacement], &held)
    }

    /// #477: a repair that leaves the box where its tips are keeps its set
    /// in force; one that moves the underside under them does not.
    #[test]
    fn a_set_stays_in_force_while_its_tips_still_touch_the_repaired_model() {
        let kept = mended_with(box_at(9.0));
        assert_eq!(kept.len(), 1);
        assert!(kept[0].session.is_some(), "the tips still touch");
        let moved = mended_with(box_at(9.5));
        assert_eq!(moved.len(), 1);
        assert!(moved[0].session.is_none(), "the underside left the tips");
    }

    #[test]
    fn the_progress_line_names_the_step_and_the_object() {
        let at = |target, targets| SceneRepairProgress {
            target,
            targets,
            step: RepairStep::FillingHoles,
            done: 0.5,
        };
        let names = ["Wolf".to_string(), "Base".to_string()];
        assert_eq!(phase_text(&at(0, 1), &names), "Filling holes…");
        assert_eq!(
            phase_text(&at(1, 2), &names),
            "Filling holes… — Base (2 of 2)"
        );
    }

    /// Pins the marks: after a run that leaves a plate open, the viewport
    /// marks its outline, follows the object when it moves, hides the marks
    /// when the card's box is off, and drops them with the mesh.
    #[test]
    fn what_a_run_leaves_is_marked_and_follows_the_scene() {
        on_fresh_slint_thread(|| {
            let mut graph = Scene::new();
            let leaf = graph
                .add_leaf(
                    None,
                    Transform3D::Identity,
                    LeafPayload::Mesh(Arc::new(plate())),
                )
                .unwrap();
            graph.add_root(leaf).unwrap();
            let w = window_with(graph);
            assert!(!w.window.get_repair_marks_available());

            w.run.start();
            wait_for(&slot_of(&w.run));
            w.run.finish();
            assert!(w.window.get_repair_marks_available());
            assert!(w.window.get_repair_marks_shown());
            let shown = w.run.shown_marks.borrow().clone().expect("marks shown");
            assert_eq!(shown.len(), 1);
            assert_eq!(shown[0].0, leaf);
            assert_eq!(shown[0].2, Transform3D::Identity);

            // The object moves: the marks go with it.
            let moved = Transform3D::Affine(
                sc_mesh_core::transform::AffineTransform3D::from_translation([5.0, 0.0, 0.0]),
            );
            let edit = SceneEdit::SetLocalTransform {
                node_id: leaf,
                before: Transform3D::Identity,
                after: moved,
            };
            w.document.apply_edit(edit.clone()).unwrap();
            w.document.push_edit(edit);
            let shown = w.run.shown_marks.borrow().clone().expect("marks shown");
            assert_eq!(shown[0].2, moved);

            // The box off hides them; the marks stay known.
            w.window.invoke_repair_marks_toggled(false);
            assert!(w.run.shown_marks.borrow().is_none());
            assert!(w.window.get_repair_marks_available());
            assert!(!w.window.get_repair_marks_shown());
            w.window.invoke_repair_marks_toggled(true);
            assert!(w.run.shown_marks.borrow().is_some());

            // Another scene: no marks.
            w.document
                .install_graph(Scene::new(), PathBuf::from("/tmp/empty.obj"));
            assert!(!w.window.get_repair_marks_available());
            assert!(
                w.run
                    .shown_marks
                    .borrow()
                    .as_ref()
                    .is_none_or(Vec::is_empty)
            );
        });
    }

    /// Pins that a run that leaves nothing broken marks nothing.
    #[test]
    fn a_run_that_fixes_everything_marks_nothing() {
        on_fresh_slint_thread(|| {
            let w = window_with(broken_trio_scene());
            w.run.start();
            wait_for(&slot_of(&w.run));
            w.run.finish();
            assert!(!w.window.get_repair_marks_available());
        });
    }
}
