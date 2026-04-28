/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Object-list panel helpers: deriving display names from scene
//! metadata (with localised fallbacks and duplicate disambiguation),
//! and pushing the resulting list to the Slint window.

use sc_mesh_core::scene::Scene;
use slint::{ModelRc, SharedString, StandardListViewItem, VecModel};

use crate::MainWindow;
use crate::i18n::ctr;

/// Display names for the object-list panel. Falls back to a localised
/// `Object N` (1-indexed) when an object has no `SceneMetadata.name`,
/// then disambiguates duplicates with a `(2)` / `(3)` suffix on every
/// repeat (the first occurrence stays bare). O(N²) on object count;
/// fine for slicer-typical scenes (tens of objects). Pure helper so it
/// can be unit-tested without a `MainWindow`.
pub(super) fn build_object_list_names(scene: &Scene) -> Vec<String> {
    let raw: Vec<String> = scene
        .objects
        .iter()
        .enumerate()
        .map(|(i, obj)| {
            obj.meta
                .as_ref()
                .and_then(|m| m.name.clone())
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
    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::primitives::Vertex;
    use sc_mesh_core::scene::{Scene, SceneMetadata, SceneObject, SceneObjectContent, Transform3D};

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

    #[test]
    fn anonymous_objects_get_one_indexed_fallback_names() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![anonymous(), anonymous(), anonymous()],
        };
        let names = build_object_list_names(&scene);
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
        let names = build_object_list_names(&scene);
        assert_eq!(names, vec!["Body", "Cap", "Pin"]);
    }

    #[test]
    fn duplicate_names_are_disambiguated_only_after_the_first() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![named("Body"), named("Body"), named("Body")],
        };
        let names = build_object_list_names(&scene);
        assert_eq!(names, vec!["Body", "Body (2)", "Body (3)"]);
    }

    #[test]
    fn unique_names_are_left_alone_even_when_others_collide() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![named("Body"), named("Body"), named("Pin")],
        };
        let names = build_object_list_names(&scene);
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
        let names = build_object_list_names(&scene);
        assert_eq!(names, vec!["Object 2", "Object 2 (2)", "Object 3"]);
    }

    #[test]
    fn empty_scene_yields_empty_name_list() {
        let scene = Scene::default();
        let names = build_object_list_names(&scene);
        assert!(names.is_empty());
    }
}
