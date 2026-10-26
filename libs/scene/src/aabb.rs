/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Axis-aligned bounding box queries for [`Scene`].

#[cfg(test)]
use std::sync::Arc;

use crate::Scene;
use crate::error::SceneError;
use crate::node::{LeafPayload, NodeId, NodeKind};
use sc_mesh_core::aabb::Aabb;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};

impl Scene {
    /// Returns the axis-aligned bounding box of every scene root in world
    /// space.
    ///
    /// `Group` contributes via the union of its children. `Csg` contributes the
    /// union of the world AABBs of its children (a conservative bound: tight
    /// for `Union`, an upper bound for `Difference` and `Intersection`).
    /// `Reference` expands the referenced subtree under the local transform of
    /// the reference. `Leaf` contributes the local AABB of its mesh,
    /// transformed by the composed ancestor chain with Arvo's method.
    ///
    /// Returns `None` for a scene whose roots contribute no geometry, and
    /// propagates dangling-id errors.
    pub fn aabb(&self) -> Result<Option<Aabb>, SceneError> {
        self.world_aabb(false)
    }

    /// Like [`Self::aabb`], but tight: every leaf contributes the box of its
    /// vertices placed in world space, with the same math the bake uses. A
    /// rotated leaf's transformed local box (the [`Self::aabb`] bound) can
    /// stand well past the mesh; this is the scene's real extent, at the
    /// cost of a pass over every vertex.
    pub fn tight_aabb(&self) -> Result<Option<Aabb>, SceneError> {
        self.world_aabb(true)
    }

    fn world_aabb(&self, tight: bool) -> Result<Option<Aabb>, SceneError> {
        let mut accum = Aabb::empty();
        let mut has_any = false;
        for &root in self.roots() {
            self.fold_world_aabb(root, Transform3D::Identity, tight, &mut accum, &mut has_any)?;
        }
        Ok(has_any.then_some(accum))
    }

    /// Returns the translation that, applied as an outer left-multiplication to
    /// the local transform of every scene root, moves the world AABB of the
    /// scene so that its XY centre is at `target_xy` and its minimum Z is at
    /// `floor_z`. Returns `Ok(None)` for an empty scene.
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

    /// Drops the scene onto the plate: translates every scene root by the
    /// shared delta from [`Self::translation_to_drop_at`], so that the XY
    /// centre of the world AABB is at `target_xy` and its minimum Z is at
    /// `floor_z`. The translation composes as an outer left-multiplication onto
    /// the local transform of each root, so the relative layout of the roots is
    /// kept. Returns whether anything moved: `false` for an empty scene or a
    /// scene already at the target (whose roots keep their exact transforms,
    /// `Identity` included).
    pub fn drop_at(&mut self, target_xy: [f32; 2], floor_z: f32) -> Result<bool, SceneError> {
        let Some(delta) = self.translation_to_drop_at(target_xy, floor_z)? else {
            return Ok(false);
        };
        if delta == [0.0, 0.0, 0.0] {
            return Ok(false);
        }
        let outer = Transform3D::Affine(AffineTransform3D::from_translation(delta));
        for root_id in self.roots().to_vec() {
            let current = self
                .node(root_id)
                .ok_or(SceneError::DanglingNodeId { id: root_id })?
                .local_transform;
            self.set_local_transform(root_id, outer.compose(&current))?;
        }
        Ok(true)
    }

