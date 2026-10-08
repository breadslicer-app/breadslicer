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
//! pass per mesh, the same cost the load worker absorbs silently.
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

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::indexed_mesh::{MeshFix, RepairPasses};
use sc_scene::mesh_targets::MeshTarget;
use sc_scene::repair::SceneRepairOutcome;
use sc_scene::{Scene, SceneError};
use sc_scene_edit::SceneEdit;
use slint::ComponentHandle;

use crate::MainWindow;
use crate::error_dialog::DialogManager;
use crate::i18n::ctr;
use crate::mesh_loader::panic_message;
use crate::scene_document::{EditNotApplied, SceneDocument, SceneRevision};
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
}

impl RepairRun {
    /// Wires the Repair / finished callbacks and returns the driver, which the
    /// caller keeps alive for the window's lifetime.
    pub(crate) fn install(
        ui: &MainWindow,
        document: Rc<SceneDocument>,
        dialogs: Rc<DialogManager>,
    ) -> Rc<Self> {
        let run = Rc::new(Self {
            ui: ui.as_weak(),
            document,
            dialogs,
            slot: Arc::new(Mutex::new(None)),
            pending_revision: RefCell::new(None),
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
    /// still current, and reports through the info dialog. The section
    /// deliberately stays Prepare: the user switches via the pill once they
    /// have read the results.
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
                self.dialogs.show_error(
                    &ctr!("RepairDialog", "Repair failed"),
                    None,
                    ctr!("RepairDialog", "The repair worker crashed.").to_string(),
                    panic,
                );
                return;
            }
        };
        match settle(&self.document, &completed, captured) {
            RepairSettlement::DiscardedStale => {
                self.dialogs.show_info(
                    &ctr!("RepairDialog", "Repair discarded"),
                    ctr!(
                        "RepairDialog",
                        "The scene changed while repairing; nothing was applied."
                    )
                    .to_string(),
                    String::new(),
                );
            }
            RepairSettlement::NothingApplied | RepairSettlement::Applied => {
                let details = self
                    .document
                    .with_scene(|spatial| dialog_lines(&completed.outcomes, spatial.graph()))
                    .unwrap_or_default()
                    .join("\n");
                self.dialogs.show_info(
                    &ctr!("RepairDialog", "Mesh repair"),
                    summary_message(&completed.outcomes),
                    details,
                );
            }
            RepairSettlement::Rejected(error) => {
                self.dialogs.show_error(
                    &ctr!("RepairDialog", "Repair failed"),
                    None,
                    ctr!(
                        "RepairDialog",
                        "The scene rejected the repair edit; nothing was changed."
                    )
                    .to_string(),
                    error.to_string(),
                );
            }
        }
    }
}

/// Repairs every stored mesh of the cloned scene, on the worker, and derives
/// the replacements from the Arc diff. The clone shares its mesh storage with
/// the live scene, so an Arc that survived the run unchanged proves the live
/// mesh needs no replacing.
fn run_repair(mut scene: Scene) -> CompletedRepair {
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
    }
}

