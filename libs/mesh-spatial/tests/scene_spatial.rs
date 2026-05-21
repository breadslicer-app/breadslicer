/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! End-to-end tests for the `mesh-spatial` public surface:
//!
//! - `MeshSpatial` cache lifecycle (priming, lazy build, eager build).
//! - `ObjectQuery` binding tests that prove the structural mesh ↔
//!   cache pairing of §2 in the extraction plan.
//! - `scene_aabb` consistency between cached and uncached forms.
//! - Per-variant invalidation discipline for `SceneEdit` (basic enum
//!   behaviour, `is_noop` and `inverse`, is tested in `mesh-core`).

use std::sync::Arc;

use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
use sc_mesh_core::scene_edit::SceneEdit;
use sc_mesh_core::transform::AffineTransform3D;
use sc_mesh_core::transform::Transform3D;
use sc_mesh_core::{IndexedMesh, LeafPayload, MeshResourceId, NodeId, NodeKind, Scene};
use sc_mesh_spatial::{SceneSpatial, scene_aabb};

/// Pre-order leaf `NodeId` of the spatial's first reachable leaf.
/// The integration tests below operate on single- or two-leaf
/// fixtures, so resolving once at the start is enough.
fn first_leaf(spatial: &SceneSpatial) -> NodeId {
    spatial
        .leaf_id_at_index(0)
        .expect("test fixture has at least one leaf")
}

/// `n`-th leaf in pre-order. Used by the few multi-leaf fixtures.
fn nth_leaf(spatial: &SceneSpatial, n: usize) -> NodeId {
    spatial
        .leaf_id_at_index(n)
        .expect("test fixture should expose enough leaves")
}

fn vertex(x: f32, y: f32, z: f32) -> Vertex {
    Vertex::new([x, y, z])
}

/// Small CCW triangle in the XY plane, three fresh vertices.
fn triangle_mesh(scale: f32) -> IndexedMesh {
    let s = scale;
    IndexedMesh::new(
        None,
        vec![vertex(-s, -s, 0.0), vertex(s, -s, 0.0), vertex(0.0, s, 0.0)],
        vec![IndexedTriangle::from_raw_indices(
            Normal::new([0.0, 0.0, 1.0]),
            [0, 1, 2],
        )],
    )
}

/// Fresh graph with a single identity-Group root. Mirrors the
/// topology the loader pipeline installs (`read_obj` and friends
/// land their objects as leaves under an identity-Group root).
/// Returns the graph and the Group's id so helpers can attach
/// leaves directly.
fn graph_with_group_root() -> (Scene, NodeId) {
    let mut g = Scene::new();
    let root = g.add_group(None, Transform3D::Identity);
    g.add_root(root).expect("group root");
    (g, root)
}

/// Attach an inline-mesh leaf as a child of `parent`. Returns the
/// leaf id so tests can address it directly.
fn add_inline_leaf(
    g: &mut Scene,
    parent: NodeId,
    mesh: IndexedMesh,
    transform: Transform3D,
) -> NodeId {
    let leaf = g
        .add_leaf(None, transform, LeafPayload::Mesh(Arc::new(mesh)))
        .expect("inline leaf");
    g.add_child(parent, leaf).expect("attach inline leaf");
    leaf
}

/// Attach a resource-backed leaf as a child of `parent`.
fn add_resource_leaf(
    g: &mut Scene,
    parent: NodeId,
    rid: MeshResourceId,
    transform: Transform3D,
) -> NodeId {
    let leaf = g
        .add_leaf(None, transform, LeafPayload::MeshResource(rid))
        .expect("resource leaf");
    g.add_child(parent, leaf).expect("attach resource leaf");
    leaf
}

mod object_query_binding {
    //! These tests prove the structural pairing: every leaf
    //! `(NodeId, IndexedMesh, MeshSpatial)` triple returned by
    //! `SceneSpatial::leaf` comes from the same internal lookup,
    //! so callers cannot mismatch them.

    use super::*;

