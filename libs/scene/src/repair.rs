/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Scene-wide repair driver: runs the per-mesh repair policy over every stored
//! mesh (the [`Scene::mesh_targets`] walk) and reports a per-target outcome,
//! with no global abort. Each target is repaired as a copy, through a shared
//! borrow; storage shared through `Arc`s is replaced only for meshes the run
//! changed, so clean, refused and unchanged targets keep their pointer identity
//! (callers can compare pointers to learn what changed).
//!
//! [`Scene::repair_meshes_watched`] tells a [`SceneRepairWatch`] how far the
//! run is, and stops when it asks to, with the scene as it was: the repaired
//! meshes are put in place only once every target is done.

use crate::Scene;
use crate::mesh_targets::MeshTarget;
use sc_mesh_core::indexed_mesh::repair_progress::{
    RepairCancelled, RepairStep, RepairWatch, Unwatched,
};
use sc_mesh_core::indexed_mesh::repair_report::{RepairDetails, repaired_copy_watched};
use sc_mesh_core::indexed_mesh::{MeshRepairReport, RepairPasses};

/// Everything a repair run did, and did not do, to 1 stored mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneRepairOutcome {
    /// The storage place this outcome describes.
    pub target: MeshTarget,
    /// The report of the run on the mesh of the target. A target that the
    /// assessment finds clean, refused or without faces gets no run: its
    /// report has no fixes, and ends as the assessment says.
    pub report: MeshRepairReport,
    /// The mesh before and after, the time of each stage, and what is left
    /// when it still does not slice.
    pub details: RepairDetails,
}

/// How far a [`Scene::repair_meshes_watched`] run is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneRepairProgress {
    /// The target being repaired, 0-based, of `targets`.
    pub target: usize,
    pub targets: usize,
    /// The step it is at.
    pub step: RepairStep,
    /// The share of the whole run done, from 0 to 1, each target weighed by
    /// its faces. It never goes back.
    pub done: f32,
}

/// Watches a [`Scene::repair_meshes_watched`] run.
pub trait SceneRepairWatch {
    /// The run reached `progress`.
    fn progress(&self, progress: SceneRepairProgress);
    /// Whether the run should stop. Asked as each step starts.
    fn cancelled(&self) -> bool;
}

/// A [`SceneRepairWatch`] seen by the run of 1 target.
struct TargetWatch<'a> {
    scene: &'a dyn SceneRepairWatch,
    target: usize,
    targets: usize,
    /// The share of the whole run done before this target, and this
    /// target's share.
    before: f32,
    share: f32,
}

impl RepairWatch for TargetWatch<'_> {
    fn step(&self, step: RepairStep, done: f32) {
        self.scene.progress(SceneRepairProgress {
            target: self.target,
            targets: self.targets,
            step,
            done: (self.before + self.share * done).min(1.0),
        });
    }

    fn cancelled(&self) -> bool {
        self.scene.cancelled()
    }
}

/// A scene watch that wants nothing.
struct NoSceneWatch;

impl SceneRepairWatch for NoSceneWatch {
    fn progress(&self, _progress: SceneRepairProgress) {}

    fn cancelled(&self) -> bool {
        Unwatched.cancelled()
    }
}

impl Scene {
    /// Runs the enabled repair passes over every stored mesh and reports 1
    /// outcome per target, in [`Scene::mesh_targets`] order. A refused target
    /// never blocks the others, and at every observable point each mesh is at
    /// least as valid as before the call (refused meshes are byte-identical,
    /// applied passes only shrink the problem set).
    pub fn repair_meshes(&mut self, passes: RepairPasses) -> Vec<SceneRepairOutcome> {
        match self.repair_meshes_watched(passes, &NoSceneWatch) {
            Ok(outcomes) => outcomes,
            // An unwatched run is never cancelled.
            Err(RepairCancelled) => unreachable!("an unwatched repair was cancelled"),
        }
    }

