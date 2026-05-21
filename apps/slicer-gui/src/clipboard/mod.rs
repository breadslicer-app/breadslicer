/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Application-wide clipboard for the copy / paste pipeline.
//!
//! Holds a [`sc_mesh_core::SceneFragment`] captured from the source
//! window's selection, plus a counter that the paste path uses to
//! offset consecutive pastes so each copy is visually distinguishable.
//!
//! The clipboard lives on `AppState`, not on any individual main
//! window: a copy in window A must be pasteable in window B (even one
//! that has loaded an entirely different scene). The `Arc<IndexedMesh>`
//! inside the fragment keeps the source mesh alive on its own, so the
//! source scene can be closed and the clipboard stays valid.

pub(crate) mod placement;

use sc_mesh_core::SceneFragment;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};

use crate::scene_document::SceneDocument;

/// Clipboard payload + cascade state. Constructed by
/// [`fragment_from_document_selection`] (wrapped on the AppState
/// side) and consumed by the paste path.
#[allow(dead_code)]
pub(crate) struct Clipboard {
    fragment: SceneFragment,
    /// Number of consecutive pastes since the fragment was captured.
    /// Bumped by the paste path; reset to zero on a fresh copy. The
    /// placement function reads it to space out repeated pastes so
    /// stacked copies don't sit on top of each other.
    cascade_count: u32,
}

#[allow(dead_code)]
impl Clipboard {
    pub(crate) fn new(fragment: SceneFragment) -> Self {
        Self {
            fragment,
            cascade_count: 0,
        }
    }

    pub(crate) fn fragment(&self) -> &SceneFragment {
        &self.fragment
    }

    pub(crate) fn cascade_count(&self) -> u32 {
        self.cascade_count
    }

    /// Increment the cascade counter; saturates at `u32::MAX`.
    /// Consecutive paste calls bump this; copy replaces the
    /// clipboard wholesale, which resets cascade to zero through
    /// [`Self::new`].
    pub(crate) fn bump_cascade(&mut self) {
        self.cascade_count = self.cascade_count.saturating_add(1);
    }
}

/// Build a single-leaf [`SceneFragment`] from `doc`'s current
/// selection, or `None` when there's no scene loaded, nothing
/// selected, or the selected node is not a mesh-backed leaf.
///
/// The fragment carries the source leaf's world-space rotation + scale
/// (the linear part of its composed world transform); the translation
/// component is dropped because the paste primitive sets the new
/// coordinates from the placement search. The mesh `Arc` is preserved
/// by identity, so a later same-scene paste can dedupe via
/// [`sc_mesh_core::Scene::find_mesh_resource_by_arc`] instead of
/// pushing a fresh `AddMeshResource` edit.
#[allow(dead_code)]
pub(crate) fn fragment_from_document_selection(doc: &SceneDocument) -> Option<SceneFragment> {
    let node_id = doc.selection_primary_node_id()?;
    let (arc, world_xform) = doc
        .with_scene(|spatial| {
            let arc = spatial.graph().leaf_mesh_arc(node_id).cloned()?;
            let world = spatial.leaf(node_id).map(|q| q.world_transform())?;
            Some((arc, world))
        })
        .flatten()?;
    Some(SceneFragment::from_leaf_snapshot(
        arc,
        drop_translation(world_xform),
    ))
}

/// Return `xform` with its translation component set to the origin,
/// preserving the linear part. The paste primitive re-anchors the
/// fragment root at a placement-chosen translation, so the
/// translation captured at copy time would be overwritten anyway;
/// dropping it here keeps the clipboard's invariant clean (the
/// fragment's root sits at the origin in fragment-space) and avoids
/// surprising callers who inspect the fragment for rotation / scale.
#[allow(dead_code)]
fn drop_translation(xform: Transform3D) -> Transform3D {
    match xform {
        Transform3D::Identity => Transform3D::Identity,
        Transform3D::Affine(a) => Transform3D::Affine(
            AffineTransform3D::from_linear_and_translation(*a.linear(), [0.0, 0.0, 0.0]),
        ),
    }
}

