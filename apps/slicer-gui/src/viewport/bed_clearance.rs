/* SPDX-License-Identifier: AGPL-3.0-only */
//! Preserve bed contact using the tight world-axis-aligned bounds of the mesh.
//! Explicit Z translation bypasses this policy; raised objects retain their gap.
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_spatial::SceneSpatial;
use sc_scene::NodeId;

pub(super) fn keep_above_bed(
    spatial: &SceneSpatial,
    node_id: NodeId,
    before: Transform3D,
    proposed: Transform3D,
) -> Transform3D {
    let Some(query) = spatial.leaf(node_id) else {
        return proposed;
    };
    let parent = query.parent_world();
    let world = parent.compose(&proposed);
    let Some(previous_min) = query.minimum_world_z(parent.compose(&before)) else {
        return proposed;
    };
    let Some(min_z) = query.minimum_world_z(world) else {
        return proposed;
    };
    if !previous_min.is_finite() || !min_z.is_finite() {
        return proposed;
    }
    // Preserve intentional elevation, but never inherit accumulated rounding
    // clearance. A model resting on the plate must also follow it downwards.
    let target_z = if previous_min > 1e-4 {
        previous_min
    } else {
        0.0
    };
    let correction = target_z - min_z;
    if correction.abs() <= 1e-6 {
        return proposed;
    }
    let Ok(inverse_parent) = parent.inverse() else {
        return proposed;
    };
    // Tiny clearance absorbs f32 GPU rounding; correction is part of the same edit.
    let lift = Transform3D::Affine(AffineTransform3D::from_translation([
        0.0,
        0.0,
        correction + 1e-5,
    ]));
    inverse_parent.compose(&lift.compose(&world))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::primitives::Vertex;
    use sc_scene::{LeafPayload, Scene};
    use std::sync::Arc;
    fn fixture(parent: Transform3D) -> (SceneSpatial, NodeId) {
        let mut scene = Scene::new();
        let root = scene.add_group(None, parent);
        scene.add_root(root).unwrap();
        let vertices = [-1.0, 1.0]
            .into_iter()
            .flat_map(|x| {
                [-1.0, 1.0]
                    .into_iter()
                    .flat_map(move |y| [0.0, 2.0].into_iter().map(move |z| Vertex::new([x, y, z])))
            })
            .collect();
        let mesh = IndexedMesh {
            meta: None,
            vertices,
            faces: vec![],
        };
        let leaf = scene
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .unwrap();
        scene.add_child(root, leaf).unwrap();
        (SceneSpatial::from_graph(scene), leaf)
    }
    fn minimum(spatial: &SceneSpatial, id: NodeId, local: Transform3D) -> f32 {
        let q = spatial.leaf(id).unwrap();
        q.transformed_aabb(q.parent_world().compose(&local))
            .unwrap()
            .min()
            .coords()[2]
    }
    #[test]
    fn rotation_is_lifted_without_changing_linear_part() {
        let (s, id) = fixture(Transform3D::Identity);
        let rotation = Transform3D::Affine(AffineTransform3D::from_rotation_y(0.8));
        assert!(minimum(&s, id, rotation) < 0.0);
        let corrected = keep_above_bed(&s, id, Transform3D::Identity, rotation);
        assert!(minimum(&s, id, corrected) >= 0.0);
        if let (Transform3D::Affine(a), Transform3D::Affine(b)) = (rotation, corrected) {
            assert_eq!(a.linear(), b.linear());
        } else {
            panic!("expected affine transforms");
        }
    }
    #[test]
    fn elevated_model_is_not_lowered() {
        let (s, id) = fixture(Transform3D::Identity);
        let t = Transform3D::Affine(AffineTransform3D::from_translation([0.0, 0.0, 20.0]));
        assert_eq!(keep_above_bed(&s, id, t, t), t);
    }
    #[test]
    fn nonuniform_scale_is_lifted() {
        let (s, id) = fixture(Transform3D::Identity);
        let t = Transform3D::Affine(AffineTransform3D::from_translation([0.0, 0.0, -2.0])).compose(
            &Transform3D::Affine(AffineTransform3D::from_non_uniform_scale([1.0, 2.0, 3.0])),
        );
        assert!(minimum(&s, id, keep_above_bed(&s, id, Transform3D::Identity, t)) >= 0.0);
    }
    #[test]
    fn rotated_parent_gets_world_vertical_lift() {
        let parent = Transform3D::Affine(AffineTransform3D::from_rotation_x(0.7));
        let (s, id) = fixture(parent);
        let corrected = keep_above_bed(&s, id, Transform3D::Identity, Transform3D::Identity);
        assert!(minimum(&s, id, corrected) >= 0.0);
        let w = parent.compose(&corrected);
        if let (Transform3D::Affine(a), Transform3D::Affine(b)) = (parent, w) {
            for (x, y) in a.linear().iter().flatten().zip(b.linear().iter().flatten()) {
                assert!((x - y).abs() < 1e-12);
            }
            assert!(b.translation()[0].abs() < 1e-12 && b.translation()[1].abs() < 1e-12);
        }
    }
    #[test]
    fn irregular_mesh_rests_on_actual_vertex_not_empty_box_corner() {
        let mut scene = Scene::new();
        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([-1.0, 0.0, 1.0]),
                Vertex::new([1.0, 0.0, 1.0]),
                Vertex::new([0.0, -1.0, 1.0]),
                Vertex::new([0.0, 1.0, 1.0]),
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([0.0, 0.0, 2.0]),
            ],
            faces: vec![],
        };
        let id = scene
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .unwrap();
        scene.add_root(id).unwrap();
        let s = SceneSpatial::from_graph(scene);
        let r = Transform3D::Affine(AffineTransform3D::from_rotation_y(0.7));
        let corrected = keep_above_bed(&s, id, Transform3D::Identity, r);
        assert!(minimum(&s, id, corrected).abs() < 3e-5);
    }
    #[test]
    fn returning_to_upright_moves_down_to_plate() {
        let (s, id) = fixture(Transform3D::Identity);
        let raised = Transform3D::Affine(AffineTransform3D::from_translation([0.0, 0.0, 2.0]));
        let corrected = keep_above_bed(&s, id, Transform3D::Identity, raised);
        assert!(minimum(&s, id, corrected).abs() < 3e-5);
    }
    #[test]
    fn raised_model_keeps_intentional_gap_during_rotation() {
        let (s, id) = fixture(Transform3D::Identity);
        let before = Transform3D::Affine(AffineTransform3D::from_translation([0.0, 0.0, 20.0]));
        let proposed = before.compose(&Transform3D::Affine(AffineTransform3D::from_rotation_y(
            0.7,
        )));
        let corrected = keep_above_bed(&s, id, before, proposed);
        assert!((minimum(&s, id, corrected) - 20.0).abs() < 3e-5);
    }
}
