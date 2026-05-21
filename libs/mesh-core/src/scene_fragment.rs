/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Self-contained scene subgraph used as the clipboard payload for
//! copy / paste.
//!
//! A [`SceneFragment`] wraps a freestanding [`Scene`] that carries the
//! roots, nodes and mesh resources the user selected, all in their
//! own arenas. The paste primitive in the document layer walks the
//! fragment, allocates target ids in the destination scene's arenas,
//! emits the matching `AddMeshResource` / `AddRootLeaf` (and later
//! `AddGroup` / `AddCsg` / `AddInstance`) typed edits, and wraps the
//! whole thing in a single [`crate::scene_edit::SceneEdit::Compound`].
//!
//! The current builder only populates the fragment with a single
//! root leaf and a single mesh resource. The same type generalises
//! to subtrees (group / CSG / instances) without a redesign: the
//! wrapper exposes the underlying `Scene` for reads, and growth on
//! the paste side is variant-additive rather than shape-changing.

use std::sync::Arc;

use crate::indexed_mesh::IndexedMesh;
use crate::scene::Scene;
use crate::scene_node::LeafPayload;
use crate::transform::Transform3D;

/// Self-contained scene subgraph. The newtype around [`Scene`] makes
/// the role explicit at API boundaries (a paste primitive that takes
/// `&SceneFragment` cannot be mistakenly handed the user's live
/// scene), and gives a place to hang fragment-specific
/// constructors without polluting the `Scene` surface.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneFragment {
    inner: Scene,
}

impl SceneFragment {
    /// Build a one-root fragment that wraps `mesh` as a mesh resource
    /// and attaches a single leaf referencing it under
    /// `local_transform`. The leaf becomes the only scene root; the
    /// fragment owns the `Arc` (and its refcount), which is what the
    /// paste path needs to do an `Arc::as_ptr` dedup against the
    /// target scene's mesh-resource arena.
    ///
    /// Used by the copy path: a single-leaf selection turns into a
    /// fragment of this shape.
    pub fn from_leaf_snapshot(mesh: Arc<IndexedMesh>, local_transform: Transform3D) -> Self {
        let mut inner = Scene::new();
        let rid = inner.add_mesh_resource_arc(mesh);
        let leaf = inner
            .add_leaf(None, local_transform, LeafPayload::MeshResource(rid))
            .expect("freshly registered resource id is live");
        inner
            .add_root(leaf)
            .expect("freshly allocated leaf has no parent and is not a root");
        Self { inner }
    }

    /// Read-only access to the wrapped scene. Paste reads roots,
    /// nodes and mesh resources through this accessor.
    pub fn scene(&self) -> &Scene {
        &self.inner
    }

    /// Consume the fragment and return its wrapped scene. Tests use
    /// this to assert on the underlying graph shape; production
    /// callers should prefer [`Self::scene`] for read-only access.
    pub fn into_scene(self) -> Scene {
        self.inner
    }
}

#[cfg(test)]
mod scene_fragment_tests {
    use super::*;
    use crate::primitives::{IndexedTriangle, Normal, Vertex};
    use crate::scene_node::NodeKind;
    use crate::transform::AffineTransform3D;

    fn triangle_arc() -> Arc<IndexedMesh> {
        Arc::new(IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([0.0, 0.0, 1.0]),
                [0, 1, 2],
            )],
        })
    }

    #[test]
    fn from_leaf_snapshot_builds_one_root_one_resource() {
        let mesh = triangle_arc();
        let fragment = SceneFragment::from_leaf_snapshot(Arc::clone(&mesh), Transform3D::Identity);
        let scene = fragment.scene();
        assert_eq!(scene.roots().len(), 1);
        assert_eq!(scene.mesh_resources_len(), 1);
        let root = scene.roots()[0];
        let node = scene.node(root).expect("root lives");
        let NodeKind::Leaf(LeafPayload::MeshResource(rid)) = &node.kind else {
            panic!("expected the root to be a mesh-resource leaf");
        };
        assert!(std::ptr::eq(
            Arc::as_ptr(&mesh),
            Arc::as_ptr(scene.mesh_resource_arc(*rid).unwrap())
        ));
    }

    #[test]
    fn from_leaf_snapshot_carries_the_provided_local_transform() {
        let xform = Transform3D::Affine(AffineTransform3D::from_translation([1.0, 2.0, 3.0]));
        let fragment = SceneFragment::from_leaf_snapshot(triangle_arc(), xform);
        let scene = fragment.scene();
        let root = scene.roots()[0];
        assert_eq!(scene.node(root).unwrap().local_transform, xform);
    }

    #[test]
    fn from_leaf_snapshot_arc_identity_is_preserved() {
        // The paste path's dedup hinges on this: the Arc the
        // clipboard holds and the Arc the fragment registers must
        // be the same pointer, so `find_mesh_resource_by_arc` on the
        // target scene can match them.
        let mesh = triangle_arc();
        let fragment = SceneFragment::from_leaf_snapshot(Arc::clone(&mesh), Transform3D::Identity);
        let stored = fragment
            .scene()
            .mesh_resources_iter()
            .next()
            .map(|(_, m)| m as *const IndexedMesh)
            .unwrap();
        assert_eq!(stored, Arc::as_ptr(&mesh));
    }
}