    fn fold_world_aabb(
        &self,
        node_id: NodeId,
        parent_world: Transform3D,
        tight: bool,
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
                    self.fold_world_aabb(c, world_from_self, tight, accum, has_any)?;
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
                    Transform3D::Affine(a) if tight => {
                        let mut placed = Aabb::empty();
                        for &v in &mesh.vertices {
                            placed.extend(a.apply_to_vertex(v));
                        }
                        placed
                    }
                    Transform3D::Affine(a) => local.transformed_arvo(a),
                };
                accum.union(&world_aabb);
                *has_any = true;
            }
            NodeKind::Reference { resource } => {
                let root = self
                    .subtree_resource_root(*resource)
                    .ok_or(SceneError::DanglingSubtreeResource { id: *resource })?;
                self.fold_world_aabb(root, world_from_self, tight, accum, has_any)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod scene_aabb_tests {
    use super::*;
    use crate::node::{CsgOp, LeafPayload};
    use float_cmp::assert_approx_eq;
    use sc_mesh_core::indexed_mesh::IndexedMesh;
    use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
    use sc_mesh_core::transform::AffineTransform3D;

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
        let face = |a, b, c| IndexedTriangle::from_raw_indices([a, b, c]);
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
        // A group with translation T1 holds a leaf with translation T2: the
        // world AABB of the leaf is at T1 + T2 + local AABB.
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
        // A CSG Difference of 2 cubes. The conservative bound is the union of
        // the world AABBs of both children, whatever the operator.
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
    fn aabb_two_references_to_same_subtree_resource() {
        // A subtree holds a single unit cube, referenced twice under different
        // transforms; the AABB is the union of both occurrences.
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
            .add_reference(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation([0.0, 0.0, 0.0])),
                rid,
            )
            .unwrap();
        g.add_child(r1, i1).unwrap();
        g.add_root(r1).unwrap();

        let r2 = g.add_group(None, Transform3D::Identity);
        let i2 = g
            .add_reference(
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

    /// Returns the 8 corners of the cuboid that spans `min`..`max`, with 2
    /// faces: enough for the AABB walk.
    fn cube_mesh(min: [f32; 3], max: [f32; 3]) -> IndexedMesh {
        let v = |x, y, z| Vertex::new([x, y, z]);
        let vertices = vec![
            v(min[0], min[1], min[2]),
            v(max[0], min[1], min[2]),
            v(max[0], max[1], min[2]),
            v(min[0], max[1], min[2]),
            v(min[0], min[1], max[2]),
            v(max[0], min[1], max[2]),
            v(max[0], max[1], max[2]),
            v(min[0], max[1], max[2]),
        ];
        let faces = vec![
            IndexedTriangle::from_raw_indices([0, 1, 2]),
            IndexedTriangle::from_raw_indices([4, 5, 6]),
        ];
        IndexedMesh {
            meta: None,
            vertices,
            faces,
        }
    }

    /// Builds a scene with the supplied leaves under a single identity `Group`
    /// root, the shape that the file loaders produce.
    fn group_rooted_scene(leaves: Vec<(IndexedMesh, Transform3D)>) -> Scene {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group as root");
        for (mesh, transform) in leaves {
            let leaf = g
                .add_leaf(None, transform, LeafPayload::Mesh(Arc::new(mesh)))
                .expect("add leaf");
            g.add_child(root, leaf).expect("attach leaf under root");
        }
        g
    }

    #[test]
    fn drop_at_on_empty_is_a_no_op() {
        let mut g = Scene::new();
        assert!(!g.drop_at([0.0, 0.0], 0.0).unwrap());
        assert!(g.aabb().unwrap().is_none());
    }

    #[test]
    fn drop_at_centres_an_off_origin_scene_with_its_bottom_on_the_floor() {
        let mut g = group_rooted_scene(vec![(
            cube_mesh([100.0, 200.0, 50.0], [110.0, 220.0, 60.0]),
            Transform3D::Identity,
        )]);
        assert!(g.drop_at([0.0, 0.0], 0.0).unwrap());
        let world = g.aabb().unwrap().unwrap();
        assert_aabb_approx(world, ([-5.0, -10.0, 0.0], [5.0, 10.0, 10.0]), 1e-4);
    }

    #[test]
    fn drop_at_honours_a_non_origin_target() {
        let mut g = group_rooted_scene(vec![(
            cube_mesh([100.0, 200.0, 50.0], [110.0, 220.0, 60.0]),
            Transform3D::Identity,
        )]);
        assert!(g.drop_at([10.0, -4.0], 2.0).unwrap());
        let world = g.aabb().unwrap().unwrap();
        assert_aabb_approx(world, ([5.0, -14.0, 2.0], [15.0, 6.0, 12.0]), 1e-4);
    }

    #[test]
    fn drop_at_on_an_already_placed_scene_keeps_the_exact_transforms() {
        let mut g = group_rooted_scene(vec![(
            cube_mesh([-5.0, -5.0, 0.0], [5.0, 5.0, 10.0]),
            Transform3D::Identity,
        )]);
        let root_id = g.roots()[0];
        assert!(!g.drop_at([0.0, 0.0], 0.0).unwrap());
        assert!(matches!(
            g.node(root_id).unwrap().local_transform,
            Transform3D::Identity
        ));
    }

    #[test]
    fn drop_at_preserves_the_relative_layout_of_leaves() {
        // 2 unit cubes 9 mm apart on X and Y under 1 group root: the union
        // lands centred with its bottom on the floor and the cubes stay 9 mm
        // apart (only the root moves).
        let mut g = group_rooted_scene(vec![
            (
                cube_mesh([100.0, 200.0, 50.0], [101.0, 201.0, 51.0]),
                Transform3D::Identity,
            ),
            (
                cube_mesh([109.0, 209.0, 53.0], [110.0, 210.0, 54.0]),
                Transform3D::Identity,
            ),
        ]);
        assert!(g.drop_at([0.0, 0.0], 0.0).unwrap());
        let world = g.aabb().unwrap().unwrap();
        assert_aabb_approx(world, ([-5.0, -5.0, 0.0], [5.0, 5.0, 4.0]), 1e-4);
    }

    #[test]
    fn drop_at_translates_every_root_by_the_shared_delta() {
        // 2 root leaves 10 mm apart on X: both roots receive the same outer
        // translation, so the union centres while the relative offset of the
        // roots stays.
        let mut g = Scene::new();
        let leaf_a = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(cube_mesh([100.0, 0.0, 5.0], [101.0, 1.0, 6.0]))),
            )
            .unwrap();
        let leaf_b = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(cube_mesh([110.0, 0.0, 5.0], [111.0, 1.0, 6.0]))),
            )
            .unwrap();
        g.add_root(leaf_a).unwrap();
        g.add_root(leaf_b).unwrap();
        assert!(g.drop_at([0.0, 0.0], 0.0).unwrap());
        let world = g.aabb().unwrap().unwrap();
        assert_aabb_approx(world, ([-5.5, -0.5, 0.0], [5.5, 0.5, 1.0]), 1e-4);
    }

    #[test]
    fn drop_at_composes_with_an_existing_leaf_transform() {
        // The leaf already carries a translation; the placement delta lands on
        // the root Group, outside it.
        let mut g = group_rooted_scene(vec![(
            cube_mesh([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]),
            Transform3D::Affine(AffineTransform3D::from_translation([10.0, 20.0, 5.0])),
        )]);
        assert!(g.drop_at([0.0, 0.0], 0.0).unwrap());
        let world = g.aabb().unwrap().unwrap();
        assert_aabb_approx(world, ([-0.5, -0.5, 0.0], [0.5, 0.5, 1.0]), 1e-4);
    }

    /// An octahedron (vertices on the axes at distance 1) turned 45 degrees
    /// about X: its top vertex sits at sin 45, while its transformed local
    /// box reaches sqrt 2.
    #[test]
    fn tight_aabb_follows_the_vertices_of_a_rotated_leaf() {
        use sc_mesh_core::Vertex;
        let v = |p: [f32; 3]| Vertex::new(p);
        let octahedron = IndexedMesh::new(
            None,
            vec![
                v([1.0, 0.0, 0.0]),
                v([-1.0, 0.0, 0.0]),
                v([0.0, 1.0, 0.0]),
                v([0.0, -1.0, 0.0]),
                v([0.0, 0.0, 1.0]),
                v([0.0, 0.0, -1.0]),
            ],
            vec![],
        );
        let mut g = Scene::new();
        let turn = AffineTransform3D::from_rotation_x(std::f64::consts::FRAC_PI_4);
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Affine(turn),
                LeafPayload::Mesh(Arc::new(octahedron)),
            )
            .unwrap();
        g.add_root(leaf).unwrap();
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let tight = g.tight_aabb().unwrap().unwrap();
        assert_aabb_approx(tight, ([-1.0, -s, -s], [1.0, s, s]), 1e-6);
        let loose = g.aabb().unwrap().unwrap();
        assert!(loose.max().coords()[2] > 1.4, "{loose:?}");
    }
}
