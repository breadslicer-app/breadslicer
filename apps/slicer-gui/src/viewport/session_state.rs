/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Per-viewport session concern: the cross-cutting state that does not belong
//! to any of the other 3 sub-structs but does outlive a single frame and a
//! single event.
//!
//! Owns the document handle, the scene-loader handle, the save and export
//! progress panels, the pending-load slot (a load request that arrived before
//! the GPU device was available), the dialog manager, the document-subscription
//! RAII handle held by the controller for the lifetime of the listener, the
//! live build volume and the active workspace section.
//!
//! Subscriber install lives on the coordinator because the listener body has to
//! dispatch across all 4 concerns and needs a `Weak<ViewportController>`
//! capture; this struct just stores the `SceneSubscription` once installed.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::WorkspaceSection;
use crate::build_volume::{BuildVolume, NO_PRINTER_BUILD_VOLUME};
use crate::error_dialog::DialogManager;
use crate::export_progress_panel::ExportProgressPanel;
use crate::pending_load::PendingLoad;
use crate::save_progress_panel::SaveProgressPanel;
use crate::scene_document::{SceneDocument, SceneSubscription};
use crate::scene_loader::SceneLoader;

pub(super) struct SessionState {
    document: Rc<SceneDocument>,
    loader: Rc<SceneLoader>,
    save_progress: Rc<SaveProgressPanel>,
    export_progress: Rc<ExportProgressPanel>,
    pending_load: Rc<PendingLoad>,
    dialogs: Rc<DialogManager>,
    /// Held by the controller so it's dropped alongside the controller;
    /// populated by `register_document_listener` after the controller is
    /// wrapped in `Rc`.
    document_subscription: RefCell<Option<SceneSubscription>>,
    /// The build volume that this window draws, edits and places against.
    /// `AppState` sets it from `DefaultEnvelope` when it creates the window and
    /// on each change of the default printer.
    build_volume: Cell<BuildVolume>,
    /// Active workspace section (Prepare / Transform / Slice). Per-window live
    /// state, not persisted. `Cell` because it is a plain `Copy` value flipped
    /// by the pill switcher and by the load pipeline (initial section from mesh
    /// health).
    section: Cell<WorkspaceSection>,
}

impl SessionState {
    pub(super) fn new(
        document: Rc<SceneDocument>,
        loader: Rc<SceneLoader>,
        save_progress: Rc<SaveProgressPanel>,
        export_progress: Rc<ExportProgressPanel>,
        pending_load: Rc<PendingLoad>,
        dialogs: Rc<DialogManager>,
    ) -> Self {
        Self {
            document,
            loader,
            save_progress,
            export_progress,
            pending_load,
            dialogs,
            document_subscription: RefCell::new(None),
            build_volume: Cell::new(NO_PRINTER_BUILD_VOLUME),
            section: Cell::new(WorkspaceSection::Transform),
        }
    }

    pub(super) fn document(&self) -> &Rc<SceneDocument> {
        &self.document
    }

    pub(super) fn loader(&self) -> &Rc<SceneLoader> {
        &self.loader
    }

    pub(super) fn save_progress(&self) -> &Rc<SaveProgressPanel> {
        &self.save_progress
    }

    pub(super) fn export_progress(&self) -> &Rc<ExportProgressPanel> {
        &self.export_progress
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

    /// Replaces the build volume. Returns `true` if the value actually changed
    /// (caller marks the renderer dirty so the plate / envelope cage rebuilds
    /// against the new envelope on the next render tick).
    pub(super) fn set_build_volume(&self, bv: BuildVolume) -> bool {
        if self.build_volume.get() == bv {
            return false;
        }
        self.build_volume.set(bv);
        true
    }

    pub(super) fn section(&self) -> WorkspaceSection {
        self.section.get()
    }

    /// Replaces the active workspace section. Returns `true` if the value
    /// actually changed (caller mirrors it to the UI and refreshes the
    /// transform tooling for the new section).
    pub(super) fn set_section(&self, section: WorkspaceSection) -> bool {
        if self.section.get() == section {
            return false;
        }
        self.section.set(section);
        true
    }

    /// Installs the document-subscription RAII handle. Called by the
    /// coordinator's `register_document_listener` once the listener closure has
    /// been built (the closure needs a `Weak<ViewportController>`, which only
    /// the coordinator can provide).
    pub(super) fn install_subscription(&self, sub: SceneSubscription) {
        *self.document_subscription.borrow_mut() = Some(sub);
    }
}

#[cfg(test)]
mod session_state_tests {
    use super::*;
    use crate::preferences_bus::{PreferencesBus, PreferencesSnapshot};
    use crate::scene_loader::SceneLoader;

    fn new_session() -> SessionState {
        let dialogs = DialogManager::new(&PreferencesBus::new(PreferencesSnapshot::for_tests()));
        SessionState::new(
            SceneDocument::new(),
            Rc::new(SceneLoader::new_for_test(dialogs.clone())),
            Rc::new(SaveProgressPanel::new(slint::Weak::default())),
            Rc::new(ExportProgressPanel::new(slint::Weak::default())),
            Rc::new(PendingLoad::new()),
            dialogs,
        )
    }

    #[test]
    fn build_volume_starts_at_the_no_printer_volume() {
        let s = new_session();
        assert_eq!(s.build_volume(), NO_PRINTER_BUILD_VOLUME);
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

    #[test]
    fn section_starts_in_transform() {
        // A no-op set to Transform reports no change, which pins the initial
        // value.
        let s = new_session();
        assert!(!s.set_section(WorkspaceSection::Transform));
    }

    #[test]
    fn set_section_returns_true_on_change() {
        let s = new_session();
        assert!(s.set_section(WorkspaceSection::Supports));
    }

    #[test]
    fn set_section_returns_false_on_no_change() {
        let s = new_session();
        assert!(s.set_section(WorkspaceSection::Supports));
        assert!(!s.set_section(WorkspaceSection::Supports));
    }
}
