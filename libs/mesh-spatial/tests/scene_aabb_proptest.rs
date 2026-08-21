/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 *
 * Property tests pinning the cross-layer agreement between
 * `Scene::aabb` (uncached, in `sc-mesh-core`) and `scene_aabb`
 * (cached, in `sc-mesh-spatial`). The two implement the same Arvo
 * loop at different caching layers; if they ever drift, framing /
 * picker / view-fit code that relies on either side breaks
 * silently. This test fails CI instead.
 */

use std::sync::Arc;

use proptest::prelude::*;
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
use sc_mesh_core::transform::AffineTransform3D;
use sc_mesh_core::transform::Transform3D;
use sc_mesh_spatial::{SceneSpatial, scene_aabb};
use sc_scene::{LeafPayload, Scene};

/// Bounded finite f32: avoid NaN / ±∞ / subnormals so the
/// element-wise min/max walk is deterministic.
fn bounded_f32() -> impl Strategy<Value = f32> {
    -1.0e3_f32..1.0e3_f32
}

fn vertex_strategy() -> impl Strategy<Value = Vertex> {
    (bounded_f32(), bounded_f32(), bounded_f32()).prop_map(|(x, y, z)| Vertex::new([x, y, z]))
}

/// 1-8 vertices wired into a single fan-triangulated face set so the
/// resulting mesh is non-empty. Face normals are arbitrary; the AABB
/// computation only walks vertices.
fn mesh_strategy() -> impl Strategy<Value = IndexedMesh> {
    prop::collection::vec(vertex_strategy(), 1..8usize).prop_map(|vertices| {
        let n = vertices.len() as u32;
        let mut faces = Vec::new();
        if n >= 3 {
            for i in 1..(n - 1) {
                faces.push(IndexedTriangle::from_raw_indices(
                    Normal::new([0.0, 0.0, 1.0]),
                    [0, i, i + 1],
                ));
            }
        }
        IndexedMesh::new(None, vertices, faces)
    })
}

/// Affine: bounded translation × Z rotation × non-uniform scale.
/// Mirrors the strategy in `mesh-core/tests/properties.rs`; restated
/// here because `proptest` strategies aren't shared across crates.
fn affine_strategy() -> impl Strategy<Value = AffineTransform3D> {
    (
        (-5.0_f64..5.0_f64, -5.0_f64..5.0_f64, -5.0_f64..5.0_f64),
        (0.25_f64..4.0_f64, 0.25_f64..4.0_f64, 0.25_f64..4.0_f64),
        -std::f64::consts::PI..std::f64::consts::PI,
    )
        .prop_map(|(t, s, theta_z)| {
            let translation = AffineTransform3D::from_translation([t.0, t.1, t.2]);
            let scale = AffineTransform3D::from_non_uniform_scale([s.0, s.1, s.2]);
            let rot_z = AffineTransform3D::from_rotation_z(theta_z);
            translation.compose(&rot_z).compose(&scale)
        })
}

fn transform_strategy() -> impl Strategy<Value = Transform3D> {
    prop_oneof![
        Just(Transform3D::Identity),
        affine_strategy().prop_map(Transform3D::Affine),
    ]
}

fn leaf_strategy() -> impl Strategy<Value = (IndexedMesh, Transform3D)> {
    (mesh_strategy(), transform_strategy())
}

fn graph_strategy() -> impl Strategy<Value = Scene> {
    // Mirrors the loader topology (e.g. `read_obj`): every object
    // lands as a leaf under a single identity-Group root.
    prop::collection::vec(leaf_strategy(), 1..5usize).prop_map(|leaves| {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group root");
        for (mesh, transform) in leaves {
            let leaf = g
                .add_leaf(None, transform, LeafPayload::Mesh(Arc::new(mesh)))
                .expect("inline leaf");
            g.add_child(root, leaf).expect("attach leaf");
        }
        g
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    /// `Scene::aabb` (uncached) and `scene_aabb` (cached) compute the
    /// same envelope for any scene the renderer or framing camera
    /// might produce. Both arms walk the same data through the same
    /// Arvo formula; the only difference is whether per-mesh AABBs
    /// are recomputed each call or read from `OnceLock`. Equality
    /// must be exact: the cache reads back the same `Aabb` it
    /// stored.
    #[test]
    fn cached_and_uncached_aabb_agree(graph in graph_strategy()) {
        let uncached = graph.aabb().unwrap();
        let spatial = SceneSpatial::primed_graph(graph);
        let cached = scene_aabb(&spatial);
        prop_assert_eq!(uncached, cached);
    }
}