    #[test]
    fn inline_object_returns_inline_mesh() {
        let (mut graph, root) = graph_with_group_root();
        add_inline_leaf(&mut graph, root, triangle_mesh(1.0), Transform3D::Identity);
        let spatial = SceneSpatial::from_graph(graph);
        let q = spatial
            .leaf(first_leaf(&spatial))
            .expect("inline object exists");
        let leaf_id = spatial.leaf_id_at_index(0).expect("leaf exists");
        let graph = spatial.graph();
        let node = graph.node(leaf_id).expect("leaf node lives in arena");
        let NodeKind::Leaf(LeafPayload::Mesh(expected)) = &node.kind else {
            panic!("expected inline-mesh leaf");
        };
        assert!(
            std::ptr::eq(q.mesh(), Arc::as_ptr(expected)),
            "ObjectQuery::mesh must point at the inline mesh inside the leaf node"
        );
    }

    #[test]
    fn resource_object_returns_resource_mesh() {
        let (mut graph, root) = graph_with_group_root();
        let rid = graph.add_mesh_resource(triangle_mesh(1.0));
        add_resource_leaf(&mut graph, root, rid, Transform3D::Identity);
        let spatial = SceneSpatial::from_graph(graph);
        let q = spatial.leaf(first_leaf(&spatial)).expect("object exists");
        let leaf_id = spatial.leaf_id_at_index(0).expect("leaf exists");
        let graph = spatial.graph();
        let node = graph.node(leaf_id).expect("leaf node lives in arena");
        let NodeKind::Leaf(LeafPayload::MeshResource(rid)) = &node.kind else {
            panic!("expected resource-backed leaf");
        };
        let expected = graph.mesh_resource(*rid).expect("resource lives in arena");
        assert!(
            std::ptr::eq(q.mesh(), expected),
            "ObjectQuery::mesh must point at the resource mesh when payload is MeshResource(_)"
        );
    }

    #[test]
    fn out_of_bounds_object_index_returns_none() {
        let (mut graph, root) = graph_with_group_root();
        add_inline_leaf(&mut graph, root, triangle_mesh(1.0), Transform3D::Identity);
        let spatial = SceneSpatial::from_graph(graph);
        assert!(spatial.leaf_id_at_index(99).is_none());
    }

    #[test]
    fn shared_resource_uses_a_single_cache_slot() {
        // Two objects pointing at the same Resource(0). The cache
        // for that resource must be built once and reused: both
        // ObjectQuery handles must observe identical AABB and the
        // same TriangleClusters reference.
        let (mut graph, root) = graph_with_group_root();
        let rid = graph.add_mesh_resource(triangle_mesh(2.0));
        add_resource_leaf(&mut graph, root, rid, Transform3D::Identity);
        add_resource_leaf(&mut graph, root, rid, Transform3D::Identity);
        let spatial = SceneSpatial::from_graph(graph);
        let a = spatial.leaf(first_leaf(&spatial)).unwrap();
        let b = spatial.leaf(nth_leaf(&spatial, 1)).unwrap();
        let aabb_a = a.local_aabb().unwrap();
        let aabb_b = b.local_aabb().unwrap();
        assert_eq!(aabb_a, aabb_b);
        // Same resource → same cluster cache reference.
        let clusters_a = a.clusters().unwrap() as *const _;
        let clusters_b = b.clusters().unwrap() as *const _;
        assert_eq!(
            clusters_a, clusters_b,
            "resource-shared cache slots must hand out the same cluster cache"
        );
    }

    #[test]
    fn repeated_lookups_are_cache_stable() {
        let (mut graph, root) = graph_with_group_root();
        add_inline_leaf(&mut graph, root, triangle_mesh(1.0), Transform3D::Identity);
        let spatial = SceneSpatial::from_graph(graph);
        let first = spatial
            .leaf(first_leaf(&spatial))
            .unwrap()
            .clusters()
            .unwrap() as *const _;
        let second = spatial
            .leaf(first_leaf(&spatial))
            .unwrap()
            .clusters()
            .unwrap() as *const _;
        assert_eq!(
            first, second,
            "second lookup must hit the same cached value"
        );
    }

    #[test]
    fn empty_mesh_has_no_local_aabb() {
        let empty_mesh = IndexedMesh::new(None, vec![], vec![]);
        let (mut graph, root) = graph_with_group_root();
        add_inline_leaf(&mut graph, root, empty_mesh, Transform3D::Identity);
        let spatial = SceneSpatial::from_graph(graph);
        let q = spatial.leaf(first_leaf(&spatial)).unwrap();
        assert!(q.local_aabb().is_none());
        assert!(q.clusters().is_none());
    }
}

