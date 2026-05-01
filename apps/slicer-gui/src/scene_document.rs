/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Document-level model for the loaded scene.
//!
//! Owns the scene and (eventually) its source path, the selection, and
//! the undo/redo command stack. Consumers like the viewport controller
//! and the object-list panel observe the document instead of holding
//! their own copies of this state.

use std::cell::RefCell;
use std::rc::Rc;

use sc_mesh_core::scene::Scene;

pub(crate) struct SceneDocument {
    scene: RefCell<Option<Scene>>,
}

impl SceneDocument {
    pub fn new() -> Rc<Self> {
        Rc::new(Self {
            scene: RefCell::new(None),
        })
    }

    pub fn install_scene(&self, scene: Scene) {
        *self.scene.borrow_mut() = Some(scene);
    }

    pub fn clear_scene(&self) {
        *self.scene.borrow_mut() = None;
    }

    /// Runs `f` against the loaded scene, if any. Returns `None` when
    /// no scene is installed. Callers must keep the closure short — the
    /// internal borrow is held for its duration.
    // Tests exercise this method; production callers land in the next
    // checkpoint, which migrates the controller's scene reads onto the
    // document. The allow lifts then.
    #[allow(dead_code)]
    pub fn with_scene<R>(&self, f: impl FnOnce(&Scene) -> R) -> Option<R> {
        self.scene.borrow().as_ref().map(f)
    }

    pub fn is_loaded(&self) -> bool {
        self.scene.borrow().is_some()
    }
}

#[cfg(test)]
mod scene_document_tests {
    use super::*;

    fn empty_scene() -> Scene {
        Scene::new(None, Vec::new(), Vec::new())
    }

    #[test]
    fn new_constructs_a_document() {
        let _doc: Rc<SceneDocument> = SceneDocument::new();
    }

    #[test]
    fn fresh_document_has_no_scene() {
        let doc = SceneDocument::new();
        assert!(!doc.is_loaded());
        assert!(doc.with_scene(|_| ()).is_none());
    }

    #[test]
    fn install_scene_makes_it_observable() {
        let doc = SceneDocument::new();
        doc.install_scene(empty_scene());
        assert!(doc.is_loaded());
        let object_count = doc.with_scene(|s| s.objects().len());
        assert_eq!(object_count, Some(0));
    }

    #[test]
    fn clear_scene_removes_it() {
        let doc = SceneDocument::new();
        doc.install_scene(empty_scene());
        doc.clear_scene();
        assert!(!doc.is_loaded());
        assert!(doc.with_scene(|_| ()).is_none());
    }

    #[test]
    fn install_scene_replaces_a_prior_scene() {
        let doc = SceneDocument::new();
        doc.install_scene(empty_scene());
        doc.install_scene(empty_scene());
        assert!(doc.is_loaded());
    }
}
