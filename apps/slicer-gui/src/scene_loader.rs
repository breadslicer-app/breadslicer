/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Owns the async mesh-load state machine.
//!
//! A load goes through the following states, all gated by a single
//! `inflight` counter so the UI's `loading` property and the reentrancy
//! guard cannot drift:
//!
//! 1. `begin_load()` bumps `inflight` (and flips `loading = true`) or
//!    refuses if a load is already running.
//! 2. `next_load_stamp()` returns a fresh generation; the worker tags its
//!    deposit with this stamp so a cancelled or superseded load is
//!    dropped on arrival.
//! 3. The worker writes `(stamp, path, result)` into `load_slot` and
//!    invokes `scene_load_finished` on the UI thread.
//! 4. The UI callback calls `end_load()` (decrementing `inflight`), then
//!    drains the slot.
//!
//! `invalidate_and_clear()` is the "user closed the scene" shortcut: it
//! bumps the generation so any in-flight worker result is ignored, and
//! drops the already-deposited slot contents immediately so the
//! potentially-large `LoadedMesh` doesn't sit around until the queued
//! finish callback runs.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use sc_mesh_core::scene::Scene;
use sc_mesh_formats::LoadedMesh;
use sc_mesh_formats::messages::{LoadMeshWarning, LoadObjWarning};
use slint::ComponentHandle;

use crate::MainWindow;
use crate::error_dialog::{DialogManager, format_anyhow_chain};
use crate::i18n::ctr;

pub type LoadSlot = Arc<Mutex<Option<(u64, PathBuf, anyhow::Result<LoadedMesh>)>>>;

pub struct SceneLoader {
    ui: slint::Weak<MainWindow>,
    dialogs: Rc<DialogManager>,
    load_gen: Cell<u64>,
    load_slot: LoadSlot,
    // Number of load operations that have begun but not yet called
    // `end_load`. `loading` on the UI stays true while this is > 0, so a
    // stale worker that finishes after a Close cannot prematurely re-enable
    // Open. In practice the count is always 0 or 1, because the open gate
    // rejects a new load while any is in flight.
    inflight: Cell<u32>,
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
            inflight: Cell::new(0),
            upload_failures: Cell::new(0),
        }
    }

    // Exposed so the file-loader worker thread (which can't hold an Rc) can
    // deposit results and tag them with a generation stamp.
    pub fn load_slot(&self) -> LoadSlot {
        self.load_slot.clone()
    }

    pub fn next_load_stamp(&self) -> u64 {
        let stamp = self.load_gen.get().wrapping_add(1);
        self.load_gen.set(stamp);
        stamp
    }

    /// Starts a new load. Returns `false` if one is already in flight and
    /// the caller must abort. Pair every successful call with exactly one
    /// `end_load` (invoked from the finish handler, or manually on paths
    /// that bail before a worker gets spawned).
    pub fn begin_load(&self) -> bool {
        if self.inflight.get() > 0 {
            return false;
        }
        self.inflight.set(self.inflight.get() + 1);
        if let Some(ui) = self.ui.upgrade() {
            ui.set_loading(true);
        }
        true
    }

    pub fn end_load(&self) {
        let remaining = self.inflight.get().saturating_sub(1);
        self.inflight.set(remaining);
        if remaining == 0
            && let Some(ui) = self.ui.upgrade()
        {
            ui.set_loading(false);
        }
    }

    /// Invalidates any in-flight load and drops any already-deposited
    /// result. Called from the "close scene" path so a subsequent worker
    /// arrival is dropped on stamp-check, and so a large `LoadedMesh`
    /// sitting in the slot is freed immediately rather than waiting for
    /// the queued finish callback. Also resets the upload-failure counter
    /// since a new empty state has no prior-upload history.
    pub fn invalidate_and_clear(&self) {
        self.load_gen.set(self.load_gen.get().wrapping_add(1));
        *self
            .load_slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        self.upload_failures.set(0);
    }

    /// Clears the upload-failure counter without touching the load FSM.
    /// Used by `RenderingSetup`: a new device has no prior-upload history,
    /// so any counter from the previous device shouldn't gate the first
    /// dialog on the new one.
    pub fn reset_upload_failures(&self) {
        self.upload_failures.set(0);
    }

    /// A scene upload just succeeded. Rearms the 0 → 1 dialog gate so the
    /// next failure can surface an error.
    pub fn note_upload_success(&self) {
        self.upload_failures.set(0);
    }

    /// A scene upload just failed. Increments the counter and, on the
    /// 0 → 1 transition, surfaces the error dialog. Subsequent failures
    /// for the same scene are swallowed until a reset — this prevents
    /// dialog spam if the renderer retries every frame.
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

    /// Drains the result slot, calls `end_load`, and applies the stamp
    /// check. Returns `Some(scene)` if the caller should install a fresh
    /// scene (also resets the upload-failure counter so the next upload's
    /// error-dialog gate is rearmed). Returns `None` on a stale / empty
    /// deposit, or surfaces the error dialog and returns `None` on a
    /// worker-side failure.
    pub fn take_finished_load(&self) -> Option<Scene> {
        let taken = self
            .load_slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        self.end_load();
        let (stamp, path, result) = taken?;
        if stamp != self.load_gen.get() {
            // The user cancelled (Close) or something else bumped the
            // generation while this worker was parsing. Drop the result.
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

// Load-time warnings (currently OBJ-only) are surfaced to stderr until the
// UI grows a place to show them.
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
