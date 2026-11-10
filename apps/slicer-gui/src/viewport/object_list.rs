/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Object-list panel helpers: deriving display names from scene
//! metadata (with localised fallbacks and duplicate disambiguation),
//! and pushing the resulting list to the Slint window.

use sc_scene::node::{LeafPayload, NodeKind};
use sc_scene::{CsgPolicy, ReferencePolicy, Scene};
use slint::{ModelRc, SharedString, StandardListViewItem, VecModel};

use crate::MainWindow;
use crate::i18n::ctr;

/// Display names for the object-list panel, one entry per reachable
/// leaf in pre-order traversal of `graph.roots()`. Prefers the
/// leaf node's `meta.name`; falls back to the resolved mesh's
/// `MeshMetadata.name` (where format readers like OBJ/STL store the
/// on-disk name); finally falls back to a localised `Object N`
/// (1-indexed by pre-order leaf rank, matching the legacy flat-scene
/// numbering). Duplicates get a `(2)` / `(3)` suffix on every repeat
/// (the first occurrence stays bare). O(N²) on object count; fine
/// for slicer-typical scenes (tens of objects). Pure helper so it can
/// be unit-tested without a `MainWindow`.
///
/// The traversal order matches `SceneSpatial::leaf_id_at_index`: row
/// `i` in the returned list maps to the leaf at pre-order index `i`.
pub(crate) fn build_object_list_names(graph: &Scene) -> Vec<String> {
    let raw: Vec<String> = graph
        .reachable_leaves(CsgPolicy::Descend, ReferencePolicy::Skip)
        .expect("a (Descend, Skip) structural walk has no error source")
        .into_iter()
        .enumerate()
        .map(|(i, leaf_id)| {
            let node = graph
                .node(leaf_id)
                .expect("reachable_leaves yields only live node ids");
            let scene_name = node.meta.as_ref().and_then(|m| m.name.clone());
            let NodeKind::Leaf(payload) = &node.kind else {
                unreachable!("reachable_leaves yields only leaf nodes")
            };
            let mesh_name = match payload {
                LeafPayload::Mesh(mesh) => mesh.meta().as_ref().and_then(|m| m.name.clone()),
                LeafPayload::MeshResource(rid) => graph
                    .mesh_resource(*rid)
                    .and_then(|mesh| mesh.meta().as_ref())
                    .and_then(|m| m.name.clone()),
            }
            .map(|name| {
                // The supports a pre-supported file came with (#258) carry
                // their role as their name: it shows in the language in force.
                if sc_geom::df_supports::is_imported_supports_name(&name) {
                    ctr!("ObjectList", "Imported supports").to_string()
                } else {
                    name
                }
            });
            scene_name
                .or(mesh_name)
                .unwrap_or_else(|| ctr!("ObjectList", "Object {}", i + 1).to_string())
        })
        .collect();

    raw.iter()
        .enumerate()
        .map(|(i, name)| {
            let total = raw.iter().filter(|n| n.as_str() == name.as_str()).count();
            if total < 2 {
                name.clone()
            } else {
                let nth = raw
                    .iter()
                    .take(i + 1)
                    .filter(|n| n.as_str() == name.as_str())
                    .count();
                if nth == 1 {
                    name.clone()
                } else {
                    format!("{name} ({nth})")
                }
            }
        })
        .collect()
}

/// Pushes a fresh `[StandardListViewItem]` model to the window. A
/// fresh `VecModel` per call is the simplest way to keep the UI in
/// sync: the list rebuilds rarely (only on scene load/close), so the
/// extra alloc is irrelevant next to mesh I/O.
pub(super) fn push_object_list(ui: &MainWindow, names: Vec<String>) {
    let items: Vec<StandardListViewItem> = names
        .into_iter()
        .map(|n| StandardListViewItem::from(SharedString::from(n)))
        .collect();
    ui.set_object_list(ModelRc::new(VecModel::from(items)));
}