    /// [`Self::repair_meshes`], telling `watch` how far the run is and
    /// stopping before a step when it asks to. The repaired meshes are put in
    /// place only once every target is done: a run that stops returns
    /// [`RepairCancelled`] and leaves the scene exactly as it was.
    pub fn repair_meshes_watched(
        &mut self,
        passes: RepairPasses,
        watch: &dyn SceneRepairWatch,
    ) -> Result<Vec<SceneRepairOutcome>, RepairCancelled> {
        let targets = self.mesh_targets();
        // Each target weighs its faces; 1 at least, so a run of empty meshes
        // still moves.
        let weights: Vec<f32> = targets
            .iter()
            .map(|&target| self.target_mesh(target).map_or(1, |m| m.faces.len().max(1)) as f32)
            .collect();
        let total: f32 = weights.iter().sum::<f32>().max(1.0);
        let mut before = 0.0;
        let mut outcomes = Vec::with_capacity(targets.len());
        let mut repaired_meshes = Vec::new();
        for (i, (&target, &weight)) in targets.iter().zip(&weights).enumerate() {
            let share = weight / total;
            let Some(mesh) = self.target_mesh(target) else {
                // `mesh_targets` lists live storage only; a dead target here
                // would be a broken graph invariant. Skipping keeps the driver
                // total.
                before += share;
                continue;
            };
            let target_watch = TargetWatch {
                scene: watch,
                target: i,
                targets: targets.len(),
                before,
                share,
            };
            let (report, repaired, details) = repaired_copy_watched(mesh, passes, &target_watch)?;
            before += share;
            if let Some(repaired) = repaired {
                repaired_meshes.push((target, repaired));
            }
            outcomes.push(SceneRepairOutcome {
                target,
                report,
                details,
            });
        }
        if watch.cancelled() {
            return Err(RepairCancelled);
        }
        for (target, repaired) in repaired_meshes {
            if let Some(mesh) = self.target_mesh_mut(target) {
                *mesh = repaired;
            }
        }
        Ok(outcomes)
    }
}

#[cfg(test)]
mod scene_repair_tests {
    use std::sync::Arc;

    use super::*;
    use crate::node::{LeafPayload, NodeId};
    use sc_mesh_core::indexed_mesh::repair::{MeshStateAfterRepair, RepairEnd};
    use sc_mesh_core::indexed_mesh::{IndexedMesh, RepairRefusal};
    use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
    use sc_mesh_core::transform::Transform3D;

    fn tetrahedron() -> IndexedMesh {
        let vertices = vec![
            Vertex::new([1.0, 1.0, 1.0]),
            Vertex::new([1.0, -1.0, -1.0]),
            Vertex::new([-1.0, 1.0, -1.0]),
            Vertex::new([-1.0, -1.0, 1.0]),
        ];
        let faces = [[0, 1, 2], [0, 3, 1], [0, 2, 3], [1, 3, 2]]
            .into_iter()
            .map(IndexedTriangle::from_raw_indices)
            .collect();
        IndexedMesh {
            meta: None,
            vertices,
            faces,
        }
    }

    fn flipped_tetrahedron() -> IndexedMesh {
        let mut mesh = tetrahedron();
        mesh.faces[0].vertices.reverse();
        mesh
    }

    fn nan_tetrahedron() -> IndexedMesh {
        let mut mesh = tetrahedron();
        mesh.vertices[0] = Vertex::new([f32::NAN, 1.0, 1.0]);
        mesh
    }

