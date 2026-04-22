/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Owns the async mesh-load state machine.
//!
//! The FSM is typed: `begin_load` returns an `Option<LoadTicket>`. A
//! ticket is the only way to deposit a worker result into the slot, and
//! its `Drop` impl decrements the inflight counter (and flips the UI's
//! `loading` property when the count hits zero). The caller cannot
//! forge a stamp, cannot deposit without a ticket, and cannot forget to
//! release the gate — the gate releases itself when the ticket goes out
//! of scope.
//!
//! A typical load:
//!
//! 1. The UI callback calls [`SceneLoader::begin_load`]. If a load is
//!    already in flight, this returns `None` and the callback bails.
//! 2. The ticket is moved into the async file-dialog closure.
//! 3. If the user cancels, the async block ends and the ticket drops,
//!    releasing the gate.
//! 4. If the user picks a file, the ticket is moved into the worker
//!    thread which calls [`LoadTicket::deposit`] with the load result.
//!    `deposit` publishes the ticket into the slot and invokes
//!    `scene_load_finished` on the UI thread.
//! 5. The UI handler calls [`SceneLoader::take_finished_load`], which
//!    drains the slot, checks the ticket's stamp against the current
//!    generation, and either returns the scene or surfaces an error.
//!    Either way, the ticket drops here on the UI thread.
//!
//! [`SceneLoader::invalidate_and_clear`] is the "user closed the scene"
//! shortcut: it bumps the generation so any still-in-flight deposit is
//! dropped on stamp-check, and drops any already-deposited slot
//! contents immediately so a large `LoadedMesh` isn't retained until
//! the queued finish callback runs. Dropping the slot also drops the
//! ticket it carried, releasing the gate.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use sc_mesh_core::scene::Scene;
use sc_mesh_formats::LoadedMesh;
use sc_mesh_formats::messages::{LoadMeshWarning, LoadObjWarning};
use slint::ComponentHandle;

use crate::MainWindow;
use crate::error_dialog::{DialogManager, format_anyhow_chain};
use crate::i18n::ctr;

type LoadSlot = Arc<Mutex<Option<(LoadTicket, PathBuf, anyhow::Result<LoadedMesh>)>>>;

/// RAII handle to an in-flight load. Holding a ticket means the inflight
/// counter is >= 1 and the UI's `loading` property is `true`; dropping
/// the ticket decrements the counter and flips `loading` to `false`
/// when the count hits zero.
///
/// A ticket is created by [`SceneLoader::begin_load`] and consumed by
/// [`LoadTicket::deposit`]. Dropping a ticket without calling `deposit`
/// (user-cancelled dialog, worker thread panic, closed-scene
/// invalidation) is safe: the gate just reopens.
pub struct LoadTicket {
    stamp: u64,
    inflight: Arc<AtomicU32>,
    slot: LoadSlot,
    ui: slint::Weak<MainWindow>,
}

impl LoadTicket {
    /// Stamp issued at ticket creation. `take_finished_load` compares
    /// this against the loader's current generation to drop stale
    /// deposits.
    pub fn stamp(&self) -> u64 {
        self.stamp
    }

    /// Publishes the load result and notifies the UI thread. Consumes
    /// the ticket; the ticket drops later, when the UI handler drains
    /// the slot via `take_finished_load`.
    pub fn deposit(self, path: PathBuf, result: anyhow::Result<LoadedMesh>) {
        let ui = self.ui.clone();
        let slot = self.slot.clone();
        *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some((self, path, result));
        let _ = ui.upgrade_in_event_loop(|ui| {
            ui.invoke_scene_load_finished();
        });
    }
}

impl Drop for LoadTicket {
    fn drop(&mut self) {
        let prior = self.inflight.fetch_sub(1, Ordering::AcqRel);
        // `prior` is at least 1 by construction: a ticket exists only
        // while inflight is non-zero.
        if prior != 1 {
            return;
        }
        // Inflight hit zero. Flip the UI's `loading` property.
        // `Weak::upgrade` only returns `Some` on the UI thread with the
        // window still alive — the common case, since tickets usually
        // drop inside `take_finished_load` or in the file-dialog
        // closure. From the worker thread (e.g. a panic that didn't
        // reach `deposit`) or when the window has closed, fall back to
        // posting the flip through the event loop.
        if let Some(ui) = self.ui.upgrade() {
            ui.set_loading(false);
        } else {
            let _ = self.ui.upgrade_in_event_loop(|ui| {
                ui.set_loading(false);
            });
        }
    }
}

