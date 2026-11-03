/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Application-wide clipboard for the copy and paste pipeline.
//!
//! Holds a [`sc_scene_edit::SceneFragment`] captured from the source window's
//! selection, plus a counter that the paste path uses to offset consecutive
//! pastes so each copy is visually distinguishable.
//!
//! The clipboard lives on `AppState`, not on any individual main window: a copy
//! in window A must be pasteable in window B (even one that has loaded an
//! entirely different scene). The `Arc<IndexedMesh>` inside the fragment keeps
//! the source mesh alive on its own, so the source scene can be closed and the
//! clipboard stays valid. A prepared model's hollow and supports go with it
//! ([`prep`], #113).

pub(crate) mod placement;
pub(crate) mod prep;

use sc_mesh_core::Aabb;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_scene::node::{LeafPayload, NodeKind};
use sc_scene::{NodeId, SceneError};
use sc_scene_edit::SceneFragment;

use crate::scene_document::SceneDocument;
use prep::Prep;

/// Clipboard payload and cascade state. The fragment comes from
/// [`fragment_from_document_selection`], `AppState` wraps it through
/// [`Clipboard::new`], and the paste path consumes it.
pub(crate) struct Clipboard {
    fragment: SceneFragment,
    /// The copied model's hollow and supports (#113).
    prep: Prep,
    /// [`fragment_world_aabb`] of `fragment`, taken once at copy time: the
    /// fragment never changes, and a walk over the vertices of a heavy mesh
    /// on every paste is time a repeated `Ctrl+V` cannot spare (#353).
    world_aabb: Option<Aabb>,
    /// Number of consecutive pastes since the fragment was captured. Bumped by
    /// the paste path; reset to 0 on a fresh copy. The placement function reads
    /// it to space out repeated pastes so stacked copies do not sit on top of
    /// each other.
    cascade_count: u32,
}

impl Clipboard {
    /// A clipboard for `fragment` alone, a plain mesh: for tests.
    #[cfg(test)]
    pub(crate) fn new(fragment: SceneFragment) -> Self {
        Self::with_prep(fragment, Prep::default())
    }

    /// A clipboard for `fragment` that pastes with `prep`.
    pub(crate) fn with_prep(fragment: SceneFragment, prep: Prep) -> Self {
        Self {
            world_aabb: prep.reach().or_else(|| fragment_world_aabb(&fragment)),
            fragment,
            prep,
            cascade_count: 0,
        }
    }

    pub(crate) fn fragment(&self) -> &SceneFragment {
        &self.fragment
    }

    /// The fragment's [`fragment_world_aabb`], or with the supports that go
    /// along, the box they and the model take.
    pub(crate) fn world_aabb(&self) -> Option<Aabb> {
        self.world_aabb
    }

    pub(crate) fn cascade_count(&self) -> u32 {
        self.cascade_count
    }

    /// Increments the cascade counter; saturates at `u32::MAX`. Consecutive
    /// paste calls bump this; copy replaces the clipboard wholesale, which
    /// resets cascade to 0 through [`Self::new`].
    pub(crate) fn bump_cascade(&mut self) {
        self.cascade_count = self.cascade_count.saturating_add(1);
    }
}

/// A clipboard of `doc`'s selected model, with its hollow and supports
/// (#113); `None` as [`fragment_from_document_selection`] has it.
pub(crate) fn clipboard_from_document_selection(doc: &SceneDocument) -> Option<Clipboard> {
    let fragment = fragment_from_document_selection(doc)?;
    let model = doc.selection_primary_node_id()?;
    Some(Clipboard::with_prep(fragment, Prep::of(doc, model)))
}

/// Pastes `clipboard` into `doc`'s scene, centred near `target` (world mm,
/// on the plate's floor at `floor_z`) and clear of what stands there: on the
/// plate, or with supports at the copied model's height, which they still
/// hold. One undo entry, the copy selected. Returns the copy's leaf; `None`
/// when no scene is loaded or the clipboard holds nothing to paste.
pub(crate) fn paste(
    doc: &SceneDocument,
    clipboard: &Clipboard,
    target: [f64; 2],
    floor_z: f64,
) -> Result<Option<NodeId>, SceneError> {
    let Some(fragment_aabb) = clipboard.world_aabb() else {
        return Ok(None);
    };
    let Some(obstacles) = doc.with_scene(placement::obstacle_aabbs_for_scene) else {
        return Ok(None);
    };
    let placed = placement::paste_translation_for_fragment(
        &fragment_aabb,
        target,
        floor_z,
        clipboard.cascade_count(),
        &obstacles,
    );
    let prep = &clipboard.prep;
    if prep.is_empty() {
        return doc.paste_fragment(clipboard.fragment(), placed);
    }
    prep.paste(doc, clipboard.fragment(), prep.translation(placed))
}

/// Builds a single-leaf [`SceneFragment`] from `doc`'s current selection, or
/// `None` when no scene is loaded, nothing is selected, or the selected node is
/// not a mesh-backed leaf.
///
/// The fragment carries the source leaf's world-space rotation and scale (the
/// linear part of its composed world transform); the translation component is
/// dropped because the paste primitive sets the new coordinates from the
/// placement search. The mesh `Arc` is preserved by identity, so a later
/// same-scene paste can dedupe via
/// [`sc_scene::Scene::find_mesh_resource_by_arc`] instead of pushing a fresh
/// `AddMeshResource` edit.
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

