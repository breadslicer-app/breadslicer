/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Document-level model for the loaded scene.
//!
//! Owns the scene, its source path, the selection, and (eventually) the
//! undo/redo command stack. Consumers like the viewport controller and
//! the object-list panel observe the document instead of holding their
//! own copies of this state.
//!
//! This skeleton is intentionally empty: subsequent steps add the
//! `scene` cell, then `source_path`, then ownership of `selection`,
//! then the listener mechanism and `apply_command` chokepoint.

use std::rc::Rc;

pub(crate) struct SceneDocument;

impl SceneDocument {
    pub fn new() -> Rc<Self> {
        Rc::new(Self)
    }
}

#[cfg(test)]
mod scene_document_tests {
    use super::*;

    #[test]
    fn new_constructs_a_document() {
        let _doc: Rc<SceneDocument> = SceneDocument::new();
    }
}
