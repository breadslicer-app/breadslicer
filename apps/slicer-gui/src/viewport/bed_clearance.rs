/* SPDX-License-Identifier: AGPL-3.0-only */
//! Keep rotate/scale edits above the plate using eight cached local-box corners.
//! No per-frame scan of dense meshes. Explicit Z translation bypasses this policy.
use sc_mesh_core::primitives::Vertex;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_spatial::SceneSpatial;
use sc_scene::NodeId;

pub(super) fn keep_above_bed(
    spatial: &SceneSpatial,
    node_id: NodeId,
    proposed: Transform3D,
) -> Transform3D {
    let Some(query) = spatial.leaf(node_id) else {
        return proposed;
    };
    let Some(bounds) = query.local_aabb() else {
        return proposed;
    };
    let parent = query.parent_world();
    let world = parent.compose(&proposed);
    let lo = bounds.min().coords();
    let hi = bounds.max().coords();
    let mut min_z = f32::INFINITY;
    for x in [lo[0], hi[0]] {
        for y in [lo[1], hi[1]] {
            for z in [lo[2], hi[2]] {
                min_z = min_z.min(world.apply_to_vertex(Vertex::new([x, y, z])).coords()[2]);
            }
        }
    }
    if !min_z.is_finite() || min_z >= 0.0 {
        return proposed;
    }
    let Ok(inverse_parent) = parent.inverse() else {
        return proposed;
    };
    // Tiny clearance absorbs f32 GPU rounding; correction is part of the same edit.
    let lift = Transform3D::Affine(AffineTransform3D::from_translation([
        0.0,
        0.0,
        -f64::from(min_z) + 1e-5,
    ]));
    inverse_parent.compose(&lift.compose(&world))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_mesh_core::IndexedMesh;
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
        let corrected = keep_above_bed(&s, id, rotation);
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
        assert_eq!(keep_above_bed(&s, id, t), t);
    }
    #[test]
    fn nonuniform_scale_is_lifted() {
        let (s, id) = fixture(Transform3D::Identity);
        let t = Transform3D::Affine(AffineTransform3D::from_translation([0.0, 0.0, -2.0])).compose(
            &Transform3D::Affine(AffineTransform3D::from_non_uniform_scale([1.0, 2.0, 3.0])),
        );
        assert!(minimum(&s, id, keep_above_bed(&s, id, t)) >= 0.0);
    }
    #[test]
    fn rotated_parent_gets_world_vertical_lift() {
        let parent = Transform3D::Affine(AffineTransform3D::from_rotation_x(0.7));
        let (s, id) = fixture(parent);
        let corrected = keep_above_bed(&s, id, Transform3D::Identity);
        assert!(minimum(&s, id, corrected) >= 0.0);
        let w = parent.compose(&corrected);
        if let (Transform3D::Affine(a), Transform3D::Affine(b)) = (parent, w) {
            for (x, y) in a.linear().iter().flatten().zip(b.linear().iter().flatten()) {
                assert!((x - y).abs() < 1e-12);
            }
            assert!(b.translation()[0].abs() < 1e-12 && b.translation()[1].abs() < 1e-12);
        }
    }
}