/// Returns the world-space AABB of the fragment's single root leaf, after
/// applying the root's local transform (rotation and scale) to the mesh's local
/// AABB via Arvo's method. Returns `None` when the fragment is empty, the root
/// is not a mesh-backed leaf, or the mesh has no vertices.
///
/// "World-space" here means fragment-space: the AABB the placement function
/// feeds into the cursor-centring math, before any paste-side translation is
/// added. Arvo's method over-reports for tilted meshes (the conservative
/// AABB-of-the-rotated-local-AABB); the paste pipeline accepts this since the
/// collision search is already a heuristic.
pub(crate) fn fragment_world_aabb(fragment: &SceneFragment) -> Option<Aabb> {
    let scene = fragment.scene();
    let root_id = scene.roots().first().copied()?;
    let node = scene.node(root_id)?;
    let mesh_arc = match &node.kind {
        NodeKind::Leaf(LeafPayload::Mesh(arc)) => arc.clone(),
        NodeKind::Leaf(LeafPayload::MeshResource(rid)) => scene.mesh_resource_arc(*rid)?.clone(),
        NodeKind::Group { .. } | NodeKind::Csg { .. } | NodeKind::Reference { .. } => return None,
    };
    let local_aabb = mesh_arc.local_aabb()?;
    Some(match node.local_transform {
        Transform3D::Identity => local_aabb,
        Transform3D::Affine(a) => local_aabb.transformed_arvo(&a),
    })
}

/// Returns `xform` with its translation component set to the origin, preserving
/// the linear part. The paste primitive re-anchors the fragment root at a
/// placement-chosen translation, so the translation captured at copy time would
/// be overwritten anyway; dropping it here keeps the clipboard's invariant
/// clean (the fragment's root sits at the origin in fragment-space) and avoids
/// surprising callers who inspect the fragment for rotation and scale.
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

    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
    use sc_scene::Scene;
    use sc_scene::node::LeafPayload;

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
            faces: vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
        })
    }

    fn fixture_path() -> PathBuf {
        PathBuf::from("/tmp/clipboard_fixture.obj")
    }

    /// Builds a scene with 1 inline leaf under the loader-shape root and
    /// returns the (scene, leaf-id) pair so tests can select the leaf after
    /// install.
    fn scene_with_inline_leaf(mesh: Arc<IndexedMesh>, leaf_local: Transform3D) -> (Scene, NodeId) {
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
        // Selection::One is typed to a LeafId, but the underlying NodeId could
        // in principle point at a group if a caller forged one. The helper
        // falls back to None rather than panicking: the leaf_mesh_arc accessor
        // reports the kind mismatch.
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
    fn clipboard_keeps_the_fragment_box_taken_at_copy_time() {
        let fragment = SceneFragment::from_leaf_snapshot(triangle_arc(), Transform3D::Identity);
        let expected = fragment_world_aabb(&fragment);
        let cb = Clipboard::new(fragment);
        assert_eq!(cb.world_aabb(), expected);
        assert!(cb.world_aabb().is_some());
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

    #[test]
    fn fragment_world_aabb_with_identity_returns_local_aabb() {
        let mesh = triangle_arc();
        let fragment = SceneFragment::from_leaf_snapshot(Arc::clone(&mesh), Transform3D::Identity);
        let aabb = fragment_world_aabb(&fragment).expect("triangle has an AABB");
        let local = mesh.local_aabb().unwrap();
        assert_eq!(aabb.min().coords(), local.min().coords());
        assert_eq!(aabb.max().coords(), local.max().coords());
    }

    #[test]
    fn fragment_world_aabb_applies_root_local_transform() {
        // 90 degree rotation around +Z: the triangle's footprint changes from x
        // in [0, 1] to y in [0, 1] (and x in [-1, 0]).
        let mesh = triangle_arc();
        let rot = Transform3D::Affine(AffineTransform3D::from_rotation_z(
            std::f64::consts::FRAC_PI_2,
        ));
        let fragment = SceneFragment::from_leaf_snapshot(mesh, rot);
        let aabb = fragment_world_aabb(&fragment).expect("rotated triangle has an AABB");
        // After rotating, the original local AABB (x in [0, 1], y in [0, 1])
        // becomes (x in [-1, 0], y in [0, 1]). For a 90 degree rotation, Arvo's
        // |M[i][j]| rule is exact up to rounding, so each bound matches within
        // 1e-5.
        let min = aabb.min().coords();
        let max = aabb.max().coords();
        assert!(min[0] <= -1.0 + 1e-5 && min[0] >= -1.0 - 1e-5);
        assert!(max[0] <= 0.0 + 1e-5 && max[0] >= 0.0 - 1e-5);
        assert!(min[1] <= 0.0 + 1e-5 && min[1] >= 0.0 - 1e-5);
        assert!(max[1] <= 1.0 + 1e-5 && max[1] >= 1.0 - 1e-5);
    }

    #[test]
    fn fragment_world_aabb_returns_none_for_empty_mesh() {
        let empty = Arc::new(IndexedMesh::new(None, Vec::new(), Vec::new()));
        let fragment = SceneFragment::from_leaf_snapshot(empty, Transform3D::Identity);
        assert!(fragment_world_aabb(&fragment).is_none());
    }
}