pub struct SceneLoader {
    ui: slint::Weak<MainWindow>,
    dialogs: Rc<DialogManager>,
    load_gen: Cell<u64>,
    load_slot: LoadSlot,
    // Shared with each live `LoadTicket`. 0 means "no load in flight";
    // 1 means "exactly one ticket exists" (either held by a pending
    // async closure / worker thread, or deposited into `load_slot`
    // awaiting drain).
    inflight: Arc<AtomicU32>,
    // Consecutive upload failures for the currently-loaded scene. The
    // renderer drives the count via `note_upload_success` /
    // `note_upload_failure`; the counter gates dialog-spam so the error
    // only fires on the transition from 0 → 1 failures. Reset on
    // successful upload, scene load/close, and on `RenderingSetup`
    // (via `reset_upload_failures`).
    upload_failures: Cell<u32>,
}

impl SceneLoader {
    pub fn new(ui: &MainWindow, dialogs: Rc<DialogManager>) -> Self {
        Self {
            ui: ui.as_weak(),
            dialogs,
            load_gen: Cell::new(0),
            load_slot: Arc::new(Mutex::new(None)),
            inflight: Arc::new(AtomicU32::new(0)),
            upload_failures: Cell::new(0),
        }
    }

    /// Constructor for pure-state unit tests. The stored `Weak` is
    /// dangling, so the ticket's UI pushes are silently skipped and the
    /// FSM can be exercised without a Slint backend. Tests must avoid
    /// paths that surface a dialog (`take_finished_load` Err branch,
    /// `note_upload_failure` on the 0 → 1 transition), since
    /// `DialogManager` there calls into Slint component construction
    /// and requires an event loop.
    #[cfg(test)]
    pub(crate) fn new_for_test(dialogs: Rc<DialogManager>) -> Self {
        Self {
            ui: slint::Weak::default(),
            dialogs,
            load_gen: Cell::new(0),
            load_slot: Arc::new(Mutex::new(None)),
            inflight: Arc::new(AtomicU32::new(0)),
            upload_failures: Cell::new(0),
        }
    }

    /// Starts a new load. Returns `None` if one is already in flight.
    /// The returned `LoadTicket` is the only way to deposit a result,
    /// and releases the gate on drop.
    pub fn begin_load(&self) -> Option<LoadTicket> {
        // compare_exchange 0 → 1: atomically checks that no ticket is
        // alive. `AcqRel` on success synchronises with the prior
        // ticket's `fetch_sub` release, so a fresh begin sees all
        // writes the previous load made.
        self.inflight
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;

        let stamp = self.load_gen.get().wrapping_add(1);
        self.load_gen.set(stamp);

        if let Some(ui) = self.ui.upgrade() {
            ui.set_loading(true);
        }

        Some(LoadTicket {
            stamp,
            inflight: self.inflight.clone(),
            slot: self.load_slot.clone(),
            ui: self.ui.clone(),
        })
    }

    /// Invalidates any in-flight load and drops any already-deposited
    /// result. Called from the "close scene" path so a subsequent
    /// worker arrival is dropped on stamp-check, and so a large
    /// `LoadedMesh` sitting in the slot is freed immediately. Dropping
    /// the slot also drops the ticket it carried, which releases the
    /// gate via the ticket's `Drop`. Also resets the upload-failure
    /// counter since a new empty state has no prior-upload history.
    pub fn invalidate_and_clear(&self) {
        self.load_gen.set(self.load_gen.get().wrapping_add(1));
        *self
            .load_slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
        self.upload_failures.set(0);
    }

    /// Clears the upload-failure counter without touching the load
    /// FSM. Used by `RenderingSetup`: a new device has no prior-upload
    /// history, so any counter from the previous device shouldn't gate
    /// the first dialog on the new one.
    pub fn reset_upload_failures(&self) {
        self.upload_failures.set(0);
    }

