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

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

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
    EditNotApplied, SceneDocument, SceneEvent, SceneRevision, SceneSubscription,
};
use crate::viewport::ViewportController;
use crate::workspace_section::allows_repair;

/// How often the card reads the worker's progress.
const PROGRESS_POLL_INTERVAL: Duration = Duration::from_millis(100);

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
    /// What each target that still does not slice has left, to mark on the
    /// model.
    marks: Vec<(MeshTarget, Arc<RepairMarks>)>,
    /// How long the whole run took on the worker.
    elapsed: Duration,
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
        if ui.get_loading() || ui.get_slicing_in_progress() || ui.get_repairing_in_progress() {
            return;
        }
        if !allows_repair(ui.get_workspace_section()) {
            return;
        }
        let Some((scene, names)) = self.document.with_scene(|spatial| {
            let graph = spatial.graph();
            (graph.clone(), names_in_run_order(graph))
        }) else {
            return;
        };
        let slot: RepairSlot = Arc::new(Mutex::new(None));
        let watch = Arc::new(RunWatch::default());
        *self.job.borrow_mut() = Some(Job {
            slot: slot.clone(),
            watch: watch.clone(),
            revision: self.document.scene_revision(),
            names,
        });
        ui.set_repair_progress(0.0);
        ui.set_repair_phase(Default::default());
        ui.set_repairing_in_progress(true);
        crate::breadcrumbs::started("repair");
        self.start_poll();

        let ui_weak = self.ui.clone();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_repair_watched(scene, watch.as_ref())
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
        match settle(&self.document, &completed, job.revision) {
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
                let CompletedRepair {
                    outcomes, elapsed, ..
                } = completed;
                self.dialogs
                    .show_notice(MessageKey::MeshRepairSummary, move || InfoTexts {
                        heading: ctr!("RepairDialog", "Mesh repair"),
                        message: summary_message(&outcomes, &names).into(),
                        details: details_text(&outcomes, &names, elapsed).into(),
                    });
            }
            RepairSettlement::Rejected(error) => {
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("RepairDialog", "Repair failed"),
                    context: None,
                    message: ctr!(
                        "RepairDialog",
                        "The scene rejected the repair edit; nothing was changed."
                    ),
                    details: error.to_string().into(),
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
            SceneEvent::SelectionChanged | SceneEvent::SliceChanged | SceneEvent::DirtyChanged => {}
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

/// The object-list name of each target of `graph`, in the order a repair
/// run walks them.
fn names_in_run_order(graph: &Scene) -> Vec<String> {
    let by_leaf = object_names_by_leaf(graph);
    graph
        .mesh_targets()
        .into_iter()
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
    match run_repair_watched(scene, &NoWatch) {
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

/// [`run_repair`] told to `watch`; a cancelled run returns nothing.
fn run_repair_watched(
    mut scene: Scene,
    watch: &dyn SceneRepairWatch,
) -> Result<CompletedRepair, RepairCancelled> {
    let start = Instant::now();
    let before: Vec<(MeshTarget, Arc<IndexedMesh>)> = scene
        .mesh_targets()
        .into_iter()
        .filter_map(|target| Some((target, target_arc(&scene, target)?)))
        .collect();
    let outcomes = scene.repair_meshes_watched(RepairPasses::default_repair(), watch)?;
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
    let marks = outcomes
        .iter()
        .filter_map(|outcome| {
            let leftovers = outcome.details.leftovers.as_ref()?;
            let mesh = target_arc(&scene, outcome.target)?;
            let marks = RepairMarks::of(&mesh, leftovers)?;
            Some((outcome.target, Arc::new(marks)))
        })
        .collect();
    Ok(CompletedRepair {
        outcomes,
        replacements,
        primed,
        marks,
        elapsed: start.elapsed(),
    })
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
    let edit = SceneEdit::Compound(
        completed
            .replacements
            .iter()
            .map(MeshReplacement::as_edit)
            .collect(),
    );
    match document.apply_edit_primed(edit.clone(), &completed.primed) {
        Ok(()) => {
            document.push_edit(edit);
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
    use sc_mesh_core::transform::Transform3D;
    use sc_mesh_fixtures::{
        cuboid_with_y_tunnel_and_flipped_faces, cuboid_with_z_tunnel_and_flipped_faces,
        unit_cube_mesh, unit_cube_with_collinear_faces, unit_cube_with_flipped_faces,
    };
    use sc_scene::node::LeafPayload;

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
        window.set_workspace_section(crate::WorkspaceSection::Transform);
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
        .map(sc_mesh_core::primitives::Vertex::new)
        .to_vec();
        let faces = [[0, 1, 2], [2, 1, 3]]
            .map(sc_mesh_core::primitives::IndexedTriangle::from_raw_indices)
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
        assert!(run_repair_watched(scene.clone(), &watch).is_err());
        assert!(
            watch.progress.lock().unwrap().is_none(),
            "a stopped run tells no step"
        );
    }

    /// Pins the card's progress line: the step, and with several objects,
    /// which one.
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