#[cfg(test)]
mod build_object_list_names_tests {
    use std::sync::Arc;

    use super::*;
    use sc_mesh_core::primitives::Vertex;
    use sc_mesh_core::transform::Transform3D;
    use sc_mesh_core::{IndexedMesh, MeshMetadata};
    use sc_mesh_spatial::SceneSpatial;
    use sc_scene::{CsgOp, NodeId, SceneMetadata};

    /// Loader-faithful empty graph: identity-Group root, no leaves yet.
    fn graph_with_group_root() -> (Scene, NodeId) {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group root");
        (g, root)
    }

    fn empty_mesh() -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new([0.0, 0.0, 0.0])],
            faces: vec![],
        }
    }

    fn mesh_with_name(name: &str) -> IndexedMesh {
        IndexedMesh {
            meta: Some(MeshMetadata {
                name: Some(name.to_string()),
                ..Default::default()
            }),
            vertices: vec![Vertex::new([0.0, 0.0, 0.0])],
            faces: vec![],
        }
    }

    fn scene_meta(name: &str) -> SceneMetadata {
        SceneMetadata {
            name: Some(name.to_string()),
            comment: None,
        }
    }

    /// Attach an inline-mesh leaf under `parent` with the given node
    /// meta and mesh meta (either may be `None`).
    fn add_leaf_under(
        g: &mut Scene,
        parent: NodeId,
        node_meta: Option<SceneMetadata>,
        mesh: IndexedMesh,
    ) -> NodeId {
        let leaf = g
            .add_leaf(
                node_meta,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .expect("add leaf");
        g.add_child(parent, leaf).expect("attach leaf");
        leaf
    }

    fn add_named(g: &mut Scene, parent: NodeId, name: &str) -> NodeId {
        add_leaf_under(g, parent, Some(scene_meta(name)), empty_mesh())
    }

    fn add_anonymous(g: &mut Scene, parent: NodeId) -> NodeId {
        add_leaf_under(g, parent, None, empty_mesh())
    }

    fn add_mesh_named(g: &mut Scene, parent: NodeId, name: &str) -> NodeId {
        add_leaf_under(g, parent, None, mesh_with_name(name))
    }

    fn add_named_detached(g: &mut Scene, name: &str) -> NodeId {
        g.add_leaf(
            Some(scene_meta(name)),
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(empty_mesh())),
        )
        .expect("add detached leaf")
    }

    #[test]
    fn anonymous_objects_get_one_indexed_fallback_names() {
        let (mut g, root) = graph_with_group_root();
        add_anonymous(&mut g, root);
        add_anonymous(&mut g, root);
        add_anonymous(&mut g, root);
        let names = build_object_list_names(&g);
        // Translation pipeline returns "Object {N}" verbatim when no .po
        // catalogue is loaded (the test runtime), so we can compare
        // strings directly.
        assert_eq!(names, vec!["Object 1", "Object 2", "Object 3"]);
    }

    #[test]
    fn named_objects_keep_their_names() {
        let (mut g, root) = graph_with_group_root();
        add_named(&mut g, root, "Body");
        add_named(&mut g, root, "Cap");
        add_named(&mut g, root, "Pin");
        let names = build_object_list_names(&g);
        assert_eq!(names, vec!["Body", "Cap", "Pin"]);
    }

    #[test]
    fn duplicate_names_are_disambiguated_only_after_the_first() {
        let (mut g, root) = graph_with_group_root();
        add_named(&mut g, root, "Body");
        add_named(&mut g, root, "Body");
        add_named(&mut g, root, "Body");
        let names = build_object_list_names(&g);
        assert_eq!(names, vec!["Body", "Body (2)", "Body (3)"]);
    }

    #[test]
    fn unique_names_are_left_alone_even_when_others_collide() {
        let (mut g, root) = graph_with_group_root();
        add_named(&mut g, root, "Body");
        add_named(&mut g, root, "Body");
        add_named(&mut g, root, "Pin");
        let names = build_object_list_names(&g);
        assert_eq!(names, vec!["Body", "Body (2)", "Pin"]);
    }

    #[test]
    fn fallback_names_can_collide_with_explicit_names() {
        // A metadata-named "Object 2" at index 0 collides with the
        // anonymous fallback for the object at index 1 (whose 1-indexed
        // position is also 2). The dedup pass treats them uniformly:
        // both are "Object 2", so the second occurrence gets a "(2)"
        // suffix; the third anonymous, at index 2, falls back to the
        // unique "Object 3".
        let (mut g, root) = graph_with_group_root();
        add_named(&mut g, root, "Object 2");
        add_anonymous(&mut g, root);
        add_anonymous(&mut g, root);
        let names = build_object_list_names(&g);
        assert_eq!(names, vec!["Object 2", "Object 2 (2)", "Object 3"]);
    }

    #[test]
    fn empty_scene_yields_empty_name_list() {
        // Empty graph (no Group root, no leaves) matches the
        // "no scene loaded" case.
        let g = Scene::new();
        let names = build_object_list_names(&g);
        assert!(names.is_empty());
    }

    #[test]
    fn mesh_meta_name_is_used_when_scene_object_meta_is_absent() {
        // OBJ/STL readers store the on-disk object name on
        // `IndexedMesh.meta.name` and leave the leaf's node meta as
        // None; the panel must surface the mesh name rather than the
        // "Object N" fallback.
        let (mut g, root) = graph_with_group_root();
        add_mesh_named(&mut g, root, "Icosahedron");
        add_mesh_named(&mut g, root, "Dodecahedron");
        let names = build_object_list_names(&g);
        assert_eq!(names, vec!["Icosahedron", "Dodecahedron"]);
    }

    #[test]
    fn row_order_matches_scene_spatial_rows() {
        // Row `i` in the sidebar list must name the leaf that
        // `SceneSpatial::leaf_id_at_index(i)` resolves. The fixture
        // includes a nested group and CSG operand leaves so the
        // equality is pinned beyond the flat case.
        let (mut g, root) = graph_with_group_root();
        add_named(&mut g, root, "Before");
        let sub = g.add_group(None, Transform3D::Identity);
        g.add_child(root, sub).expect("attach subgroup");
        add_named(&mut g, sub, "Nested");
        let op1 = add_named_detached(&mut g, "Op1");
        let op2 = add_named_detached(&mut g, "Op2");
        let csg = g
            .add_csg(None, Transform3D::Identity, CsgOp::Union, vec![op1, op2])
            .expect("csg union");
        g.add_child(root, csg).expect("attach csg");
        add_named(&mut g, root, "After");

        let names = build_object_list_names(&g);
        assert_eq!(names, vec!["Before", "Nested", "Op1", "Op2", "After"]);

        let spatial = SceneSpatial::from_graph(g);
        for (i, name) in names.iter().enumerate() {
            let leaf_id = spatial
                .leaf_id_at_index(i)
                .expect("every list row resolves to a leaf");
            let node = spatial.graph().node(leaf_id).expect("live leaf");
            assert_eq!(
                node.meta.as_ref().unwrap().name.as_deref(),
                Some(name.as_str())
            );
        }
        assert!(spatial.leaf_id_at_index(names.len()).is_none());
    }

    #[test]
    fn scene_object_meta_name_takes_priority_over_mesh_meta_name() {
        // If a future UI ever sets the leaf's node meta name to
        // override the on-disk mesh name, that override must win.
        let (mut g, root) = graph_with_group_root();
        add_leaf_under(
            &mut g,
            root,
            Some(scene_meta("UserRenamed")),
            mesh_with_name("OnDiskName"),
        );
        let names = build_object_list_names(&g);
        assert_eq!(names, vec!["UserRenamed"]);
    }
}