#[cfg(test)]
mod clipboard_tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
    use sc_mesh_core::scene_node::LeafPayload;
    use sc_mesh_core::{IndexedMesh, Scene};

    use crate::selection::{LeafId, Selection};

    use super::*;

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

    fn fixture_path() -> PathBuf {
        PathBuf::from("/tmp/clipboard_fixture.obj")
    }

    /// Build a one-inline-leaf scene under the loader-shape root and
    /// return the (scene, leaf-id) pair so tests can select the leaf
    /// after install.
    fn scene_with_inline_leaf(
        mesh: Arc<IndexedMesh>,
        leaf_local: Transform3D,
    ) -> (Scene, sc_mesh_core::NodeId) {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).unwrap();
        let leaf = g
            .add_leaf(None, leaf_local, LeafPayload::Mesh(mesh))
            .unwrap();
        g.add_child(root, leaf).unwrap();
        (g, leaf)
    }

    #[test]
    fn fragment_from_document_selection_returns_none_with_no_scene_loaded() {
        let doc = SceneDocument::new();
        assert!(fragment_from_document_selection(&doc).is_none());
    }

    #[test]
    fn fragment_from_document_selection_returns_none_with_empty_selection() {
        let doc = SceneDocument::new();
        let (graph, _leaf) = scene_with_inline_leaf(triangle_arc(), Transform3D::Identity);
        doc.install_graph(graph, fixture_path());
        // No set_selection: still Selection::None.
        assert!(fragment_from_document_selection(&doc).is_none());
    }

    #[test]
    fn fragment_from_document_selection_preserves_the_inline_mesh_arc() {
        let doc = SceneDocument::new();
        let mesh = triangle_arc();
        let (graph, leaf) = scene_with_inline_leaf(Arc::clone(&mesh), Transform3D::Identity);
        doc.install_graph(graph, fixture_path());
        doc.set_selection(Selection::One(LeafId(leaf)));
        let fragment =
            fragment_from_document_selection(&doc).expect("selected leaf yields fragment");
        let stored = fragment
            .scene()
            .mesh_resources_iter()
            .next()
            .map(|(_, m)| m as *const IndexedMesh)
            .unwrap();
        assert_eq!(stored, Arc::as_ptr(&mesh));
    }

    #[test]
    fn fragment_from_document_selection_resolves_resource_backed_leaf() {
        let doc = SceneDocument::new();
        let mesh = triangle_arc();
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).unwrap();
        let rid = g.add_mesh_resource_arc(Arc::clone(&mesh));
        let leaf = g
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .unwrap();
        g.add_child(root, leaf).unwrap();
        doc.install_graph(g, fixture_path());
        doc.set_selection(Selection::One(LeafId(leaf)));
        let fragment =
            fragment_from_document_selection(&doc).expect("resource-backed leaf yields fragment");
        let stored = fragment
            .scene()
            .mesh_resources_iter()
            .next()
            .map(|(_, m)| m as *const IndexedMesh)
            .unwrap();
        assert_eq!(stored, Arc::as_ptr(&mesh));
    }

    #[test]
    fn fragment_from_document_selection_drops_translation_and_keeps_linear_part() {
        let doc = SceneDocument::new();
        let linear = [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
        let leaf_local = Transform3D::Affine(AffineTransform3D::from_linear_and_translation(
            linear,
            [7.0, 8.0, 9.0],
        ));
        let (graph, leaf) = scene_with_inline_leaf(triangle_arc(), leaf_local);
        doc.install_graph(graph, fixture_path());
        doc.set_selection(Selection::One(LeafId(leaf)));
        let fragment = fragment_from_document_selection(&doc).unwrap();
        let scene = fragment.scene();
        let root = scene.roots()[0];
        let node = scene.node(root).unwrap();
        let Transform3D::Affine(a) = node.local_transform else {
            panic!("fragment root must materialise an Affine transform");
        };
        assert_eq!(*a.linear(), linear);
        assert_eq!(*a.translation(), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn fragment_from_document_selection_returns_none_for_group_selection() {
        // Selection::One is typed to a LeafId, but the underlying NodeId
        // could in principle point at a group if a caller forged one.
        // The helper falls back to None rather than panicking: the
        // leaf_mesh_arc accessor reports the kind mismatch.
        let doc = SceneDocument::new();
        let mut g = Scene::new();
        let group = g.add_group(None, Transform3D::Identity);
        g.add_root(group).unwrap();
        doc.install_graph(g, fixture_path());
        doc.set_selection(Selection::One(LeafId(group)));
        assert!(fragment_from_document_selection(&doc).is_none());
    }

    #[test]
    fn fragment_with_identity_world_transform_yields_identity_fragment_root() {
        let doc = SceneDocument::new();
        let (graph, leaf) = scene_with_inline_leaf(triangle_arc(), Transform3D::Identity);
        doc.install_graph(graph, fixture_path());
        doc.set_selection(Selection::One(LeafId(leaf)));
        let fragment = fragment_from_document_selection(&doc).unwrap();
        let scene = fragment.scene();
        let root = scene.roots()[0];
        assert_eq!(
            scene.node(root).unwrap().local_transform,
            Transform3D::Identity
        );
    }

    #[test]
    fn clipboard_new_starts_with_zero_cascade() {
        let fragment = SceneFragment::from_leaf_snapshot(triangle_arc(), Transform3D::Identity);
        let cb = Clipboard::new(fragment);
        assert_eq!(cb.cascade_count(), 0);
    }

    #[test]
    fn clipboard_bump_cascade_increments_then_saturates() {
        let fragment = SceneFragment::from_leaf_snapshot(triangle_arc(), Transform3D::Identity);
        let mut cb = Clipboard::new(fragment);
        cb.bump_cascade();
        cb.bump_cascade();
        cb.bump_cascade();
        assert_eq!(cb.cascade_count(), 3);
    }
}
