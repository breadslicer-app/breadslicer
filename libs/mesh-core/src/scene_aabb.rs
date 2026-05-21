/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Axis-aligned bounding box queries for [`Scene`].

#[cfg(test)]
use std::sync::Arc;

use crate::aabb::Aabb;
use crate::scene::{Scene, SceneError};
use crate::scene_node::{LeafPayload, NodeId, NodeKind};
use crate::transform::Transform3D;

impl Scene {
    /// Axis-aligned bounding box over every scene root in world space.
    ///
    /// `Group` contributes via the union of its children. `Csg`
    /// contributes the union of its children's world AABBs (a
    /// conservative bound: tight for `Union`, an upper bound for
    /// `Difference` and `Intersection`). `Instance` expands the
    /// referenced subtree under the instance's local transform.
    /// `Leaf` contributes the mesh's local AABB, transformed by the
    /// composed ancestor chain via Arvo's method.
    ///
    /// Returns `None` for a scene whose roots collectively contribute
    /// no geometry, and propagates dangling-id errors.
    pub fn aabb(&self) -> Result<Option<Aabb>, SceneError> {
        let mut accum = Aabb::empty();
        let mut has_any = false;
        for &root in self.roots() {
            self.fold_world_aabb(root, Transform3D::Identity, &mut accum, &mut has_any)?;
        }
        Ok(has_any.then_some(accum))
    }

    /// Translation that, applied as an outer left-multiplication to
    /// every scene root's local transform, drops the scene's world
    /// AABB so its XY centre lands at `target_xy` and its minimum Z
    /// lands at `floor_z`. Returns `Ok(None)` for an empty scene.
    pub fn translation_to_drop_at(
        &self,
        target_xy: [f32; 2],
        floor_z: f32,
    ) -> Result<Option<[f64; 3]>, SceneError> {
        let Some(aabb) = self.aabb()? else {
            return Ok(None);
        };
        let min = aabb.min().coords();
        let max = aabb.max().coords();
        let centre_x = 0.5 * (min[0] + max[0]);
        let centre_y = 0.5 * (min[1] + max[1]);
        Ok(Some([
            f64::from(target_xy[0] - centre_x),
            f64::from(target_xy[1] - centre_y),
            f64::from(floor_z - min[2]),
        ]))
    }

    fn fold_world_aabb(
        &self,
        node_id: NodeId,
        parent_world: Transform3D,
        accum: &mut Aabb,
        has_any: &mut bool,
    ) -> Result<(), SceneError> {
        let node = self
            .node(node_id)
            .ok_or(SceneError::DanglingNodeId { id: node_id })?;
        let world_from_self = parent_world.compose(&node.local_transform);
        match &node.kind {
            NodeKind::Group { children } | NodeKind::Csg { children, .. } => {
                for &c in children {
                    self.fold_world_aabb(c, world_from_self, accum, has_any)?;
                }
            }
            NodeKind::Leaf(payload) => {
                let mesh = match payload {
                    LeafPayload::Mesh(m) => m.as_ref(),
                    LeafPayload::MeshResource(rid) => self
                        .mesh_resource(*rid)
                        .ok_or(SceneError::DanglingMeshResource { id: *rid })?,
                };
                let Some(local) = mesh.local_aabb() else {
                    return Ok(());
                };
                let world_aabb = match &world_from_self {
                    Transform3D::Identity => local,
                    Transform3D::Affine(a) => local.transformed_arvo(a),
                };
                accum.union(&world_aabb);
                *has_any = true;
            }
            NodeKind::Instance { resource } => {
                let root = self
                    .subtree_resource_root(*resource)
                    .ok_or(SceneError::DanglingSubtreeResource { id: *resource })?;
                self.fold_world_aabb(root, world_from_self, accum, has_any)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod scene_aabb_tests {
    use super::*;
    use crate::indexed_mesh::IndexedMesh;
    use crate::primitives::{IndexedTriangle, Normal, Vertex};
    use crate::scene_node::{CsgOp, LeafPayload};
    use crate::transform::AffineTransform3D;
    use float_cmp::assert_approx_eq;

    fn unit_cube_mesh() -> IndexedMesh {
        let v: Vec<Vertex> = vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([1.0, 1.0, 0.0]),
            Vertex::new([0.0, 1.0, 0.0]),
            Vertex::new([0.0, 0.0, 1.0]),
            Vertex::new([1.0, 0.0, 1.0]),
            Vertex::new([1.0, 1.0, 1.0]),
            Vertex::new([0.0, 1.0, 1.0]),
        ];
        let n = Normal::new([0.0, 0.0, 1.0]);
        let face = |a, b, c| IndexedTriangle::from_raw_indices(n, [a, b, c]);
        IndexedMesh {
            meta: None,
            vertices: v,
            faces: vec![face(0, 1, 2), face(4, 5, 6)],
        }
    }

    fn assert_aabb_approx(actual: Aabb, expected: ([f32; 3], [f32; 3]), eps: f32) {
        for (a, e) in actual.min().iter().zip(expected.0.iter()) {
            assert_approx_eq!(f32, *a, *e, epsilon = eps);
        }
        for (a, e) in actual.max().iter().zip(expected.1.iter()) {
            assert_approx_eq!(f32, *a, *e, epsilon = eps);
        }
    }

    #[test]
    fn aabb_of_empty_graph_is_none() {
        let g = Scene::new();
        assert_eq!(g.aabb().unwrap(), None);
    }

    #[test]
    fn aabb_of_single_leaf_at_identity_matches_mesh() {
        let mut g = Scene::new();
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
            )
            .unwrap();
        g.add_root(leaf).unwrap();
        let aabb = g.aabb().unwrap().unwrap();
        assert_aabb_approx(aabb, ([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]), 1e-6);
    }

    #[test]
    fn aabb_composes_group_and_leaf_transforms() {
        // Group with translation T1 contains a leaf with translation T2.
        // Leaf's world AABB is at T1 + T2 + local AABB.
        let mut g = Scene::new();
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation([1.0, 2.0, 3.0])),
                LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
            )
            .unwrap();
        let group = g.add_group(
            None,
            Transform3D::Affine(AffineTransform3D::from_translation([10.0, 0.0, 0.0])),
        );
        g.add_child(group, leaf).unwrap();
        g.add_root(group).unwrap();
        let aabb = g.aabb().unwrap().unwrap();
        assert_aabb_approx(aabb, ([11.0, 2.0, 3.0], [12.0, 3.0, 4.0]), 1e-5);
    }