/// Returns a clone of the `Arc` stored at `target`, or `None` for a dead
/// target.
fn target_arc(scene: &Scene, target: MeshTarget) -> Option<Arc<IndexedMesh>> {
    match target {
        MeshTarget::Resource(rid) => scene.mesh_resource_arc(rid).cloned(),
        MeshTarget::InlineLeaf(id) => scene.leaf_mesh_arc(id).cloned(),
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

/// Builds the 1-line rollup for the dialog's message.
fn summary_message(outcomes: &[SceneRepairOutcome]) -> String {
    let repaired = outcomes
        .iter()
        .filter(|o| !o.fixes.is_empty() && o.refusal.is_none())
        .count();
    let refused = outcomes.iter().filter(|o| o.refusal.is_some()).count();
    let still_holed = outcomes
        .iter()
        .filter(|o| o.refusal.is_none() && !o.edge_manifold_after)
        .count();
    if repaired == 0 && refused == 0 && still_holed == 0 {
        return ctr!("RepairDialog", "No problems found.").to_string();
    }
    let mut parts: Vec<String> = Vec::new();
    if repaired > 0 {
        parts.push(ctr!("RepairDialog", "{} mesh(es) repaired", repaired).to_string());
    }
    if refused > 0 {
        parts.push(
            ctr!(
                "RepairDialog",
                "{} mesh(es) could not be fully repaired",
                refused
            )
            .to_string(),
        );
    }
    if still_holed > 0 {
        parts.push(
            ctr!(
                "RepairDialog",
                "{} mesh(es) still not edge-manifold",
                still_holed
            )
            .to_string(),
        );
    }
    format!("{}.", parts.join(", "))
}

/// Builds the per-mesh detail lines: 1 line per target with something to say
/// (fixes, a refusal, or a remaining edge-manifold failure); clean targets stay
/// silent.
fn dialog_lines(outcomes: &[SceneRepairOutcome], graph: &Scene) -> Vec<String> {
    outcomes
        .iter()
        .enumerate()
        .filter_map(|(i, outcome)| {
            let mut notes: Vec<String> = Vec::new();
            let mut removed = 0usize;
            let mut reoriented = 0usize;
            for fix in &outcome.fixes {
                match fix {
                    MeshFix::RemovedZeroLengthEdgeFace { .. } => removed += 1,
                    MeshFix::FlippedFaceOrientation { .. } => reoriented += 1,
                }
            }
            if removed > 0 {
                notes.push(
                    ctr!("RepairDialog", "removed {} degenerate face(s)", removed).to_string(),
                );
            }
            if reoriented > 0 {
                notes.push(ctr!("RepairDialog", "reoriented {} face(s)", reoriented).to_string());
            }
            if let Some(refusal) = &outcome.refusal {
                notes.push(
                    ctr!("RepairDialog", "could not be fully repaired: {}", refusal).to_string(),
                );
            }
            if outcome.refusal.is_none() && !outcome.edge_manifold_after {
                notes.push(ctr!("RepairDialog", "still not edge-manifold").to_string());
            }
            if notes.is_empty() {
                return None;
            }
            Some(format!(
                "{}: {}",
                target_display_name(graph, outcome.target, i),
                notes.join("; ")
            ))
        })
        .collect()
}

/// Returns the display name of a target: the first referencing leaf's scene
/// name, else the mesh's own name, else a 1-based fallback, the same resolution
/// order the object list uses.
fn target_display_name(graph: &Scene, target: MeshTarget, ordinal: usize) -> String {
    let node_name = graph
        .leaves_referencing(target)
        .into_iter()
        .next()
        .and_then(|id| graph.node(id))
        .and_then(|node| node.meta.as_ref())
        .and_then(|meta| meta.name.clone());
    let mesh_name = graph
        .target_mesh(target)
        .and_then(|mesh| mesh.meta().as_ref())
        .and_then(|meta| meta.name.clone());
    node_name
        .or(mesh_name)
        .unwrap_or_else(|| ctr!("RepairDialog", "Mesh {}", ordinal + 1).to_string())
}

#[cfg(test)]
mod repair_run_tests {
    use std::path::PathBuf;

    use sc_mesh_core::transform::Transform3D;
    use sc_mesh_fixtures::{
        cuboid_with_y_tunnel_and_flipped_faces, cuboid_with_z_tunnel_and_flipped_faces,
        unit_cube_mesh, unit_cube_with_flipped_faces,
    };
    use sc_scene::node::LeafPayload;

    use super::*;
    use crate::mesh_health::{MeshHealth, scene_mesh_health};

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
        assert!(
            completed
                .outcomes
                .iter()
                .all(|o| o.fixes.is_empty() && o.refusal.is_none() && o.edge_manifold_after)
        );
        assert_eq!(summary_message(&completed.outcomes), "No problems found.");
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
        assert_eq!(
            doc.with_scene(|s| scene_mesh_health(s.graph())).unwrap(),
            MeshHealth::HasProblems
        );

        let completed = doc
            .with_scene(|spatial| run_repair(spatial.graph().clone()))
            .unwrap();
        assert_eq!(
            settle(&doc, &completed, captured),
            RepairSettlement::Applied
        );
        assert_eq!(
            doc.with_scene(|s| scene_mesh_health(s.graph())).unwrap(),
            MeshHealth::ProblemFree
        );
        // The apply moved the revision: a slice captured before the repair
        // reads as stale.
        assert!(doc.slice_geometry_changed(captured));

        assert!(doc.undo().unwrap());
        assert_eq!(
            doc.with_scene(|s| scene_mesh_health(s.graph())).unwrap(),
            MeshHealth::HasProblems
        );
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
        assert_eq!(
            doc.with_scene(|s| scene_mesh_health(s.graph())).unwrap(),
            MeshHealth::HasProblems,
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
        assert_eq!(
            doc.with_scene(|s| scene_mesh_health(s.graph())).unwrap(),
            MeshHealth::HasProblems,
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

    #[test]
    fn dialog_lines_name_each_touched_mesh_and_skip_clean_ones() {
        let scene = broken_trio_scene();
        let completed = run_repair(scene.clone());
        let lines = dialog_lines(&completed.outcomes, &scene);
        assert_eq!(lines.len(), 3, "3 defective targets, clean cube silent");
        assert!(
            lines
                .iter()
                .all(|line| line.contains("reoriented") && line.contains("face")),
            "each line reports the flip fixes: {lines:?}"
        );
    }
}
