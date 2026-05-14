/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Object-list panel helpers: deriving display names from scene
//! metadata (with localised fallbacks and duplicate disambiguation),
//! and pushing the resulting list to the Slint window.

use sc_mesh_core::NodeId;
use sc_mesh_core::scene_graph::SceneGraph;
use sc_mesh_core::scene_graph_node::{LeafPayload, NodeKind};
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
pub(super) fn build_object_list_names(graph: &SceneGraph) -> Vec<String> {
    let raw: Vec<String> = pre_order_leaves(graph)
        .into_iter()
        .enumerate()
        .map(|(i, leaf_id)| {
            let node = graph
                .node(leaf_id)
                .expect("pre_order_leaves only yields reachable leaves");
            let scene_name = node.meta.as_ref().and_then(|m| m.name.clone());
            let NodeKind::Leaf(payload) = &node.kind else {
                unreachable!("pre_order_leaves filters to NodeKind::Leaf")
            };
            let mesh_name = match payload {
                LeafPayload::Mesh(mesh) => mesh.meta().as_ref().and_then(|m| m.name.clone()),
                LeafPayload::MeshResource(rid) => graph
                    .mesh_resource(*rid)
                    .and_then(|mesh| mesh.meta().as_ref())
                    .and_then(|m| m.name.clone()),
            };
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

/// Pre-order list of `NodeId`s for every reachable leaf reachable from
/// `graph.roots()`, in the same traversal order as
/// `SceneSpatial::leaf_id_at_index`. Groups and CSG nodes recurse;
/// `Instance` nodes are skipped (the current phase does not produce
/// them, and expansion semantics get their own resolver later).
fn pre_order_leaves(graph: &SceneGraph) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack: Vec<NodeId> = graph.roots().iter().rev().copied().collect();
    while let Some(id) = stack.pop() {
        let Some(node) = graph.node(id) else {
            continue;
        };
        match &node.kind {
            NodeKind::Leaf(_) => out.push(id),
            NodeKind::Group { children } | NodeKind::Csg { children, .. } => {
                for child in children.iter().rev() {
                    stack.push(*child);
                }
            }
            NodeKind::Instance { .. } => {}
        }
    }
    out
}

/// Pushes a fresh `[StandardListViewItem]` model to the window. A
/// fresh `VecModel` per call is the simplest way to keep the UI in
/// sync — the list rebuilds rarely (only on scene load/close), so the
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
    use super::*;
    use sc_mesh_core::primitives::Vertex;
    use sc_mesh_core::scene::{Scene, SceneMetadata, SceneObject, SceneObjectContent, Transform3D};
    use sc_mesh_core::{IndexedMesh, MeshMetadata};

    fn graph_from(scene: Scene) -> SceneGraph {
        SceneGraph::try_from(scene).expect("test scene has no dangling resources")
    }

    fn empty_mesh() -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new([0.0, 0.0, 0.0])],
            faces: vec![],
        }
    }

    fn named(name: &str) -> SceneObject {
        SceneObject {
            meta: Some(SceneMetadata {
                name: Some(name.to_string()),
                comment: None,
            }),
            transform: Transform3D::Identity,
            content: SceneObjectContent::Mesh(empty_mesh()),
        }
    }

    fn anonymous() -> SceneObject {
        SceneObject {
            meta: None,
            transform: Transform3D::Identity,
            content: SceneObjectContent::Mesh(empty_mesh()),
        }
    }

    fn mesh_named(name: &str) -> SceneObject {
        SceneObject {
            meta: None,
            transform: Transform3D::Identity,
            content: SceneObjectContent::Mesh(IndexedMesh {
                meta: Some(MeshMetadata {
                    name: Some(name.to_string()),
                    ..Default::default()
                }),
                vertices: vec![Vertex::new([0.0, 0.0, 0.0])],
                faces: vec![],
            }),
        }
    }

    #[test]
    fn anonymous_objects_get_one_indexed_fallback_names() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![anonymous(), anonymous(), anonymous()],
        };
        let names = build_object_list_names(&graph_from(scene));
        // Translation pipeline returns "Object {N}" verbatim when no .po
        // catalogue is loaded (the test runtime), so we can compare
        // strings directly.
        assert_eq!(names, vec!["Object 1", "Object 2", "Object 3"]);
    }

    #[test]
    fn named_objects_keep_their_names() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![named("Body"), named("Cap"), named("Pin")],
        };
        let names = build_object_list_names(&graph_from(scene));
        assert_eq!(names, vec!["Body", "Cap", "Pin"]);
    }

    #[test]
    fn duplicate_names_are_disambiguated_only_after_the_first() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![named("Body"), named("Body"), named("Body")],
        };
        let names = build_object_list_names(&graph_from(scene));
        assert_eq!(names, vec!["Body", "Body (2)", "Body (3)"]);
    }

    #[test]
    fn unique_names_are_left_alone_even_when_others_collide() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![named("Body"), named("Body"), named("Pin")],
        };
        let names = build_object_list_names(&graph_from(scene));
        assert_eq!(names, vec!["Body", "Body (2)", "Pin"]);
    }

    #[test]
    fn fallback_names_can_collide_with_explicit_names() {
        // A metadata-named "Object 2" at index 0 collides with the
        // anonymous fallback for the object at index 1 (whose 1-indexed
        // position is also 2). The dedup pass treats them uniformly —
        // both are "Object 2", so the second instance gets a "(2)"
        // suffix; the third anonymous, at index 2, falls back to the
        // unique "Object 3".
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![named("Object 2"), anonymous(), anonymous()],
        };
        let names = build_object_list_names(&graph_from(scene));
        assert_eq!(names, vec!["Object 2", "Object 2 (2)", "Object 3"]);
    }

    #[test]
    fn empty_scene_yields_empty_name_list() {
        let scene = Scene::default();
        let names = build_object_list_names(&graph_from(scene));
        assert!(names.is_empty());
    }

    #[test]
    fn mesh_meta_name_is_used_when_scene_object_meta_is_absent() {
        // OBJ/STL readers store the on-disk object name on
        // `IndexedMesh.meta.name` and leave `SceneObject.meta` as None;
        // the panel must surface the mesh name rather than the
        // "Object N" fallback.
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![mesh_named("Icosahedron"), mesh_named("Dodecahedron")],
        };
        let names = build_object_list_names(&graph_from(scene));
        assert_eq!(names, vec!["Icosahedron", "Dodecahedron"]);
    }

    #[test]
    fn row_order_matches_pre_order_leaf_traversal() {
        // The traversal order this helper uses must agree with
        // `SceneSpatial::leaf_id_at_index`, so row `i` in the
        // sidebar list selects the same leaf as the controller's
        // `on_select_by_index(i)` resolves. The check is positional:
        // the names appear in the same order as the underlying
        // pre-order leaf walk hands them out.
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![named("Body"), named("Cap"), named("Pin")],
        };
        let graph = graph_from(scene);
        let names = build_object_list_names(&graph);
        let leaves = pre_order_leaves(&graph);
        assert_eq!(names.len(), leaves.len());
        for (i, leaf_id) in leaves.iter().enumerate() {
            let node = graph.node(*leaf_id).unwrap();
            let expected = node.meta.as_ref().unwrap().name.as_deref().unwrap();
            assert_eq!(names[i], expected);
        }
    }

    #[test]
    fn scene_object_meta_name_takes_priority_over_mesh_meta_name() {
        // If a future UI ever sets `SceneObject.meta.name` to override
        // the on-disk mesh name, that override must win.
        let mut obj = mesh_named("OnDiskName");
        obj.meta = Some(SceneMetadata {
            name: Some("UserRenamed".to_string()),
            comment: None,
        });
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![obj],
        };
        let names = build_object_list_names(&graph_from(scene));
        assert_eq!(names, vec!["UserRenamed"]);
    }
}