    #[test]
    fn aabb_unions_two_roots() {
        let mut g = Scene::new();
        let leaf_a = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
            )
            .unwrap();
        let leaf_b = g
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation([10.0, 0.0, 0.0])),
                LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
            )
            .unwrap();
        g.add_root(leaf_a).unwrap();
        g.add_root(leaf_b).unwrap();
        let aabb = g.aabb().unwrap().unwrap();
        assert_aabb_approx(aabb, ([0.0, 0.0, 0.0], [11.0, 1.0, 1.0]), 1e-5);
    }

    #[test]
    fn aabb_treats_csg_children_as_union_conservatively() {
        // A CSG Difference of two cubes. Conservative bound is the
        // union of both children's world AABBs, regardless of operator.
        let mut g = Scene::new();
        let big = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
            )
            .unwrap();
        let small = g
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation([0.25, 0.25, 0.25])),
                LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
            )
            .unwrap();
        let csg = g
            .add_csg(
                None,
                Transform3D::Identity,
                CsgOp::Difference,
                vec![big, small],
            )
            .unwrap();
        g.add_root(csg).unwrap();
        let aabb = g.aabb().unwrap().unwrap();
        // big spans [0,1]^3, small spans [0.25, 1.25]^3; union [0, 1.25]^3.
        assert_aabb_approx(aabb, ([0.0, 0.0, 0.0], [1.25, 1.25, 1.25]), 1e-5);
    }

    #[test]
    fn aabb_two_instances_of_same_subtree_resource() {
        // A subtree contains a single unit cube. Instantiated twice
        // under different transforms; AABB unions both occurrences.
        let mut g = Scene::new();
        let res_leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
            )
            .unwrap();
        let res_root = g.add_group(None, Transform3D::Identity);
        g.add_child(res_root, res_leaf).unwrap();
        let rid = g.register_subtree_resource(res_root).unwrap();

        let r1 = g.add_group(None, Transform3D::Identity);
        let i1 = g
            .add_instance(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation([0.0, 0.0, 0.0])),
                rid,
            )
            .unwrap();
        g.add_child(r1, i1).unwrap();
        g.add_root(r1).unwrap();

        let r2 = g.add_group(None, Transform3D::Identity);
        let i2 = g
            .add_instance(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation([5.0, 0.0, 0.0])),
                rid,
            )
            .unwrap();
        g.add_child(r2, i2).unwrap();
        g.add_root(r2).unwrap();

        let aabb = g.aabb().unwrap().unwrap();
        assert_aabb_approx(aabb, ([0.0, 0.0, 0.0], [6.0, 1.0, 1.0]), 1e-5);
    }

    #[test]
    fn translation_to_drop_at_centres_xy_and_sets_floor_z() {
        let mut g = Scene::new();
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation([100.0, 200.0, 50.0])),
                LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
            )
            .unwrap();
        g.add_root(leaf).unwrap();
        let t = g
            .translation_to_drop_at([0.0, 0.0], 0.0)
            .unwrap()
            .expect("non-empty");
        assert_approx_eq!(f64, t[0], -100.5);
        assert_approx_eq!(f64, t[1], -200.5);
        assert_approx_eq!(f64, t[2], -50.0);
    }

    #[test]
    fn translation_to_drop_at_on_empty_returns_none() {
        let g = Scene::new();
        assert_eq!(g.translation_to_drop_at([0.0, 0.0], 0.0).unwrap(), None);
    }
}