mod cache_priming {
    use super::*;

    #[test]
    fn new_starts_lazy_first_query_builds() {
        let (mut graph, root) = graph_with_group_root();
        add_inline_leaf(&mut graph, root, triangle_mesh(1.0), Transform3D::Identity);
        let spatial = SceneSpatial::from_graph(graph);
        // First query yields the AABB; subsequent queries are
        // cache hits (same value, regardless of OnceLock state
        // visible from outside the crate).
        let aabb = spatial
            .leaf(first_leaf(&spatial))
            .unwrap()
            .local_aabb()
            .unwrap();
        assert_eq!(aabb.min().coords(), [-1.0, -1.0, 0.0]);
        assert_eq!(aabb.max().coords(), [1.0, 1.0, 0.0]);
    }

    #[test]
    fn primed_returns_same_values_as_lazy() {
        let mk_graph = || {
            let (mut g, root) = graph_with_group_root();
            let rid = g.add_mesh_resource(triangle_mesh(2.0));
            add_inline_leaf(&mut g, root, triangle_mesh(1.0), Transform3D::Identity);
            add_resource_leaf(&mut g, root, rid, Transform3D::Identity);
            g
        };
        let lazy = SceneSpatial::from_graph(mk_graph());
        let eager = SceneSpatial::primed_graph(mk_graph());
        for idx in 0..2 {
            let l = lazy
                .leaf(nth_leaf(&lazy, idx))
                .unwrap()
                .local_aabb()
                .unwrap();
            let e = eager
                .leaf(nth_leaf(&eager, idx))
                .unwrap()
                .local_aabb()
                .unwrap();
            assert_eq!(l, e);
        }
    }

    /// Compile-time proof that the off-thread priming claim holds:
    /// `SceneSpatial: Send`. If the auto-Send bound ever breaks
    /// (e.g. someone adds a `Cell` or `Rc` field), this test fails
    /// to compile and the eager-priming-on-worker-thread story is
    /// gone.
    #[test]
    fn scene_spatial_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<SceneSpatial>();
    }
}

mod scene_aabb_consistency {
    use super::*;

    #[test]
    fn cached_form_agrees_with_uncached_for_identity_scene() {
        let (mut graph, root) = graph_with_group_root();
        add_inline_leaf(&mut graph, root, triangle_mesh(1.0), Transform3D::Identity);
        add_inline_leaf(
            &mut graph,
            root,
            triangle_mesh(0.5),
            Transform3D::Affine(AffineTransform3D::from_translation([5.0, 0.0, 0.0])),
        );
        let uncached = graph.aabb().unwrap().unwrap();
        let spatial = SceneSpatial::primed_graph(graph);
        let cached = scene_aabb(&spatial).unwrap();
        assert_eq!(uncached, cached);
    }

    #[test]
    fn cached_form_returns_none_for_empty_scene() {
        let (graph, _root) = graph_with_group_root();
        let spatial = SceneSpatial::from_graph(graph);
        assert!(scene_aabb(&spatial).is_none());
    }

    #[test]
    fn cached_form_skips_empty_meshes() {
        let empty = IndexedMesh::new(None, vec![], vec![]);
        let (mut graph, root) = graph_with_group_root();
        add_inline_leaf(&mut graph, root, empty, Transform3D::Identity);
        let spatial = SceneSpatial::from_graph(graph);
        assert!(scene_aabb(&spatial).is_none());
    }

    /// `Scene::aabb` (in `mesh-core`) and `scene_aabb` (in
    /// `mesh-spatial`) implement the same Arvo loop at different
    /// caching layers; review §3.4 flagged the duplication. These
    /// cases pin the cross-layer equivalence on every transform
    /// shape the renderer is likely to produce, so a drift between
    /// the two surfaces fails CI rather than picker / framing.
    fn assert_cached_matches_uncached(mut build: impl FnMut() -> Scene) {
        let uncached = build().aabb().unwrap();
        let spatial = SceneSpatial::primed_graph(build());
        let cached = scene_aabb(&spatial);
        assert_eq!(uncached, cached);
    }

    #[test]
    fn cached_form_agrees_under_pure_rotation() {
        assert_cached_matches_uncached(|| {
            let (mut g, root) = graph_with_group_root();
            add_inline_leaf(
                &mut g,
                root,
                triangle_mesh(1.0),
                Transform3D::Affine(AffineTransform3D::from_rotation_z(
                    std::f64::consts::FRAC_PI_4,
                )),
            );
            g
        });
    }

