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
//! - Per-variant invalidation discipline for `SceneEdit`.

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
use sc_mesh_core::scene::{Scene, SceneObject, SceneObjectContent, Transform3D};
use sc_mesh_core::transform::AffineTransform3D;
use sc_mesh_spatial::{SceneEdit, SceneSpatial, scene_aabb};

fn vertex(x: f32, y: f32, z: f32) -> Vertex {
    Vertex::new([x, y, z])
}

/// Small CCW triangle in the XY plane, three fresh vertices.
fn triangle_mesh(scale: f32) -> IndexedMesh {
    let s = scale;
    IndexedMesh::new(
        None,
        vec![vertex(-s, -s, 0.0), vertex(s, -s, 0.0), vertex(0.0, s, 0.0)],
        vec![IndexedTriangle {
            normal: Normal::new([0.0, 0.0, 1.0]),
            vertices: [0, 1, 2],
        }],
    )
}

fn inline_object(mesh: IndexedMesh, transform: Transform3D) -> SceneObject {
    SceneObject {
        meta: None,
        transform,
        content: SceneObjectContent::Mesh(mesh),
    }
}

fn resource_object(resource_idx: usize, transform: Transform3D) -> SceneObject {
    SceneObject {
        meta: None,
        transform,
        content: SceneObjectContent::Resource(resource_idx),
    }
}

mod object_query_binding {
    //! These tests prove the structural pairing: every triple
    //! `(SceneObject, IndexedMesh, MeshSpatial)` returned by
    //! `SceneSpatial::object` comes from the same internal lookup,
    //! so callers cannot mismatch them.

    use super::*;

    #[test]
    fn inline_object_returns_inline_mesh() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![inline_object(triangle_mesh(1.0), Transform3D::Identity)],
        };
        let spatial = SceneSpatial::new(scene);
        let q = spatial.object(0).expect("inline object exists");
        let scene = spatial.scene();
        let SceneObjectContent::Mesh(expected) = &scene.objects[0].content else {
            panic!("expected inline mesh");
        };
        assert!(
            std::ptr::eq(q.mesh(), expected),
            "ObjectQuery::mesh must point at the inline mesh inside scene.objects[idx]"
        );
    }

    #[test]
    fn resource_object_returns_resource_mesh() {
        let scene = Scene {
            meta: None,
            resources: vec![triangle_mesh(1.0)],
            objects: vec![resource_object(0, Transform3D::Identity)],
        };
        let spatial = SceneSpatial::new(scene);
        let q = spatial.object(0).expect("object exists");
        let scene = spatial.scene();
        assert!(
            std::ptr::eq(q.mesh(), &scene.resources[0]),
            "ObjectQuery::mesh must point at scene.resources[r] when content is Resource(r)"
        );
    }

    #[test]
    fn out_of_bounds_object_index_returns_none() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![inline_object(triangle_mesh(1.0), Transform3D::Identity)],
        };
        let spatial = SceneSpatial::new(scene);
        assert!(spatial.object(99).is_none());
    }

    #[test]
    fn dangling_resource_index_returns_none() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![resource_object(99, Transform3D::Identity)],
        };
        let spatial = SceneSpatial::new(scene);
        assert!(spatial.object(0).is_none());
    }

    #[test]
    fn shared_resource_uses_a_single_cache_slot() {
        // Two objects pointing at the same Resource(0). The cache
        // for that resource must be built once and reused — both
        // ObjectQuery handles must observe identical AABB and the
        // same TriangleClusters reference.
        let scene = Scene {
            meta: None,
            resources: vec![triangle_mesh(2.0)],
            objects: vec![
                resource_object(0, Transform3D::Identity),
                resource_object(0, Transform3D::Identity),
            ],
        };
        let spatial = SceneSpatial::new(scene);
        let a = spatial.object(0).unwrap();
        let b = spatial.object(1).unwrap();
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
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![inline_object(triangle_mesh(1.0), Transform3D::Identity)],
        };
        let spatial = SceneSpatial::new(scene);
        let first = spatial.object(0).unwrap().clusters().unwrap() as *const _;
        let second = spatial.object(0).unwrap().clusters().unwrap() as *const _;
        assert_eq!(
            first, second,
            "second lookup must hit the same cached value"
        );
    }

    #[test]
    fn empty_mesh_has_no_local_aabb() {
        let empty_mesh = IndexedMesh::new(None, vec![], vec![]);
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![inline_object(empty_mesh, Transform3D::Identity)],
        };
        let spatial = SceneSpatial::new(scene);
        let q = spatial.object(0).unwrap();
        assert!(q.local_aabb().is_none());
        assert!(q.clusters().is_none());
    }
}

mod cache_priming {
    use super::*;

