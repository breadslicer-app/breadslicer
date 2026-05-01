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
use std::path::{Path, PathBuf};
use std::rc::Rc;

use sc_mesh_core::scene::Scene;

pub(crate) struct SceneDocument {
    scene: RefCell<Option<Scene>>,
    /// Filesystem path the loaded scene came from. Used today to
    /// derive default export filenames; tomorrow for "Save" / "Reload".
    /// Cleared in lockstep with `scene`.
    source_path: RefCell<Option<PathBuf>>,
}

impl SceneDocument {
    pub fn new() -> Rc<Self> {
        Rc::new(Self {
            scene: RefCell::new(None),
            source_path: RefCell::new(None),
        })
    }

    pub fn install_scene(&self, scene: Scene, source_path: PathBuf) {
        *self.scene.borrow_mut() = Some(scene);
        *self.source_path.borrow_mut() = Some(source_path);
    }

    pub fn clear_scene(&self) {
        *self.scene.borrow_mut() = None;
        *self.source_path.borrow_mut() = None;
    }

    /// Path the current scene was loaded from. `None` when no scene is
    /// loaded; cloned out so the caller doesn't extend the borrow.
    // Tests exercise the field; the production caller (export-as
    // default-filename derivation) lands with the export feature.
    #[allow(dead_code)]
    pub fn source_path(&self) -> Option<PathBuf> {
        self.source_path.borrow().clone()
    }

    /// Borrow form for callers that only need the path's lifetime
    /// inside a closure (avoids the clone in `source_path`).
    #[allow(dead_code)]
    pub fn with_source_path<R>(&self, f: impl FnOnce(&Path) -> R) -> Option<R> {
        self.source_path.borrow().as_deref().map(f)
    }

    /// Runs `f` against the loaded scene, if any. Returns `None` when
    /// no scene is installed. Callers must keep the closure short — the
    /// internal borrow is held for its duration.
    pub fn with_scene<R>(&self, f: impl FnOnce(&Scene) -> R) -> Option<R> {
        self.scene.borrow().as_ref().map(f)
    }

    /// Mutable counterpart to `with_scene`. Same borrow-scope rules
    /// apply; nesting another scene access (read or write) inside `f`
    /// will panic on the inner borrow.
    pub fn with_scene_mut<R>(&self, f: impl FnOnce(&mut Scene) -> R) -> Option<R> {
        self.scene.borrow_mut().as_mut().map(f)
    }

    pub fn is_loaded(&self) -> bool {
        self.scene.borrow().is_some()
    }
}

#[cfg(test)]
mod scene_document_tests {
    use super::*;
    use sc_mesh_core::scene::{SceneObject, SceneObjectContent};

    fn empty_scene() -> Scene {
        Scene::new(None, Vec::new(), Vec::new())
    }

    fn fixture_path() -> PathBuf {
        PathBuf::from("/tmp/scene_document_fixture.obj")
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
        assert!(doc.source_path().is_none());
    }

    #[test]
    fn install_scene_makes_it_observable() {
        let doc = SceneDocument::new();
        doc.install_scene(empty_scene(), fixture_path());
        assert!(doc.is_loaded());
        let object_count = doc.with_scene(|s| s.objects().len());
        assert_eq!(object_count, Some(0));
        assert_eq!(doc.source_path(), Some(fixture_path()));
    }

    #[test]
    fn clear_scene_removes_it() {
        let doc = SceneDocument::new();
        doc.install_scene(empty_scene(), fixture_path());
        doc.clear_scene();
        assert!(!doc.is_loaded());
        assert!(doc.with_scene(|_| ()).is_none());
        assert!(doc.source_path().is_none());
    }

    #[test]
    fn install_scene_replaces_a_prior_scene() {
        let doc = SceneDocument::new();
        doc.install_scene(empty_scene(), PathBuf::from("/tmp/a.obj"));
        doc.install_scene(empty_scene(), PathBuf::from("/tmp/b.obj"));
        assert!(doc.is_loaded());
        assert_eq!(doc.source_path(), Some(PathBuf::from("/tmp/b.obj")));
    }

    #[test]
    fn with_scene_mut_observes_writes_through_with_scene() {
        let doc = SceneDocument::new();
        doc.install_scene(empty_scene(), fixture_path());
        let added = doc.with_scene_mut(|s| {
            s.objects.push(SceneObject {
                meta: None,
                transform: Default::default(),
                content: SceneObjectContent::Resource(0),
            });
            s.objects.len()
        });
        assert_eq!(added, Some(1));
        let observed = doc.with_scene(|s| s.objects.len());
        assert_eq!(observed, Some(1));
    }

    #[test]
    fn with_scene_mut_on_empty_returns_none() {
        let doc = SceneDocument::new();
        let result = doc.with_scene_mut(|s| s.objects.len());
        assert!(result.is_none());
    }

    #[test]
    fn with_source_path_runs_only_when_a_path_is_set() {
        let doc = SceneDocument::new();
        assert!(doc.with_source_path(|_| 0_usize).is_none());
        doc.install_scene(empty_scene(), fixture_path());
        let stem = doc.with_source_path(|p| p.file_stem().unwrap().to_owned());
        assert_eq!(stem.as_deref(), Some("scene_document_fixture".as_ref()));
    }
}