    #[test]
    fn cached_form_agrees_under_mirror() {
        // Negative-determinant transform path; the renderer's
        // winding-reversal logic depends on the cached AABB matching
        // the uncached one bit-for-bit so the framing camera is
        // identical.
        assert_cached_matches_uncached(|| {
            let (mut g, root) = graph_with_group_root();
            add_inline_leaf(
                &mut g,
                root,
                triangle_mesh(1.0),
                Transform3D::Affine(AffineTransform3D::from_non_uniform_scale([-1.0, 1.0, 1.0])),
            );
            g
        });
    }

    #[test]
    fn cached_form_agrees_for_multi_object_scene() {
        assert_cached_matches_uncached(|| {
            let (mut g, root) = graph_with_group_root();
            add_inline_leaf(&mut g, root, triangle_mesh(1.0), Transform3D::Identity);
            add_inline_leaf(
                &mut g,
                root,
                triangle_mesh(0.5),
                Transform3D::Affine(AffineTransform3D::from_translation([5.0, 0.0, 0.0])),
            );
            add_inline_leaf(
                &mut g,
                root,
                triangle_mesh(2.0),
                Transform3D::Affine(AffineTransform3D::from_rotation_z(0.7)),
            );
            g
        });
    }

    #[test]
    fn cached_form_agrees_with_resource_objects() {
        // Two scene objects that share a single resource mesh;
        // exercises the cache slot merging path.
        assert_cached_matches_uncached(|| {
            let (mut g, root) = graph_with_group_root();
            let rid = g.add_mesh_resource(triangle_mesh(2.0));
            add_resource_leaf(&mut g, root, rid, Transform3D::Identity);
            add_resource_leaf(
                &mut g,
                root,
                rid,
                Transform3D::Affine(AffineTransform3D::from_translation([10.0, 0.0, 0.0])),
            );
            g
        });
    }
}

mod invalidation_discipline {
    //! For each `SceneEdit` variant, document the invalidation
    //! contract via a runtime test. Future variants get one entry
    //! here when they land; the compile-time match in `apply_edit`
    //! is the first line of defence, this is the second.

    use super::*;

    #[test]
    fn set_local_transform_does_not_invalidate() {
        // Transform-only edits are object-relative; per-mesh caches
        // are object-space. Applying the edit must not change the
        // cached local AABB.
        let (mut graph, root) = graph_with_group_root();
        add_inline_leaf(&mut graph, root, triangle_mesh(1.0), Transform3D::Identity);
        let mut spatial = SceneSpatial::primed_graph(graph);
        let leaf_id = spatial.leaf_id_at_index(0).unwrap();
        let before = spatial.leaf(leaf_id).unwrap().local_aabb().unwrap();
        let new_transform =
            Transform3D::Affine(AffineTransform3D::from_translation([10.0, 0.0, 0.0]));
        spatial.apply_edit(SceneEdit::SetLocalTransform {
            node_id: leaf_id,
            before: Transform3D::Identity,
            after: new_transform,
        });
        let after = spatial.leaf(leaf_id).unwrap().local_aabb().unwrap();
        assert_eq!(
            before, after,
            "SetLocalTransform must not invalidate the local AABB cache"
        );
        // The leaf's local transform actually moved.
        assert_eq!(
            spatial.graph().node(leaf_id).unwrap().local_transform,
            new_transform
        );
    }

