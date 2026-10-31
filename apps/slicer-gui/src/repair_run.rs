/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Per-window driver for the Repair action.
//!
//! The Repair button clones the scene on the UI thread and runs the mesh-core
//! repair policy on a worker thread so the event loop stays responsive; the
//! finished result is deposited into a slot and picked up on the UI thread via
//! `invoke_repair_finished`. No progress bar or cancel: the run is 1 validation
//! pass per mesh, the same cost the load worker absorbs silently. The notice
//! that follows shows the report of `repair_report`.
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

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::indexed_mesh::RepairPasses;
use sc_mesh_core::indexed_mesh::repair::RepairEnd;
use sc_scene::mesh_targets::MeshTarget;
use sc_scene::repair::SceneRepairOutcome;
use sc_scene::{Scene, SceneError};
use sc_scene_edit::SceneEdit;
use slint::ComponentHandle;

use crate::MainWindow;
use crate::error_dialog::{DialogManager, ErrorTexts, InfoTexts, MessageKey};
use crate::i18n::ctr;
use crate::mesh_check::{show_card, target_arc};
use crate::mesh_loader::panic_message;
use crate::repair_report::{details_text, left_reason, summary_message, target_names};
use crate::scene_document::{
    EditNotApplied, SceneDocument, SceneEvent, SceneRevision, SceneSubscription,
};
use crate::workspace_section::allows_repair;

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
    /// How long the whole run took on the worker.
    elapsed: Duration,
}

type RepairSlot = Arc<Mutex<Option<Result<CompletedRepair, String>>>>;

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

pub(crate) struct RepairRun {
    ui: slint::Weak<MainWindow>,
    document: Rc<SceneDocument>,
    dialogs: Rc<DialogManager>,
    slot: RepairSlot,
    // Revision captured at launch; `None` when no run is in flight.
    pending_revision: RefCell<Option<SceneRevision>>,
    // Keeps the MESH CHECK card following the scene.
    _scene_events: SceneSubscription,
}

impl RepairRun {
    /// Wires the Repair / finished callbacks and returns the driver, which the
    /// caller keeps alive for the window's lifetime.
    pub(crate) fn install(
        ui: &MainWindow,
        document: Rc<SceneDocument>,
        dialogs: Rc<DialogManager>,
    ) -> Rc<Self> {
        let scene_events = {
            let ui = ui.as_weak();
            let weak_document = Rc::downgrade(&document);
            document.subscribe(move |event| {
                if let Some(document) = weak_document.upgrade() {
                    follow_scene(&ui, &document, event);
                }
            })
        };
        let run = Rc::new(Self {
            ui: ui.as_weak(),
            document,
            dialogs,
            slot: Arc::new(Mutex::new(None)),
            pending_revision: RefCell::new(None),
            _scene_events: scene_events,
        });
        {
            let run = run.clone();
            ui.on_repair_requested(move || run.start());
        }
        {
            let run = run.clone();
            ui.on_repair_finished(move || run.finish());
        }
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
        let Some(scene) = self.document.with_scene(|spatial| spatial.graph().clone()) else {
            return;
        };
        *self.pending_revision.borrow_mut() = Some(self.document.scene_revision());
        ui.set_repairing_in_progress(true);

        let slot = self.slot.clone();
        let ui_weak = self.ui.clone();
        std::thread::spawn(move || {
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_repair(scene)))
                    .map_err(|payload| panic_message(&payload));
            *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(result);
            let _ = ui_weak.upgrade_in_event_loop(|ui| ui.invoke_repair_finished());
        });
    }

    /// Drains the deposited result on the UI thread, applies it when it is
    /// still current, and reports through the info dialog. The tab stays
    /// Transform; its repair card follows the scene's new health.
    fn finish(&self) {
        let taken = self
            .slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let captured = self.pending_revision.borrow_mut().take();
        if let Some(ui) = self.ui.upgrade() {
            ui.set_repairing_in_progress(false);
        }
        let (Some(result), Some(captured)) = (taken, captured) else {
            return;
        };
        let completed = match result {
            Ok(completed) => completed,
            Err(panic) => {
                self.dialogs.show_error(move || ErrorTexts {
                    heading: ctr!("RepairDialog", "Repair failed"),
                    context: None,
                    message: ctr!("RepairDialog", "The repair worker crashed."),
                    details: panic.as_str().into(),
                });
                return;
            }
        };
        match settle(&self.document, &completed, captured) {
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
                let CompletedRepair {
                    outcomes, elapsed, ..
                } = completed;
                // The Transform tab's repair card follows what the run left:
                // the live scene now holds the meshes the run ended with.
                let check = self.document.mesh_check();
                let (names, card) = self
                    .document
                    .with_scene(|spatial| {
                        let graph = spatial.graph();
                        for outcome in &outcomes {
                            if let Some(mesh) = target_arc(graph, outcome.target) {
                                let sliceable = outcome.report.end == RepairEnd::NoFaces
                                    || outcome.details.after.sliceable;
                                check.remember(&mesh, sliceable, left_reason(outcome));
                            }
                        }
                        (target_names(&outcomes, graph), check.card(graph, true))
                    })
                    .unwrap_or_default();
                if let (Some(ui), Some(card)) = (self.ui.upgrade(), card) {
                    show_card(&ui, &card);
                }
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
}

/// Repairs every stored mesh of the cloned scene, on the worker, and derives
/// the replacements from the Arc diff. The clone shares its mesh storage with
/// the live scene, so an Arc that survived the run unchanged proves the live
/// mesh needs no replacing.
fn run_repair(mut scene: Scene) -> CompletedRepair {
    let start = Instant::now();
    let before: Vec<(MeshTarget, Arc<IndexedMesh>)> = scene
        .mesh_targets()
        .into_iter()
        .filter_map(|target| Some((target, target_arc(&scene, target)?)))
        .collect();
    let outcomes = scene.repair_meshes(RepairPasses::default_repair());
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
        .collect();
    CompletedRepair {
        outcomes,
        replacements,
        elapsed: start.elapsed(),
    }
}

/// Keeps the MESH CHECK card on what the scene holds now: an undo that puts
/// a broken mesh back, a deleted object. A mesh with no verdict is checked
/// only while the card shows a problem (see `mesh_check`).
fn follow_scene(ui: &slint::Weak<MainWindow>, document: &SceneDocument, event: SceneEvent) {
    let Some(ui) = ui.upgrade() else {
        return;
    };
    match event {
        SceneEvent::Mutated | SceneEvent::Restructured | SceneEvent::UndoStackChanged => {
            let check_unknown = ui.get_mesh_needs_repair();
            let card = document
                .with_scene(|spatial| document.mesh_check().card(spatial.graph(), check_unknown))
                .flatten();
            if let Some(card) = card {
                show_card(&ui, &card);
            }
        }
        SceneEvent::Cleared => {
            document.mesh_check().forget_all();
            show_card(&ui, &Default::default());
        }
        // A new scene gets its card from the load worker's verdicts.
        SceneEvent::Installed
        | SceneEvent::SelectionChanged
        | SceneEvent::SliceChanged
        | SceneEvent::DirtyChanged => {}
    }
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
    match document.apply_edit(edit.clone()) {
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
}