    #[test]
    fn new_starts_lazy_first_query_builds() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![inline_object(triangle_mesh(1.0), Transform3D::Identity)],
        };
        let spatial = SceneSpatial::new(scene);
        // First query yields the AABB; subsequent queries are
        // cache hits (same value, regardless of OnceLock state
        // visible from outside the crate).
        let aabb = spatial.object(0).unwrap().local_aabb().unwrap();
        assert_eq!(aabb.min().coords(), [-1.0, -1.0, 0.0]);
        assert_eq!(aabb.max().coords(), [1.0, 1.0, 0.0]);
    }

    #[test]
    fn primed_returns_same_values_as_lazy() {
        let mk_scene = || Scene {
            meta: None,
            resources: vec![triangle_mesh(2.0)],
            objects: vec![
                inline_object(triangle_mesh(1.0), Transform3D::Identity),
                resource_object(0, Transform3D::Identity),
            ],
        };
        let lazy = SceneSpatial::new(mk_scene());
        let eager = SceneSpatial::primed(mk_scene());
        for idx in 0..2 {
            let l = lazy.object(idx).unwrap().local_aabb().unwrap();
            let e = eager.object(idx).unwrap().local_aabb().unwrap();
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
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![
                inline_object(triangle_mesh(1.0), Transform3D::Identity),
                inline_object(
                    triangle_mesh(0.5),
                    Transform3D::Affine(AffineTransform3D::from_translation([5.0, 0.0, 0.0])),
                ),
            ],
        };
        let uncached = scene.aabb().unwrap().unwrap();
        let spatial = SceneSpatial::primed(scene);
        let cached = scene_aabb(&spatial).unwrap();
        assert_eq!(uncached, cached);
    }

    #[test]
    fn cached_form_returns_none_for_empty_scene() {
        let spatial = SceneSpatial::new(Scene::default());
        assert!(scene_aabb(&spatial).is_none());
    }

    #[test]
    fn cached_form_skips_empty_meshes() {
        let empty = IndexedMesh::new(None, vec![], vec![]);
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![inline_object(empty, Transform3D::Identity)],
        };
        let spatial = SceneSpatial::new(scene);
        assert!(scene_aabb(&spatial).is_none());
    }
}

mod invalidation_discipline {
    //! For each `SceneEdit` variant, document the invalidation
    //! contract via a runtime test. Future variants get one entry
    //! here when they land; the compile-time match in `apply_edit`
    //! is the first line of defence, this is the second.

    use super::*;

    #[test]
    fn object_transform_does_not_invalidate() {
        // Transform-only edits are object-relative; per-mesh caches
        // are object-space. Applying the edit must not change the
        // cached local AABB.
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![inline_object(triangle_mesh(1.0), Transform3D::Identity)],
        };
        let mut spatial = SceneSpatial::primed(scene);
        let before = spatial.object(0).unwrap().local_aabb().unwrap();
        let new_transform =
            Transform3D::Affine(AffineTransform3D::from_translation([10.0, 0.0, 0.0]));
        spatial.apply_edit(SceneEdit::ObjectTransform {
            object_idx: 0,
            before: Transform3D::Identity,
            after: new_transform,
        });
        let after = spatial.object(0).unwrap().local_aabb().unwrap();
        assert_eq!(
            before, after,
            "ObjectTransform must not invalidate the local AABB cache"
        );
        // The scene's transform actually moved.
        assert_eq!(spatial.scene().objects[0].transform, new_transform);
    }

    #[test]
    fn invalidate_object_clears_inline_cache() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![inline_object(triangle_mesh(1.0), Transform3D::Identity)],
        };
        let mut spatial = SceneSpatial::primed(scene);
        // Prime via a query, then invalidate, then assert the next
        // query still returns the right value (because the cache is
        // rebuilt lazily — invalidation should not corrupt
        // anything, just discard).
        let before = spatial.object(0).unwrap().local_aabb().unwrap();
        spatial.invalidate_object(0);
        let after = spatial.object(0).unwrap().local_aabb().unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn invalidate_resource_clears_resource_cache() {
        let scene = Scene {
            meta: None,
            resources: vec![triangle_mesh(2.0)],
            objects: vec![resource_object(0, Transform3D::Identity)],
        };
        let mut spatial = SceneSpatial::primed(scene);
        let before = spatial.object(0).unwrap().local_aabb().unwrap();
        spatial.invalidate_resource(0);
        let after = spatial.object(0).unwrap().local_aabb().unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn invalidate_object_on_resource_object_is_noop() {
        // Resource objects don't have inline cache slots; calling
        // `invalidate_object` on their idx must be a no-op rather
        // than corrupting the inline vec layout.
        let scene = Scene {
            meta: None,
            resources: vec![triangle_mesh(1.0)],
            objects: vec![resource_object(0, Transform3D::Identity)],
        };
        let mut spatial = SceneSpatial::primed(scene);
        spatial.invalidate_object(0);
        assert!(spatial.object(0).unwrap().local_aabb().is_some());
    }
}

mod scene_edit_basics {
    use super::*;

    #[test]
    fn is_noop_detects_identical_before_after() {
        let edit = SceneEdit::ObjectTransform {
            object_idx: 0,
            before: Transform3D::Identity,
            after: Transform3D::Identity,
        };
        assert!(edit.is_noop());
    }

    #[test]
    fn is_noop_false_when_transforms_differ() {
        let edit = SceneEdit::ObjectTransform {
            object_idx: 0,
            before: Transform3D::Identity,
            after: Transform3D::Affine(AffineTransform3D::from_translation([1.0, 0.0, 0.0])),
        };
        assert!(!edit.is_noop());
    }

    #[test]
    fn inverse_swaps_before_and_after() {
        let after = Transform3D::Affine(AffineTransform3D::from_translation([1.0, 2.0, 3.0]));
        let edit = SceneEdit::ObjectTransform {
            object_idx: 7,
            before: Transform3D::Identity,
            after,
        };
        let inv = edit.inverse();
        assert_eq!(
            inv,
            SceneEdit::ObjectTransform {
                object_idx: 7,
                before: after,
                after: Transform3D::Identity,
            }
        );
    }
}