    #[test]
    fn invalidate_leaf_clears_inline_cache() {
        let (mut graph, root) = graph_with_group_root();
        add_inline_leaf(&mut graph, root, triangle_mesh(1.0), Transform3D::Identity);
        let mut spatial = SceneSpatial::primed_graph(graph);
        // Prime via a query, then invalidate, then assert the next
        // query still returns the right value (because the cache is
        // rebuilt lazily; invalidation should not corrupt
        // anything, just discard).
        let leaf_id = spatial.leaf_id_at_index(0).unwrap();
        let before = spatial.leaf(leaf_id).unwrap().local_aabb().unwrap();
        spatial.invalidate_leaf(leaf_id);
        let after = spatial.leaf(leaf_id).unwrap().local_aabb().unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn invalidate_mesh_resource_clears_resource_cache() {
        let (mut graph, root) = graph_with_group_root();
        let rid = graph.add_mesh_resource(triangle_mesh(2.0));
        add_resource_leaf(&mut graph, root, rid, Transform3D::Identity);
        let mut spatial = SceneSpatial::primed_graph(graph);
        let leaf_id = spatial.leaf_id_at_index(0).unwrap();
        let before = spatial.leaf(leaf_id).unwrap().local_aabb().unwrap();
        spatial.invalidate_mesh_resource(rid);
        let after = spatial.leaf(leaf_id).unwrap().local_aabb().unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn invalidate_leaf_on_resource_backed_leaf_preserves_resource_cache() {
        // A resource-backed leaf has no inline cache slot; calling
        // `invalidate_leaf` on it is a no-op for that leaf and must
        // not corrupt the resource cache the leaf reads from.
        let (mut graph, root) = graph_with_group_root();
        let rid = graph.add_mesh_resource(triangle_mesh(1.0));
        add_resource_leaf(&mut graph, root, rid, Transform3D::Identity);
        let mut spatial = SceneSpatial::primed_graph(graph);
        let leaf_id = spatial.leaf_id_at_index(0).unwrap();
        spatial.invalidate_leaf(leaf_id);
        assert!(spatial.leaf(leaf_id).unwrap().local_aabb().is_some());
    }

    #[test]
    fn allocate_unbound_node_id_keeps_leaves_cache_in_sync() {
        let (mut graph, _root) = graph_with_group_root();
        let _rid = graph.add_mesh_resource(triangle_mesh(1.0));
        let mut spatial = SceneSpatial::primed_graph(graph);
        let before = spatial.graph().nodes_len();
        let id = spatial.allocate_unbound_node_id();
        assert_eq!(spatial.graph().nodes_len(), before + 1);
        // The new id is not a live node, not in roots, and the
        // spatial query path returns None for it.
        assert!(spatial.graph().node(id).is_none());
        assert!(!spatial.graph().roots().contains(&id));
        assert!(spatial.leaf(id).is_none());
    }

    #[test]
    fn add_root_leaf_edit_makes_the_new_leaf_queryable() {
        // Walks the full paste flow at the spatial layer: allocate
        // an unbound id, build an AddRootLeaf edit, apply it, then
        // query the new leaf through the cache-aware path.
        let (mut graph, _root) = graph_with_group_root();
        let rid = graph.add_mesh_resource(triangle_mesh(1.0));
        let mut spatial = SceneSpatial::primed_graph(graph);
        let id = spatial.allocate_unbound_node_id();
        let xform = Transform3D::Affine(AffineTransform3D::from_translation([7.0, 8.0, 9.0]));
        spatial.apply_edit(SceneEdit::AddRootLeaf {
            node_id: id,
            mesh: rid,
            local_transform: xform,
        });
        assert!(spatial.graph().roots().contains(&id));
        let q = spatial.leaf(id).expect("new leaf is queryable");
        assert!(q.local_aabb().is_some());
    }

    #[test]
    fn remove_root_leaf_edit_undoes_a_prior_add() {
        let (mut graph, _root) = graph_with_group_root();
        let rid = graph.add_mesh_resource(triangle_mesh(1.0));
        let mut spatial = SceneSpatial::primed_graph(graph);
        let id = spatial.allocate_unbound_node_id();
        let xform = Transform3D::Identity;
        spatial.apply_edit(SceneEdit::AddRootLeaf {
            node_id: id,
            mesh: rid,
            local_transform: xform,
        });
        assert!(spatial.leaf(id).is_some());
        spatial.apply_edit(SceneEdit::RemoveRootLeaf {
            node_id: id,
            mesh: rid,
            local_transform: xform,
        });
        assert!(spatial.leaf(id).is_none());
        assert!(!spatial.graph().roots().contains(&id));
    }

    #[test]
    fn add_remove_add_round_trips_through_the_spatial() {
        let (mut graph, _root) = graph_with_group_root();
        let rid = graph.add_mesh_resource(triangle_mesh(1.0));
        let mut spatial = SceneSpatial::primed_graph(graph);
        let id = spatial.allocate_unbound_node_id();
        let xform = Transform3D::Affine(AffineTransform3D::from_translation([3.0, 4.0, 5.0]));
        let add = SceneEdit::AddRootLeaf {
            node_id: id,
            mesh: rid,
            local_transform: xform,
        };
        let remove = add.inverse();
        spatial.apply_edit(add.clone());
        let aabb_first = spatial.leaf(id).unwrap().local_aabb().unwrap();
        spatial.apply_edit(remove);
        assert!(spatial.leaf(id).is_none());
        spatial.apply_edit(add);
        let aabb_redo = spatial.leaf(id).unwrap().local_aabb().unwrap();
        assert_eq!(
            aabb_first, aabb_redo,
            "the mesh-resource cache survives the round-trip so the local AABB matches"
        );
    }

    #[test]
    fn allocate_unbound_mesh_resource_id_extends_per_resource_cache() {
        let (graph, _root) = graph_with_group_root();
        let mut spatial = SceneSpatial::primed_graph(graph);
        let before = spatial.graph().mesh_resources_len();
        let _rid = spatial.allocate_unbound_mesh_resource_id();
        assert_eq!(spatial.graph().mesh_resources_len(), before + 1);
    }

    #[test]
    fn add_mesh_resource_edit_installs_at_allocated_slot() {
        let (graph, _root) = graph_with_group_root();
        let mut spatial = SceneSpatial::primed_graph(graph);
        let rid = spatial.allocate_unbound_mesh_resource_id();
        let arc = Arc::new(triangle_mesh(1.0));
        spatial.apply_edit(SceneEdit::AddMeshResource {
            rid,
            mesh: Arc::clone(&arc),
        });
        assert!(std::ptr::eq(
            Arc::as_ptr(&arc),
            Arc::as_ptr(spatial.graph().mesh_resource_arc(rid).unwrap())
        ));
    }

    #[test]
    fn compound_paste_shape_round_trips_through_spatial() {
        // The minimal paste compound: Add the mesh, then add the
        // root leaf referencing it. Undo (Compound's inverse) must
        // remove the leaf first, then the mesh resource (the
        // RemoveMeshResource precondition rejects in-use ids).
        let (graph, _root) = graph_with_group_root();
        let mut spatial = SceneSpatial::primed_graph(graph);
        let rid = spatial.allocate_unbound_mesh_resource_id();
        let nid = spatial.allocate_unbound_node_id();
        let arc = Arc::new(triangle_mesh(1.0));
        let xform = Transform3D::Identity;
        let compound = SceneEdit::Compound(vec![
            SceneEdit::AddMeshResource {
                rid,
                mesh: Arc::clone(&arc),
            },
            SceneEdit::AddRootLeaf {
                node_id: nid,
                mesh: rid,
                local_transform: xform,
            },
        ]);
        spatial.apply_edit(compound.clone());
        assert!(spatial.leaf(nid).is_some());
        assert!(spatial.graph().mesh_resource(rid).is_some());
        spatial.apply_edit(compound.inverse());
        assert!(spatial.leaf(nid).is_none());
        assert!(spatial.graph().mesh_resource(rid).is_none());
    }
}

mod transformed_aabb {
    //! `ObjectQuery::transformed_aabb` returns the AABB of the
    //! mesh's transformed vertices: tight for any mesh, including
    //! rotated non-box meshes where Arvo's AABB-of-AABB is strictly
    //! larger than the AABB-of-vertices.

    use super::*;
    use float_cmp::assert_approx_eq;
    use std::f64::consts::FRAC_PI_4;

    /// Six-vertex octahedron skeleton on the unit axes. The local
    /// AABB is the unit cube `[-1, 1]^3`, but no vertex reaches a
    /// corner: the gap is what makes Arvo's transformed-AABB
    /// strictly larger than the actual rotated-vertex AABB.
    fn octahedron_mesh() -> IndexedMesh {
        IndexedMesh::new(
            None,
            vec![
                vertex(1.0, 0.0, 0.0),
                vertex(-1.0, 0.0, 0.0),
                vertex(0.0, 1.0, 0.0),
                vertex(0.0, -1.0, 0.0),
                vertex(0.0, 0.0, 1.0),
                vertex(0.0, 0.0, -1.0),
            ],
            vec![],
        )
    }

    /// Axis-aligned unit cube (eight corner vertices). Fills its
    /// local AABB: Arvo and per-vertex agree under any linear, so
    /// these tests serve as a baseline that the cube case is
    /// unchanged.
    fn cube_mesh() -> IndexedMesh {
        IndexedMesh::new(
            None,
            vec![
                vertex(-1.0, -1.0, -1.0),
                vertex(1.0, -1.0, -1.0),
                vertex(-1.0, 1.0, -1.0),
                vertex(1.0, 1.0, -1.0),
                vertex(-1.0, -1.0, 1.0),
                vertex(1.0, -1.0, 1.0),
                vertex(-1.0, 1.0, 1.0),
                vertex(1.0, 1.0, 1.0),
            ],
            vec![],
        )
    }

    fn one_object(mesh: IndexedMesh, transform: Transform3D) -> SceneSpatial {
        let (mut g, root) = graph_with_group_root();
        add_inline_leaf(&mut g, root, mesh, transform);
        SceneSpatial::from_graph(g)
    }

    fn assert_aabb_approx(actual: sc_mesh_core::Aabb, min: [f32; 3], max: [f32; 3], eps: f32) {
        let amin = actual.min().coords();
        let amax = actual.max().coords();
        for i in 0..3 {
            assert_approx_eq!(f32, amin[i], min[i], epsilon = eps);
            assert_approx_eq!(f32, amax[i], max[i], epsilon = eps);
        }
    }

    #[test]
    fn identity_returns_local_aabb() {
        let spatial = one_object(octahedron_mesh(), Transform3D::Identity);
        let q = spatial.leaf(first_leaf(&spatial)).unwrap();
        assert_eq!(q.transformed_aabb(Transform3D::Identity), q.local_aabb());
    }

    #[test]
    fn pure_translation_shifts_local_aabb() {
        let spatial = one_object(octahedron_mesh(), Transform3D::Identity);
        let q = spatial.leaf(first_leaf(&spatial)).unwrap();
        let t = Transform3D::Affine(AffineTransform3D::from_translation([10.0, -5.0, 2.0]));
        assert_aabb_approx(
            q.transformed_aabb(t).unwrap(),
            [9.0, -6.0, 1.0],
            [11.0, -4.0, 3.0],
            1e-5,
        );
    }

    #[test]
    fn rotated_octahedron_is_tighter_than_arvo_for_unit_cube_aabb() {
        // The octahedron's local AABB is the unit cube; Arvo's box
        // for a 45° Z rotation of a unit cube would extend ±√2 on
        // X and Y. The actual rotated octahedron only reaches ±√2/2
        // on those axes (the rotated tips of (±1, 0, 0) / (0, ±1, 0)).
        // Z is unrotated, so it stays at ±1.
        let r = AffineTransform3D::from_rotation_z(FRAC_PI_4);
        let spatial = one_object(octahedron_mesh(), Transform3D::Affine(r));
        let q = spatial.leaf(first_leaf(&spatial)).unwrap();
        let aabb = q.transformed_aabb(Transform3D::Affine(r)).unwrap();
        let half = std::f32::consts::FRAC_1_SQRT_2; // = √2 / 2.
        assert_aabb_approx(aabb, [-half, -half, -1.0], [half, half, 1.0], 1e-5);
    }

    #[test]
    fn rotated_cube_matches_arvos_corner_aabb() {
        // A cube fills its local AABB, so the tight per-vertex AABB
        // and Arvo's AABB-of-AABB agree. After a 45° Z rotation,
        // both produce X/Y half-extents of √2 and Z half-extents
        // unchanged.
        let r = AffineTransform3D::from_rotation_z(FRAC_PI_4);
        let spatial = one_object(cube_mesh(), Transform3D::Affine(r));
        let q = spatial.leaf(first_leaf(&spatial)).unwrap();
        let aabb = q.transformed_aabb(Transform3D::Affine(r)).unwrap();
        let s = std::f32::consts::SQRT_2;
        assert_aabb_approx(aabb, [-s, -s, -1.0], [s, s, 1.0], 1e-5);
    }

    #[test]
    fn rotation_then_translation_composes_per_vertex_and_shift() {
        // The whole transform = R(45° Z) then T(+10, +20, +30).
        // Per-vertex computation gives the rotated octahedron AABB
        // ±√2/2 on X/Y, ±1 on Z; the translation shifts the corners.
        let composed = AffineTransform3D::from_translation([10.0, 20.0, 30.0])
            .compose(&AffineTransform3D::from_rotation_z(FRAC_PI_4));
        let spatial = one_object(octahedron_mesh(), Transform3D::Affine(composed));
        let q = spatial.leaf(first_leaf(&spatial)).unwrap();
        let aabb = q.transformed_aabb(Transform3D::Affine(composed)).unwrap();
        let half = std::f32::consts::FRAC_1_SQRT_2;
        assert_aabb_approx(
            aabb,
            [10.0 - half, 20.0 - half, 29.0],
            [10.0 + half, 20.0 + half, 31.0],
            1e-5,
        );
    }

    #[test]
    fn empty_mesh_returns_none() {
        let spatial = one_object(
            IndexedMesh::new(None, vec![], vec![]),
            Transform3D::Identity,
        );
        let q = spatial.leaf(first_leaf(&spatial)).unwrap();
        assert!(q.transformed_aabb(Transform3D::Identity).is_none());
        let t = Transform3D::Affine(AffineTransform3D::from_translation([1.0, 2.0, 3.0]));
        assert!(q.transformed_aabb(t).is_none());
    }

    #[test]
    fn cache_hits_across_translation_changes() {
        // Two queries with the same linear part but different
        // translations must agree on the AABB modulo the translation
        // shift. This exercises the "linear part is the cache key,
        // translation is a post-shift" contract: a translate drag
        // changes only the translation tick to tick and must reuse
        // the cached linear AABB.
        let r = AffineTransform3D::from_rotation_z(FRAC_PI_4);
        let t1 =
            Transform3D::Affine(AffineTransform3D::from_translation([1.0, 0.0, 0.0]).compose(&r));
        let t2 =
            Transform3D::Affine(AffineTransform3D::from_translation([0.0, 1.0, 0.0]).compose(&r));
        let spatial = one_object(octahedron_mesh(), Transform3D::Identity);
        let q = spatial.leaf(first_leaf(&spatial)).unwrap();
        let a1 = q.transformed_aabb(t1).unwrap();
        let a2 = q.transformed_aabb(t2).unwrap();
        // Each component of (a2 - a1) must equal (t2_translation - t1_translation).
        let a1_min = a1.min().coords();
        let a1_max = a1.max().coords();
        let a2_min = a2.min().coords();
        let a2_max = a2.max().coords();
        assert_approx_eq!(f32, a2_min[0] - a1_min[0], -1.0, epsilon = 1e-5);
        assert_approx_eq!(f32, a2_min[1] - a1_min[1], 1.0, epsilon = 1e-5);
        assert_approx_eq!(f32, a2_min[2] - a1_min[2], 0.0, epsilon = 1e-5);
        assert_approx_eq!(f32, a2_max[0] - a1_max[0], -1.0, epsilon = 1e-5);
        assert_approx_eq!(f32, a2_max[1] - a1_max[1], 1.0, epsilon = 1e-5);
        assert_approx_eq!(f32, a2_max[2] - a1_max[2], 0.0, epsilon = 1e-5);
    }

    #[test]
    fn linear_change_replaces_cached_entry() {
        // After querying with one linear, querying with a different
        // linear must return the AABB for the new linear (not stale
        // data from the first one). This guards against a future
        // refactor that turns the slot into "remember the first
        // linear" by accident.
        let spatial = one_object(octahedron_mesh(), Transform3D::Identity);
        let q = spatial.leaf(first_leaf(&spatial)).unwrap();
        let identity = Transform3D::Affine(AffineTransform3D::identity());
        let rotated = Transform3D::Affine(AffineTransform3D::from_rotation_z(FRAC_PI_4));

        let a_identity = q.transformed_aabb(identity).unwrap();
        assert_aabb_approx(a_identity, [-1.0, -1.0, -1.0], [1.0, 1.0, 1.0], 1e-5);

        let a_rotated = q.transformed_aabb(rotated).unwrap();
        let half = std::f32::consts::FRAC_1_SQRT_2;
        assert_aabb_approx(a_rotated, [-half, -half, -1.0], [half, half, 1.0], 1e-5);

        // Re-query identity: must be the identity AABB again, not
        // the cached rotated one.
        let a_identity_again = q.transformed_aabb(identity).unwrap();
        assert_eq!(a_identity, a_identity_again);
    }
}
