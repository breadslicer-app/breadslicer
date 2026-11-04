/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Self-contained scene subgraph used as the clipboard payload for copy and
//! paste.
//!
//! A [`SceneFragment`] wraps a freestanding [`Scene`] that carries the roots,
//! nodes and mesh resources the user selected, all in their own arenas. The
//! paste primitive in the document layer walks the fragment, allocates target
//! ids in the arenas of the destination scene, emits the matching
//! `AddMeshResource` and `AddLeaf` typed edits, and wraps the whole thing in a
//! single [`crate::edit::SceneEdit::Compound`].
//!
//! [`SceneFragment::from_leaf_snapshot`] builds a fragment with a single root
//! leaf and a single mesh resource.

use std::sync::Arc;

use sc_mesh_core::indexed_mesh::IndexedMesh;
use sc_mesh_core::transform::Transform3D;
use sc_scene::node::LeafPayload;
use sc_scene::{PaintKind, Scene, SurfaceTags};

/// Self-contained scene subgraph. The newtype around [`Scene`] makes the role
/// explicit at API boundaries (a paste primitive that takes `&SceneFragment`
/// cannot be handed the live scene of the user by mistake), and gives a place
/// to hang fragment-specific constructors without polluting the `Scene`
/// surface.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneFragment {
    inner: Scene,
}

impl SceneFragment {
    /// Builds a 1-root fragment that wraps `mesh` as a mesh resource and
    /// attaches a single leaf referencing it under `local_transform`. The leaf
    /// becomes the only scene root; the fragment owns the `Arc` (and its
    /// refcount), which the paste path needs for an `Arc::as_ptr` dedup against
    /// the mesh-resource arena of the target scene.
    ///
    /// Used by the copy path: a single-leaf selection turns into a fragment of
    /// this shape.
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

    /// This fragment with `paint` on its first root: a copy keeps the paint of
    /// its model (#183). Paint of another mesh is left out.
    #[must_use]
    pub fn with_paint(mut self, paint: Vec<(PaintKind, SurfaceTags)>) -> Self {
        if let Some(&root) = self.inner.roots().first() {
            for (kind, tags) in paint {
                let _ = self.inner.set_surface_tags(root, kind, Some(tags));
            }
        }
        self
    }

    /// Builds a fragment of one root leaf per entry of `leaves`, in order,
    /// each wrapping its mesh as a mesh resource of its own under its local
    /// transform: several objects pasted as one gesture.
    pub fn from_leaf_snapshots(
        leaves: impl IntoIterator<Item = (Arc<IndexedMesh>, Transform3D)>,
    ) -> Self {
        let mut inner = Scene::new();
        for (mesh, local_transform) in leaves {
            let rid = inner.add_mesh_resource_arc(mesh);
            let leaf = inner
                .add_leaf(None, local_transform, LeafPayload::MeshResource(rid))
                .expect("freshly registered resource id is live");
            inner
                .add_root(leaf)
                .expect("freshly allocated leaf has no parent and is not a root");
        }
        Self { inner }
    }

    /// Borrows the wrapped scene. Paste reads roots, nodes and mesh resources
    /// through it.
    pub fn scene(&self) -> &Scene {
        &self.inner
    }

    /// Consumes the fragment and returns its wrapped scene. Tests use this to
    /// assert on the underlying graph shape; production callers should prefer
    /// [`Self::scene`] for read-only access.
    pub fn into_scene(self) -> Scene {
        self.inner
    }
}

#[cfg(test)]
mod scene_fragment_tests {
    use super::*;
    use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
    use sc_mesh_core::transform::AffineTransform3D;
    use sc_scene::node::NodeKind;

    fn triangle_arc() -> Arc<IndexedMesh> {
        Arc::new(IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
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
        // The dedup of the paste path depends on this: the Arc the clipboard
        // holds and the Arc the fragment registers must be the same pointer, so
        // `find_mesh_resource_by_arc` on the target scene can match them.
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