    /// A scene upload just succeeded. Rearms the 0 → 1 dialog gate so
    /// the next failure can surface an error.
    pub fn note_upload_success(&self) {
        self.upload_failures.set(0);
    }

    /// A scene upload just failed. Increments the counter and, on the
    /// 0 → 1 transition, surfaces the error dialog. Subsequent
    /// failures for the same scene are swallowed until a reset — this
    /// prevents dialog spam if the renderer retries every frame.
    pub fn note_upload_failure(&self, err: &anyhow::Error) {
        let prior = self.upload_failures.get();
        self.upload_failures.set(prior.saturating_add(1));
        if prior == 0 {
            self.dialogs.show_error_deferred(
                ctr!("ErrorDialog", "Failed to load the 3D model").to_string(),
                Some(
                    ctr!(
                        "ErrorDialog",
                        "The mesh could not be prepared for rendering."
                    )
                    .to_string(),
                ),
                err.to_string(),
                format_anyhow_chain(err),
            );
        }
    }

    /// Drains the result slot and applies the stamp check. Returns
    /// `Some(scene)` if the caller should install a fresh scene (also
    /// resets the upload-failure counter so the next upload's
    /// error-dialog gate is rearmed). Returns `None` on a stale /
    /// empty deposit, or surfaces the error dialog and returns `None`
    /// on a worker-side failure.
    ///
    /// The ticket that was in the slot drops here on the UI thread,
    /// which synchronously flips `loading = false` when the counter
    /// hits zero.
    pub fn take_finished_load(&self) -> Option<Scene> {
        let taken = self
            .load_slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let (ticket, path, result) = taken?;
        if ticket.stamp() != self.load_gen.get() {
            // The user cancelled (Close) or something else bumped the
            // generation while this worker was parsing. Drop the
            // ticket (and the result along with it).
            return None;
        }
        match result {
            Ok(LoadedMesh { scene, warnings }) => {
                log_load_warnings(&path, &warnings);
                self.upload_failures.set(0);
                Some(scene)
            }
            Err(err) => {
                self.dialogs.show_error(
                    ctr!("ErrorDialog", "Failed to open 3D model").as_str(),
                    Some(ctr!("ErrorDialog", "File: {}", path.display()).to_string()),
                    err.to_string(),
                    format_anyhow_chain(&err),
                );
                None
            }
        }
    }
}

// Load-time warnings (currently OBJ-only) are surfaced to stderr until
// the UI grows a place to show them.
fn log_load_warnings(path: &Path, warnings: &[LoadMeshWarning]) {
    for warning in warnings {
        match warning {
            LoadMeshWarning::Obj(w) => {
                eprintln!("WARNING [{}]: {}", path.display(), describe_obj_warning(w));
            }
        }
    }
}

fn describe_obj_warning(warning: &LoadObjWarning) -> String {
    match warning {
        LoadObjWarning::NoVertices => "OBJ file has no vertices".to_string(),
        LoadObjWarning::NoFaces => "OBJ file has no faces".to_string(),
        LoadObjWarning::SkippedTextureCoordinates { line } => {
            format!("line {line}: skipped texture coordinates")
        }
        LoadObjWarning::SkippedVertexNormal { line } => {
            format!("line {line}: skipped vertex normal")
        }
        LoadObjWarning::SkippedGroup { line, name } => {
            format!("line {line}: skipped group '{name}'")
        }
        LoadObjWarning::SkippedSmoothingGroup { line } => {
            format!("line {line}: skipped smoothing group")
        }
        LoadObjWarning::SkippedMaterialReference { line, material } => {
            format!("line {line}: skipped material reference '{material}'")
        }
        LoadObjWarning::SkippedMaterialLibrary { line, path } => {
            format!("line {line}: skipped material library '{path}'")
        }
        LoadObjWarning::SkippedComment { line } => format!("line {line}: skipped comment"),
        LoadObjWarning::SharedVertices {
            object_name,
            shared_indices,
        } => match object_name {
            Some(name) => format!(
                "object '{name}' shares global vertex indices with previously loaded objects: {shared_indices:?}"
            ),
            None => format!(
                "an unnamed object shares global vertex indices with previously loaded objects: {shared_indices:?}"
            ),
        },
    }
}