    /// Returns an open strip of 2 triangles: a hole, with no pass work.
    fn open_strip() -> IndexedMesh {
        let vertices = vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([0.0, 1.0, 0.0]),
            Vertex::new([1.0, 1.0, 0.0]),
        ];
        let faces = [[0, 1, 2], [2, 1, 3]]
            .into_iter()
            .map(IndexedTriangle::from_raw_indices)
            .collect();
        IndexedMesh {
            meta: None,
            vertices,
            faces,
        }
    }

    /// Returns a closed tetrahedron with its 4 vertices in 1 plane: vertex 2 is
    /// the midpoint of vertices 0 and 1, so face 0 is collinear. No pass has
    /// work on it.
    fn flat_tetrahedron() -> IndexedMesh {
        let vertices = vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([2.0, 0.0, 0.0]),
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([1.0, 1.0, 0.0]),
        ];
        let faces = [[2, 0, 1], [0, 2, 3], [2, 1, 3], [1, 0, 3]]
            .into_iter()
            .map(IndexedTriangle::from_raw_indices)
            .collect();
        IndexedMesh {
            meta: None,
            vertices,
            faces,
        }
    }

    fn add_root_group(graph: &mut Scene) -> NodeId {
        let root = graph.add_group(None, Transform3D::Identity);
        graph.add_root(root).unwrap();
        root
    }

    fn add_inline_leaf(graph: &mut Scene, parent: NodeId, mesh: IndexedMesh) -> NodeId {
        let leaf = graph
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .unwrap();
        graph.add_child(parent, leaf).unwrap();
        leaf
    }

    /// Pins that a resource with repairable winding, a refused NaN inline leaf
    /// and a clean inline leaf give 1 outcome each, in walk order, and that the
    /// refused target blocks no other target.
    #[test]
    fn outcomes_are_per_target_in_walk_order_with_no_abort() {
        let mut graph = Scene::new();
        let root = add_root_group(&mut graph);
        let nan_leaf = add_inline_leaf(&mut graph, root, nan_tetrahedron());
        let clean_leaf = add_inline_leaf(&mut graph, root, tetrahedron());
        let rid = graph.add_mesh_resource(flipped_tetrahedron());
        let res_leaf = graph
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .unwrap();
        graph.add_child(root, res_leaf).unwrap();

        let outcomes = graph.repair_meshes(RepairPasses::default_repair());

        let closed = RepairEnd::Completed(MeshStateAfterRepair {
            edge_manifold: true,
            zero_area_face_count: 0,
            holes_left_open: 0,
        });
        assert_eq!(outcomes.len(), 3);
        assert_eq!(outcomes[0].target, MeshTarget::Resource(rid));
        assert_eq!(outcomes[0].report.fixes.len(), 1);
        assert_eq!(outcomes[0].report.end, closed);

        assert_eq!(outcomes[1].target, MeshTarget::InlineLeaf(nan_leaf));
        assert!(outcomes[1].report.fixes.is_empty());
        assert!(matches!(
            outcomes[1].report.end,
            RepairEnd::Refused(RepairRefusal::InvalidVertices(_))
        ));

        assert_eq!(outcomes[2].target, MeshTarget::InlineLeaf(clean_leaf));
        assert!(outcomes[2].report.fixes.is_empty());
        assert_eq!(outcomes[2].report.end, closed);

        // The repaired resource is edge-manifold in the scene, and the refused
        // mesh kept its bytes (NaN compared bitwise).
        let repaired = graph.target_mesh(MeshTarget::Resource(rid)).unwrap();
        assert!(
            repaired
                .validated_faces()
                .is_ok_and(|faces| faces.validated_edge_manifold().is_ok())
        );
        let kept = graph.target_mesh(MeshTarget::InlineLeaf(nan_leaf)).unwrap();
        let expected = nan_tetrahedron();
        assert_eq!(kept.faces, expected.faces);
        for (a, b) in kept.vertices.iter().zip(expected.vertices.iter()) {
            assert_eq!(a.coords().map(f32::to_bits), b.coords().map(f32::to_bits));
        }
    }

    /// Pins that, with every `Arc` shared with an outside holder (as when the
    /// GUI worker clones the scene), only the repaired target forks; clean and
    /// refused targets keep their pointer identity, and the outside holder
    /// keeps the bytes from before the repair.
    #[test]
    fn only_repaired_targets_fork_their_shared_storage() {
        let mut graph = Scene::new();
        let root = add_root_group(&mut graph);
        let nan_leaf = add_inline_leaf(&mut graph, root, nan_tetrahedron());
        let clean_leaf = add_inline_leaf(&mut graph, root, tetrahedron());
        let rid = graph.add_mesh_resource(flipped_tetrahedron());
        let res_leaf = graph
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .unwrap();
        graph.add_child(root, res_leaf).unwrap();

        // Outside holders, as a cloned live document would hold them.
        let held_res = Arc::clone(graph.mesh_resource_arc(rid).unwrap());
        let held_nan = Arc::clone(graph.leaf_mesh_arc(nan_leaf).unwrap());
        let held_clean = Arc::clone(graph.leaf_mesh_arc(clean_leaf).unwrap());

        graph.repair_meshes(RepairPasses::default_repair());

        // Repaired: forked away from the holder, which kept the bytes from
        // before the repair (still flipped).
        assert!(!Arc::ptr_eq(
            graph.mesh_resource_arc(rid).unwrap(),
            &held_res
        ));
        assert_eq!(*held_res, flipped_tetrahedron());
        // Refused and clean: pointer identity preserved.
        assert!(Arc::ptr_eq(
            graph.leaf_mesh_arc(nan_leaf).unwrap(),
            &held_nan
        ));
        assert!(Arc::ptr_eq(
            graph.leaf_mesh_arc(clean_leaf).unwrap(),
            &held_clean
        ));
    }

    /// Records the progress of a watched run, and cancels once it was asked
    /// `cancel_after` times.
    struct Recorder {
        seen: std::cell::RefCell<Vec<SceneRepairProgress>>,
        asked: std::cell::Cell<usize>,
        cancel_after: usize,
    }

    impl Recorder {
        fn new(cancel_after: usize) -> Self {
            Self {
                seen: Default::default(),
                asked: Default::default(),
                cancel_after,
            }
        }
    }

    impl SceneRepairWatch for Recorder {
        fn progress(&self, progress: SceneRepairProgress) {
            self.seen.borrow_mut().push(progress);
        }

        fn cancelled(&self) -> bool {
            let asked = self.asked.get();
            self.asked.set(asked + 1);
            asked >= self.cancel_after
        }
    }

    /// 2 repairable targets and a clean one.
    fn two_broken_and_a_clean_one() -> Scene {
        let mut graph = Scene::new();
        let root = add_root_group(&mut graph);
        add_inline_leaf(&mut graph, root, flipped_tetrahedron());
        add_inline_leaf(&mut graph, root, tetrahedron());
        let rid = graph.add_mesh_resource(flipped_tetrahedron());
        let res_leaf = graph
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .unwrap();
        graph.add_child(root, res_leaf).unwrap();
        graph
    }

    /// The `Arc` of every target of `graph`, in walk order.
    fn arcs(graph: &Scene) -> Vec<*const IndexedMesh> {
        graph
            .mesh_targets()
            .into_iter()
            .map(|target| match target {
                MeshTarget::Resource(rid) => Arc::as_ptr(graph.mesh_resource_arc(rid).unwrap()),
                MeshTarget::InlineLeaf(id) => Arc::as_ptr(graph.leaf_mesh_arc(id).unwrap()),
            })
            .collect()
    }

    /// Pins that a watched run gives what an unwatched one gives, and that
    /// its progress walks every target, in order, never going back.
    #[test]
    fn a_watched_run_repairs_as_an_unwatched_one_and_walks_every_target() {
        let mut plain = two_broken_and_a_clean_one();
        let mut watched = two_broken_and_a_clean_one();
        let expected = plain.repair_meshes(RepairPasses::default_repair());
        let watch = Recorder::new(usize::MAX);
        let outcomes = watched
            .repair_meshes_watched(RepairPasses::default_repair(), &watch)
            .expect("never cancelled");
        assert_eq!(outcomes.len(), expected.len());
        for (a, b) in outcomes.iter().zip(&expected) {
            assert_eq!(a.target, b.target);
            assert_eq!(a.report, b.report);
            assert_eq!(a.details.after, b.details.after);
        }
        for target in plain.mesh_targets() {
            assert_eq!(plain.target_mesh(target), watched.target_mesh(target));
        }
        let seen = watch.seen.into_inner();
        let targets: Vec<usize> = seen.iter().map(|p| p.target).collect();
        assert_eq!(targets.first(), Some(&0));
        assert_eq!(targets.last(), Some(&2));
        assert!(targets.windows(2).all(|w| w[0] <= w[1]));
        assert!(seen.iter().all(|p| p.targets == 3));
        assert!(seen.windows(2).all(|w| w[0].done <= w[1].done));
        assert!(seen.last().is_some_and(|p| p.done > 0.9 && p.done <= 1.0));
    }

    /// Pins that a run cancelled at any point leaves every mesh of the scene
    /// where it was: none of the targets done before the cancel is put in.
    #[test]
    fn a_cancelled_run_leaves_the_scene_exactly_as_it_was() {
        let full = Recorder::new(usize::MAX);
        two_broken_and_a_clean_one()
            .repair_meshes_watched(RepairPasses::default_repair(), &full)
            .expect("never cancelled");
        let asked = full.asked.get();
        assert!(asked > 5);
        for k in 0..asked {
            let mut graph = two_broken_and_a_clean_one();
            let before = arcs(&graph);
            let watch = Recorder::new(k);
            assert_eq!(
                graph
                    .repair_meshes_watched(RepairPasses::default_repair(), &watch)
                    .err(),
                Some(RepairCancelled),
                "cancel after {k} asks"
            );
            assert_eq!(arcs(&graph), before, "cancel after {k} asks");
        }
    }

    /// Pins that a hole too large to fill (the strip is a flat plate) is a
    /// verdict, not a refusal, and does not fork.
    #[test]
    fn a_holed_target_reports_false_verdict_without_work() {
        let mut graph = Scene::new();
        let root = add_root_group(&mut graph);
        let leaf = add_inline_leaf(&mut graph, root, open_strip());
        let held = Arc::clone(graph.leaf_mesh_arc(leaf).unwrap());

        let outcomes = graph.repair_meshes(RepairPasses::default_repair());

        assert_eq!(outcomes.len(), 1);
        assert_eq!(
            outcomes[0].report,
            MeshRepairReport {
                fixes: Vec::new(),
                end: RepairEnd::Completed(MeshStateAfterRepair {
                    edge_manifold: false,
                    zero_area_face_count: 0,
                    holes_left_open: 1,
                }),
            }
        );
        assert!(Arc::ptr_eq(graph.leaf_mesh_arc(leaf).unwrap(), &held));
    }

    /// Pins that a target with no work gets the zero-area faces of its mesh in
    /// its report, and does not fork.
    #[test]
    fn a_clean_target_reports_its_zero_area_face_without_work() {
        let mut graph = Scene::new();
        let root = add_root_group(&mut graph);
        let leaf = add_inline_leaf(&mut graph, root, flat_tetrahedron());
        let held = Arc::clone(graph.leaf_mesh_arc(leaf).unwrap());

        let outcomes = graph.repair_meshes(RepairPasses::default_repair());

        assert_eq!(outcomes.len(), 1);
        assert_eq!(
            outcomes[0].report,
            MeshRepairReport {
                fixes: Vec::new(),
                end: RepairEnd::Completed(MeshStateAfterRepair {
                    edge_manifold: true,
                    zero_area_face_count: 1,
                    holes_left_open: 0,
                }),
            }
        );
        assert!(Arc::ptr_eq(graph.leaf_mesh_arc(leaf).unwrap(), &held));
    }

    /// Pins parity with the shared walk and the health sweep: the driver covers
    /// exactly `mesh_targets()`, the inline leaf of a registered subtree
    /// included, and every leaf that the health sweep flags resolves to a
    /// covered target.
    #[test]
    fn driver_targets_match_the_walk_and_cover_health_flags() {
        let mut graph = Scene::new();
        let root = add_root_group(&mut graph);
        add_inline_leaf(&mut graph, root, open_strip());
        let rid = graph.add_mesh_resource(flipped_tetrahedron());
        let res_leaf = graph
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .unwrap();
        graph.add_child(root, res_leaf).unwrap();
        let sub_root = graph.add_group(None, Transform3D::Identity);
        add_inline_leaf(&mut graph, sub_root, open_strip());
        graph.register_subtree_resource(sub_root).unwrap();

        let expected_targets = graph.mesh_targets();
        let flagged = graph.non_edge_manifold_leaves();

        let outcomes = graph.repair_meshes(RepairPasses::default_repair());
        let outcome_targets: Vec<MeshTarget> = outcomes.iter().map(|o| o.target).collect();

        assert_eq!(outcome_targets, expected_targets);
        for leaf in flagged {
            let target = graph.leaf_mesh_target(leaf).unwrap();
            assert!(
                outcome_targets.contains(&target),
                "health-flagged leaf {leaf:?} not covered by the driver"
            );
        }
    }
}
