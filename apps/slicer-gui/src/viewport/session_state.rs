/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Per-viewport session concern: the cross-cutting state that does
//! not belong to any of the other three sub-structs but does outlive
//! a single frame and a single event.
//!
//! Owns the document handle, the scene-loader handle, the pending-
//! load slot (a load request that arrived before the GPU device was
//! available), the dialog manager, the document-subscription RAII
//! handle held by the controller for the lifetime of the listener,
//! and the live build volume.
//!
//! Subscriber install lives on the coordinator because the listener
//! body has to dispatch across all four concerns and needs a
//! `Weak<ViewportController>` capture; this struct just stores the
//! `SceneSubscription` once installed.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::build_volume::{BuildVolume, HARDCODED_DEFAULT_BUILD_VOLUME};
use crate::error_dialog::DialogManager;
use crate::pending_load::PendingLoad;
use crate::scene_document::{SceneDocument, SceneSubscription};
use crate::scene_loader::SceneLoader;

pub(super) struct SessionState {
    document: Rc<SceneDocument>,
    loader: Rc<SceneLoader>,
    pending_load: Rc<PendingLoad>,
    dialogs: Rc<DialogManager>,
    /// Held by the controller so it's dropped alongside the
    /// controller; populated by `register_document_listener` after
    /// the controller is wrapped in `Rc`.
    document_subscription: RefCell<Option<SceneSubscription>>,
    /// Build envelope used to clamp translate edits. `Cell` so the
    /// hardcoded default can later be swapped at runtime by the
    /// preferences pipeline (and by tests, which inject a smaller
    /// envelope to make the wall reachable in a few drag pixels).
    build_volume: Cell<BuildVolume>,
}

impl SessionState {
    pub(super) fn new(
        document: Rc<SceneDocument>,
        loader: Rc<SceneLoader>,
        pending_load: Rc<PendingLoad>,
        dialogs: Rc<DialogManager>,
    ) -> Self {
        Self {
            document,
            loader,
            pending_load,
            dialogs,
            document_subscription: RefCell::new(None),
            build_volume: Cell::new(HARDCODED_DEFAULT_BUILD_VOLUME),
        }
    }

    pub(super) fn document(&self) -> &Rc<SceneDocument> {
        &self.document
    }

    pub(super) fn loader(&self) -> &Rc<SceneLoader> {
        &self.loader
    }

    pub(super) fn dialogs(&self) -> &Rc<DialogManager> {
        &self.dialogs
    }

    pub(super) fn pending_load(&self) -> &Rc<PendingLoad> {
        &self.pending_load
    }

    pub(super) fn build_volume(&self) -> BuildVolume {
        self.build_volume.get()
    }

    /// Replaces the build volume. Returns `true` if the value
    /// actually changed (caller marks the renderer dirty so the
    /// plate / envelope cage rebuilds against the new envelope on
    /// the next render tick).
    pub(super) fn set_build_volume(&self, bv: BuildVolume) -> bool {
        if self.build_volume.get() == bv {
            return false;
        }
        self.build_volume.set(bv);
        true
    }

    /// Installs the document-subscription RAII handle. Called by the
    /// coordinator's `register_document_listener` once the listener
    /// closure has been built (the closure needs a
    /// `Weak<ViewportController>`, which only the coordinator can
    /// provide).
    pub(super) fn install_subscription(&self, sub: SceneSubscription) {
        *self.document_subscription.borrow_mut() = Some(sub);
    }
}

#[cfg(test)]
mod session_state_tests {
    use super::*;
    use crate::scene_loader::SceneLoader;

    fn new_session() -> SessionState {
        let dialogs = DialogManager::new();
        SessionState::new(
            SceneDocument::new(),
            Rc::new(SceneLoader::new_for_test(dialogs.clone())),
            Rc::new(PendingLoad::new()),
            dialogs,
        )
    }

    #[test]
    fn build_volume_starts_at_hardcoded_default() {
        let s = new_session();
        assert_eq!(s.build_volume(), HARDCODED_DEFAULT_BUILD_VOLUME);
    }

    #[test]
    fn set_build_volume_returns_true_on_change() {
        let s = new_session();
        let bv = BuildVolume::new(50.0, 50.0, 50.0);
        assert!(s.set_build_volume(bv));
        assert_eq!(s.build_volume(), bv);
    }

    #[test]
    fn set_build_volume_returns_false_on_no_change() {
        let s = new_session();
        let same = s.build_volume();
        assert!(!s.set_build_volume(same));
    }
}
