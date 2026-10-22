/* SPDX-License-Identifier: Apache-2.0 */
#[test]
fn blocked_minimum_matches_full_vertex_scan_across_transforms() {
    use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
    use sc_mesh_core::{IndexedMesh, Vertex};
    use sc_mesh_spatial::SceneSpatial;
    use sc_scene::{LeafPayload, Scene};
    use std::sync::Arc;
    for scrambled in [false, true] {
        let mut vertices = Vec::new();
        for i in 0..8192 {
            let t = if scrambled { (i * 7919) % 8192 } else { i };
            let f = t as f32;
            vertices.push(Vertex::new([
                f * 0.03 - 120.0,
                (f * 0.017).sin() * 30.0,
                (f * 0.031).cos() * 70.0,
            ]));
        }
        let mut graph = Scene::new();
        let id = graph
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(IndexedMesh {
                    meta: None,
                    vertices,
                    faces: vec![],
                })),
            )
            .unwrap();
        graph.add_root(id).unwrap();
        let spatial = SceneSpatial::from_graph(graph);
        let q = spatial.leaf(id).unwrap();
        for i in 0..31 {
            let r = AffineTransform3D::from_rotation_x(i as f64 * 0.23);
            let y = Transform3D::Affine(AffineTransform3D::from_rotation_y(i as f64 * 0.37));
            let scale =
                Transform3D::Affine(AffineTransform3D::from_non_uniform_scale([-1.3, 2.7, 0.4]));
            let world =
                Transform3D::Affine(AffineTransform3D::from_translation([12.0, -30.0, 47.0]))
                    .compose(&Transform3D::Affine(r).compose(&y.compose(&scale)));
            let expected = q
                .mesh()
                .vertices
                .iter()
                .map(|v| world.apply_to_vertex(*v).coords()[2])
                .fold(f32::INFINITY, f32::min);
            let actual = q.minimum_world_z(world).unwrap();
            assert!(
                (actual - f64::from(expected)).abs() < 3e-5,
                "scrambled={} angle={} actual={} expected={}",
                scrambled,
                i,
                actual,
                expected
            );
        }
    }
}