#[cfg(test)]
mod scene_loader_tests {
    use super::*;

    use sc_mesh_core::scene::Scene;
    use sc_mesh_formats::LoadedMesh;

    fn loader() -> SceneLoader {
        SceneLoader::new_for_test(DialogManager::new())
    }

    fn empty_loaded_mesh() -> LoadedMesh {
        LoadedMesh {
            scene: Scene::default(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn begin_load_rejects_while_ticket_alive() {
        let loader = loader();
        let ticket = loader.begin_load().expect("first begin_load succeeds");
        assert!(
            loader.begin_load().is_none(),
            "second begin_load must fail while a ticket is alive"
        );
        drop(ticket);
    }

    #[test]
    fn dropping_ticket_without_deposit_reopens_the_gate() {
        let loader = loader();
        let ticket = loader.begin_load().expect("first begin_load succeeds");
        drop(ticket);
        assert!(
            loader.begin_load().is_some(),
            "dropping the ticket must release the gate"
        );
    }

    #[test]
    fn ticket_stamp_matches_the_freshly_issued_generation() {
        let loader = loader();
        let ticket_a = loader.begin_load().expect("a");
        let stamp_a = ticket_a.stamp();
        drop(ticket_a);
        let ticket_b = loader.begin_load().expect("b");
        let stamp_b = ticket_b.stamp();
        assert_eq!(
            stamp_b,
            stamp_a.wrapping_add(1),
            "each successful begin_load should bump the stamp by 1"
        );
    }

    #[test]
    fn invalidate_and_clear_advances_generation_between_loads() {
        let loader = loader();
        let ticket_a = loader.begin_load().expect("a");
        let stamp_a = ticket_a.stamp();
        drop(ticket_a);

        loader.invalidate_and_clear();

        let ticket_b = loader.begin_load().expect("b");
        let stamp_b = ticket_b.stamp();
        // invalidate_and_clear bumps the generation once; then begin_load
        // bumps it again.
        assert_eq!(stamp_b, stamp_a.wrapping_add(2));
    }

    #[test]
    fn invalidate_and_clear_drops_deposited_result_and_reopens_gate() {
        let loader = loader();
        let ticket = loader.begin_load().expect("begin");
        ticket.deposit(PathBuf::from("/tmp/fake.obj"), Ok(empty_loaded_mesh()));

        loader.invalidate_and_clear();

        assert!(
            loader
                .load_slot
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_none(),
            "invalidate_and_clear must drop any pending deposit"
        );
        assert!(
            loader.begin_load().is_some(),
            "the ticket dropped with the slot must have released the gate"
        );
    }

    #[test]
    fn take_finished_load_on_empty_slot_is_none() {
        let loader = loader();
        let ticket = loader.begin_load().expect("begin");
        // Ticket is alive but nothing is in the slot yet.
        assert!(loader.take_finished_load().is_none());
        drop(ticket);
    }

    #[test]
    fn take_finished_load_drops_stale_stamped_result() {
        let loader = loader();
        let ticket = loader.begin_load().expect("begin");
        // User closes the scene while the worker is still running: the
        // ticket is still in flight, but the generation advances.
        loader.invalidate_and_clear();
        // Worker finishes and deposits; the ticket in the slot now has
        // a stale stamp.
        ticket.deposit(PathBuf::from("/tmp/fake.obj"), Ok(empty_loaded_mesh()));

        assert!(
            loader.take_finished_load().is_none(),
            "a stale-stamped deposit must be dropped"
        );
        assert!(
            loader.begin_load().is_some(),
            "the stale ticket's drop must have released the gate"
        );
    }

    #[test]
    fn take_finished_load_returns_scene_for_matching_stamp() {
        let loader = loader();
        let ticket = loader.begin_load().expect("begin");
        ticket.deposit(PathBuf::from("/tmp/fake.obj"), Ok(empty_loaded_mesh()));

        assert!(loader.take_finished_load().is_some());
        assert!(
            loader.begin_load().is_some(),
            "the ticket dropped during take_finished_load must have released the gate"
        );
    }
}
