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
//! release the gate: the gate releases itself when the ticket goes out
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
use std::time::Duration;

use sc_mesh_formats::messages::{
    LoadMeshWarning, LoadObjWarning, LoadStlWarning, LoadThreeMfWarning, SceneLoadWarning,
};
use sc_mesh_spatial::SceneSpatial;
use sc_project_formats::{ForeignDialect, ProfileRefWarning, ProjectPayload, ReadProjectWarning};
use slint::ComponentHandle;
use slint::wgpu_29::wgpu;

use crate::MainWindow;
use crate::error_dialog::{DialogManager, format_anyhow_chain};
use crate::failure_gate::FailureGate;
use crate::graphics_status::{GraphicsGap, GraphicsUnavailable};
use crate::i18n::ctr;
use crate::load_progress::{
    LoadPhase as WorkerLoadPhase, LoadProgressSink, PROGRESS_PERMILLE_MAX, decode,
};
use crate::mesh_health::MeshHealth;
use crate::mesh_render::{GpuScene, RenderAllocation, RenderError};
use crate::reloaded_slices::ReloadedSlices;

/// Cadence of the UI-side progress poll while a load is in flight.
const PROGRESS_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Maps the worker-side phase onto the UI enum generated from the
/// `LoadPhase` declaration in `main-window.slint`.
fn ui_load_phase(phase: WorkerLoadPhase) -> crate::LoadPhase {
    match phase {
        WorkerLoadPhase::Parse => crate::LoadPhase::ReadingFile,
        WorkerLoadPhase::Priming => crate::LoadPhase::PreparingGeometry,
        WorkerLoadPhase::GpuUpload => crate::LoadPhase::UploadingToGpu,
    }
}

/// Worker-built bundle handed to the UI thread. The `GpuScene`
/// carries the `device_token` identifying the device pairing it was
/// built against; the [`SceneSpatial`] owns the scene plus pre-primed
/// spatial caches and is retained for device-loss recovery (a fresh
/// `RenderingSetup` re-uploads via `Renderer::upload_scene`, which
/// reads the scene through the document's `SceneSpatial`).
pub(crate) struct PreparedScene {
    pub spatial: SceneSpatial,
    pub gpu_scene: GpuScene,
    pub warnings: Vec<LoadMeshWarning>,
    /// Project data the file carried (profile references + embedded
    /// documents). `None` for plain mesh files and for 3MF packages
    /// without a project part. Threaded into the document at install
    /// time; the save flow writes it back out.
    pub project_payload: Option<ProjectPayload>,
    /// Project-level load warnings (foreign project data, unknown
    /// payload versions, unrecognized profile parts).
    pub project_warnings: Vec<ReadProjectWarning>,
    /// Classification of the payload's profile references against
    /// the locally installed profiles, computed on the worker.
    pub profile_warnings: Vec<ProfileRefWarning>,
    /// The project's persisted slices, revalidated against the reopened
    /// scene on the worker. `None` unless a reopened project carried
    /// in-sync slices; installed as the held slice result so the viewer
    /// opens on them without a re-slice.
    pub reloaded_slices: Option<ReloadedSlices>,
    /// The filesystem path the scene was loaded from. Threaded into
    /// the document at install time so callers can derive default
    /// export filenames, surface the source in window titles, etc.
    pub source_path: PathBuf,
    /// Mesh-health verdict computed on the worker (faces + manifold
    /// per leaf). Consumed by the load-finished handler to pick the
    /// initial workspace section: Transform when problem-free, else
    /// Prepare.
    pub mesh_health: MeshHealth,
}

type LoadSlot = Arc<Mutex<Option<(TicketGuard, PathBuf, anyhow::Result<PreparedScene>)>>>;

/// Inflight-counter RAII guard. Stored inside `LoadSlot` after a deposit
/// so the counter is decremented (and the UI's `loading` property is
/// flipped) when the slot is drained or freed.
///
/// Holds no strong reference to the slot it lives in. That separation
/// is what prevents the slot from forming a self-referential `Arc`
/// cycle once a ticket is deposited: nothing inside the `Mutex<Option<..>>`
/// points back at the `Arc` that owns it, so dropping the loader without
/// a drain frees the slot and everything in it.
struct TicketGuard {
    stamp: u64,
    inflight: Arc<AtomicU32>,
    ui: slint::Weak<MainWindow>,
}

impl Drop for TicketGuard {
    fn drop(&mut self) {
        let prior = self.inflight.fetch_sub(1, Ordering::AcqRel);
        // `prior` is at least 1 by construction: a guard exists only
        // while inflight is non-zero.
        if prior != 1 {
            return;
        }
        // Inflight hit zero. Flip the UI's `loading` property.
        // `Weak::upgrade` only returns `Some` on the UI thread with the
        // window still alive, the common case, since guards usually
        // drop inside `take_finished_load` or in the file-dialog
        // closure. From the worker thread (e.g. a panic that didn't
        // reach `deposit`) or when the window has closed, fall back to
        // posting the flip through the event loop.
        if let Some(ui) = self.ui.upgrade() {
            ui.set_loading(false);
            ui.set_load_in_progress(false);
        } else {
            let _ = self.ui.upgrade_in_event_loop(|ui| {
                ui.set_loading(false);
                ui.set_load_in_progress(false);
            });
        }
    }
}

/// RAII handle to an in-flight load. Holding a ticket means the inflight
/// counter is >= 1 and the UI's `loading` property is `true`; dropping
/// the ticket decrements the counter and flips `loading` to `false`
/// when the count hits zero.
///
/// A ticket is created by [`SceneLoader::begin_load`] and consumed by
/// [`LoadTicket::deposit`]. Dropping a ticket without calling `deposit`
/// (user-cancelled dialog, worker thread panic, closed-scene
/// invalidation) is safe: the gate just reopens.
///
/// The strong slot reference lives on the ticket rather than on the
/// internal guard so it gets released the moment the ticket is consumed
/// (deposit destructures `self` and the local slot ref drops at the end
/// of `deposit`'s body) or cancelled (the field drops with the ticket).
/// Crucially, the slot ref does NOT travel into the slot itself, only
/// the guard does, so there is no path that forms a self-cycle.
pub struct LoadTicket {
    guard: TicketGuard,
    slot: LoadSlot,
}

impl LoadTicket {
    /// Stamp issued at ticket creation. `drain_matching_deposit`
    /// compares the deposited guard's stamp against the loader's
    /// current generation to drop stale deposits; tests use this
    /// accessor to assert the gating-and-bumping invariants.
    #[cfg(test)]
    pub(crate) fn stamp(&self) -> u64 {
        self.guard.stamp
    }

    /// Publishes the load result and notifies the UI thread. Consumes
    /// the ticket; the inflight commitment is transferred to the guard
    /// inside the slot, which drops later when the UI handler drains
    /// the slot via `take_finished_load` (or when the slot itself is
    /// freed because the loader was dropped without a drain).
    pub fn deposit(self, path: PathBuf, result: anyhow::Result<PreparedScene>) {
        let LoadTicket { guard, slot } = self;
        let ui = guard.ui.clone();
        *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some((guard, path, result));
        let _ = ui.upgrade_in_event_loop(|ui| {
            ui.invoke_scene_load_finished();
        });
        // `slot` (the local strong Arc clone) drops here, releasing the
        // ticket's strong ref to the slot. Only the guard remains
        // inside the `Mutex<Option<..>>`, and the guard holds no slot
        // reference.
    }
}

pub struct SceneLoader {
    ui: slint::Weak<MainWindow>,
    dialogs: Rc<DialogManager>,
    load_gen: Cell<u64>,
    load_slot: LoadSlot,
    // Shared with each live `LoadTicket` and with any guard that has
    // been deposited into `load_slot`. 0 means "no load in flight";
    // 1 means "exactly one ticket exists" (either held by a pending
    // async closure / worker thread, or transferred to a guard sitting
    // in `load_slot` awaiting drain).
    inflight: Arc<AtomicU32>,
    // Consecutive upload failures for the currently-loaded scene. The
    // renderer drives the count via `note_upload_success` /
    // `note_upload_failure`; the counter gates dialog-spam so the error
    // only fires on the transition from 0 -> 1 failures. Reset on
    // successful upload, scene load/close, and on `RenderingSetup`
    // (via `reset_upload_failures`).
    upload_failures: Cell<u32>,
    // Dialog gate for the renderer's per-frame refusals: one dialog per
    // run of failures.
    render_failures: FailureGate,
    // Current graphics device/queue, populated by `RenderingSetup` and
    // cleared by `RenderingTeardown`. A clone is handed to the worker at
    // `spawn_load` time so parse + GPU upload both run off the UI
    // thread; the UI thread only installs the finished `MeshGpu`.
    gpu: Mutex<Option<(wgpu::Device, crate::mesh_render::DeviceToken, wgpu::Queue)>>,
    // Why `gpu` is empty, for the times it is. Only consulted when the
    // slot holds nothing, so it can never disagree with the slot about
    // whether a device exists. Driven from the same two calls that fill
    // and empty the slot, which is what keeps the pair in step.
    graphics_gap: GraphicsGap,
    // Shared load-progress cell: packed (phase, permille) values, see
    // `load_progress`. The worker writes through a `LoadProgressSink`
    // clone; the UI polls it while a load is in flight.
    progress: Arc<AtomicU32>,
    // Poll timer driving the progress panel. Started by
    // `begin_progress_ui` at dispatch; its callback stops it once the
    // window's `load-in-progress` flips false (the guard's drop).
    // Behind an `Rc` so the callback can reach it through a `Weak`
    // without forming a cycle.
    progress_timer: Rc<slint::Timer>,
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
            render_failures: FailureGate::default(),
            gpu: Mutex::new(None),
            graphics_gap: GraphicsGap::default(),
            progress: Arc::new(AtomicU32::new(0)),
            progress_timer: Rc::new(slint::Timer::default()),
        }
    }

    /// Constructor for pure-state unit tests. The stored `Weak` is
    /// dangling, so the ticket's UI pushes are silently skipped and the
    /// FSM can be exercised without a Slint backend. Tests must avoid
    /// paths that surface a dialog (`take_finished_load` Err branch,
    /// `note_upload_failure` on the 0 -> 1 transition), since
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
            render_failures: FailureGate::default(),
            gpu: Mutex::new(None),
            graphics_gap: GraphicsGap::default(),
            progress: Arc::new(AtomicU32::new(0)),
            progress_timer: Rc::new(slint::Timer::default()),
        }
    }

    /// Sink handle for the shared progress cell, handed to the worker
    /// at dispatch time.
    pub fn progress_sink(&self) -> LoadProgressSink {
        LoadProgressSink::new(self.progress.clone())
    }

    /// Dispatch-time hook: rearm the progress cell, show the progress
    /// panel, and start the poll timer. `start_load_with_ticket`
    /// calls this right before the worker spawns, so the panel never
    /// shows while the file dialog is open nor for pre-spawn
    /// bailouts. Headless callers (unit tests) have no window: the
    /// cell still rearms and the UI side is skipped.
    ///
    /// The poll stops itself: the ticket guard's drop flips the
    /// window's `load-in-progress` to false on every load-exit path,
    /// and the next tick observes that and calls `Timer::stop`
    /// through the `Weak` (no timer/closure `Rc` cycle).
    pub fn begin_progress_ui(&self) {
        self.progress_sink().reset();
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        ui.set_load_phase(ui_load_phase(WorkerLoadPhase::Parse));
        ui.set_load_progress(0.0);
        ui.set_load_in_progress(true);
        let cell = self.progress.clone();
        let ui_weak = self.ui.clone();
        let timer_weak = Rc::downgrade(&self.progress_timer);
        self.progress_timer.start(
            slint::TimerMode::Repeated,
            PROGRESS_POLL_INTERVAL,
            move || {
                let Some(ui) = ui_weak.upgrade() else {
                    if let Some(timer) = timer_weak.upgrade() {
                        timer.stop();
                    }
                    return;
                };
                if !ui.get_load_in_progress() {
                    if let Some(timer) = timer_weak.upgrade() {
                        timer.stop();
                    }
                    return;
                }
                let (phase, permille) = decode(cell.load(Ordering::Relaxed));
                ui.set_load_phase(ui_load_phase(phase));
                ui.set_load_progress(permille as f32 / PROGRESS_PERMILLE_MAX as f32);
            },
        );
    }

    /// Test-only raw view of the progress cell, used to assert the
    /// dispatch-time rearm without a window.
    #[cfg(test)]
    pub(crate) fn progress_value_for_test(&self) -> u32 {
        self.progress.load(Ordering::Relaxed)
    }

    /// Starts a new load. Returns `None` if one is already in flight.
    /// The returned `LoadTicket` is the only way to deposit a result,
    /// and releases the gate on drop.
    ///
    /// The gpu slot is *not* checked here; callers must read it via
    /// [`SceneLoader::gpu_snapshot`] after the user has actually
    /// picked a file, since the loader is constructed before the
    /// first `RenderingSetup` and a synchronous gate would prevent
    /// the file dialog from opening at all in unit-test or
    /// pre-first-paint scenarios.
    pub fn begin_load(&self) -> Option<LoadTicket> {
        // compare_exchange 0 -> 1: atomically checks that no ticket is
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
            guard: TicketGuard {
                stamp,
                inflight: self.inflight.clone(),
                ui: self.ui.clone(),
            },
            slot: self.load_slot.clone(),
        })
    }

    /// Publishes the graphics device/queue. Called from
    /// `handle_rendering_setup`; the worker clones this pair at
    /// `spawn_load` time so parsing + upload both happen off the UI
    /// thread.
    pub fn set_gpu(
        &self,
        device: wgpu::Device,
        device_token: crate::mesh_render::DeviceToken,
        queue: wgpu::Queue,
    ) {
        *self.gpu.lock().unwrap_or_else(PoisonError::into_inner) =
            Some((device, device_token, queue));
        self.graphics_gap.note_device_published();
    }

    /// Records that a `RenderingSetup` arrived carrying a graphics API
    /// this build cannot render through. The device slot stays empty,
    /// and anything that needs a device now has a reason to report
    /// instead of an unexplained absence.
    pub fn note_unsupported_graphics_api(&self, api: String) {
        self.graphics_gap.note_unsupported_api(api);
    }

    /// Why there is no device, or `None` when there is one. Derived
    /// from the device slot so readiness has a single source: the
    /// recorded reason only speaks for an empty slot.
    pub fn graphics_unavailable(&self) -> Option<GraphicsUnavailable> {
        if self
            .gpu
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
        {
            return None;
        }
        Some(self.graphics_gap.reason())
    }

    /// Clears the graphics device/queue. Called on
    /// `RenderingTeardown`. In-flight workers hold `Arc`-backed clones
    /// and continue running; if a resulting `PreparedScene` arrives
    /// against a stale device, `Renderer::install_mesh` drops it and
    /// the UI thread re-uploads via the recovery path.
    pub fn clear_gpu(&self) {
        *self.gpu.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// Snapshot of the current device/queue. `None` if
    /// `RenderingSetup` hasn't fired yet (or a teardown cleared it).
    pub fn gpu_snapshot(
        &self,
    ) -> Option<(wgpu::Device, crate::mesh_render::DeviceToken, wgpu::Queue)> {
        self.gpu
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
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

    /// Test-only accessor for the upload-failure counter, used to
    /// assert that `note_upload_success` / `note_upload_failure` /
    /// `reset_upload_failures` fired the expected number of times
    /// during a `handle_before_rendering` cycle.
    #[cfg(test)]
    pub(crate) fn upload_failures_for_test(&self) -> u32 {
        self.upload_failures.get()
    }

    /// A scene upload just succeeded. Rearms the 0 -> 1 dialog gate so
    /// the next failure can surface an error.
    pub fn note_upload_success(&self) {
        self.upload_failures.set(0);
    }

    /// A scene upload just failed. Increments the counter and, on the
    /// 0 -> 1 transition, surfaces the error dialog. Subsequent
    /// failures for the same scene are swallowed until a reset; this
    /// keeps a scene that fails on every attempt to one dialog.
    pub fn note_upload_failure(&self, err: &anyhow::Error) {
        let prior = self.upload_failures.get();
        self.upload_failures.set(prior.saturating_add(1));
        if prior == 0 {
            self.dialogs.show_error_deferred(
                ctr!("ErrorDialog", "Failed to prepare the 3D view").to_string(),
                Some(
                    ctr!(
                        "ErrorDialog",
                        "The scene could not be handed to the graphics device. The 3D view shows the build plate alone until the next successful update."
                    )
                    .to_string(),
                ),
                err.to_string(),
                format_anyhow_chain(err),
            );
        }
    }

    /// Shows the error dialog only for the first failure of a run.
    pub fn note_render_failure(&self, err: &RenderError) {
        if !self.render_failures.note_failure() {
            return;
        }
        let RenderError::DeviceRefused { what, source } = err;
        let context = match what {
            RenderAllocation::CapTexture => ctr!(
                "ErrorDialog",
                "The cut is drawn without its fill until another layer is shown."
            ),
            _ => ctr!(
                "ErrorDialog",
                "The 3D view keeps its last image until it is resized."
            ),
        };
        self.dialogs.show_error_deferred(
            ctr!("ErrorDialog", "Failed to draw the 3D view").to_string(),
            Some(context.to_string()),
            err.to_string(),
            source.to_string(),
        );
    }

    pub fn note_render_success(&self) {
        self.render_failures.reset();
    }

    pub fn reset_render_failures(&self) {
        self.render_failures.reset();
    }

    #[cfg(test)]
    pub(crate) fn render_failures_for_test(&self) -> u32 {
        self.render_failures.consecutive_failures()
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
    pub fn take_finished_load(&self) -> Option<PreparedScene> {
        let (path, result) = self.drain_matching_deposit()?;
        match result {
            Ok(prepared) => {
                log_load_warnings(&path, &prepared);
                self.upload_failures.set(0);
                Some(prepared)
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

    /// Drains the result slot and applies the stamp check, returning
    /// the raw `(path, result)` for the caller to dispatch on. The
    /// ticket that was in the slot drops here, releasing the gate when
    /// its drop runs the inflight counter down to zero. Returns `None`
    /// for empty / stale deposits, same condition as
    /// `take_finished_load`'s early return, without surfacing any
    /// dialog. Split from `take_finished_load` so the FSM-only
    /// behaviour (drain + stamp + gate release) is unit-testable
    /// without a `wgpu::Device` or a Slint event loop.
    /// Drain the result slot, applying the stamp check, without going
    /// through `take_finished_load`'s `DialogManager::show_error` path.
    /// Tests need this so the Err arm can be asserted on without
    /// requiring a live Slint event loop. Returns the raw
    /// `(path, result)` exactly like the private
    /// `drain_matching_deposit` does.
    #[cfg(test)]
    pub(crate) fn drain_deposit_for_test(
        &self,
    ) -> Option<(PathBuf, anyhow::Result<PreparedScene>)> {
        self.drain_matching_deposit()
    }

    fn drain_matching_deposit(&self) -> Option<(PathBuf, anyhow::Result<PreparedScene>)> {
        let taken = self
            .load_slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let (guard, path, result) = taken?;
        if guard.stamp != self.load_gen.get() {
            // The user cancelled (Close) or something else bumped the
            // generation while this worker was parsing. Drop the
            // guard (and the result along with it).
            return None;
        }
        Some((path, result))
    }
}

// Load-time warnings are surfaced to stderr until the UI grows a
// place to show them. Profile-reference lines come in two tiers: a
// reference with an embedded copy in the file is a note (the data is
// at hand, just not installed), one without is a warning.
fn log_load_warnings(path: &Path, prepared: &PreparedScene) {
    for warning in &prepared.warnings {
        let line = match warning {
            LoadMeshWarning::Obj(w) => describe_obj_warning(w),
            LoadMeshWarning::Stl(w) => describe_stl_warning(w),
            LoadMeshWarning::ThreeMf(w) => describe_threemf_warning(w),
            LoadMeshWarning::Scene(w) => describe_scene_warning(w),
        };
        eprintln!("WARNING [{}]: {}", path.display(), line);
    }
    for warning in &prepared.project_warnings {
        eprintln!(
            "WARNING [{}]: {}",
            path.display(),
            describe_project_warning(warning),
        );
    }
    for warning in &prepared.profile_warnings {
        let tier = if profile_ref_warning_is_informational(warning) {
            "NOTE"
        } else {
            "WARNING"
        };
        eprintln!(
            "{tier} [{}]: {}",
            path.display(),
            describe_profile_ref_warning(warning),
        );
    }
}

pub(crate) fn describe_stl_warning(warning: &LoadStlWarning) -> String {
    match warning {
        LoadStlWarning::HeaderNotText => {
            "binary STL header is not readable text; no mesh name was taken from it".to_string()
        }
    }
}

pub(crate) fn describe_obj_warning(warning: &LoadObjWarning) -> String {
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
        LoadObjWarning::SkippedUnknownDirective { line, directive } => {
            format!("line {line}: skipped OBJ directive '{directive}'")
        }
    }
}

pub(crate) fn describe_threemf_warning(warning: &LoadThreeMfWarning) -> String {
    match warning {
        LoadThreeMfWarning::MaterialsDropped { group_count } => {
            format!("dropped {group_count} material group(s); materials are not supported yet")
        }
        LoadThreeMfWarning::UnbuiltObjectSkipped { id, name } => {
            let label = match name {
                Some(name) => format!("'{name}' (id {id})"),
                None => format!("id {id}"),
            };
            format!("skipped object {label}: it is not referenced by the build")
        }
        LoadThreeMfWarning::NonModelObjectTypeFlattened { id, object_type } => {
            format!(
                "object {id} has type '{}'; it was loaded as plain model geometry",
                object_type.as_str(),
            )
        }
        LoadThreeMfWarning::SliceDataIgnored { stack_count } => {
            format!(
                "ignored {stack_count} slice stack(s); the full-resolution meshes were loaded instead"
            )
        }
        LoadThreeMfWarning::EmptyBuild => "the 3MF build section lists no items".to_string(),
    }
}

pub(crate) fn describe_scene_warning(warning: &SceneLoadWarning) -> String {
    match warning {
        SceneLoadWarning::SharedVertices { leaves } => {
            let labels: Vec<String> = leaves
                .iter()
                .map(|leaf| match &leaf.object_name {
                    Some(name) => format!("'{name}'"),
                    None => format!("node {}", leaf.node_id.get()),
                })
                .collect();
            format!(
                "objects share at least one vertex coordinate: {}",
                labels.join(", "),
            )
        }
    }
}

pub(crate) fn describe_project_warning(warning: &ReadProjectWarning) -> String {
    match warning {
        ReadProjectWarning::ThreeMf(w) => describe_threemf_warning(w),
        ReadProjectWarning::ProjectVersionUnsupported { version } => format!(
            "the file carries project data with version {version}, which this build does not understand; the project settings were ignored"
        ),
        ReadProjectWarning::UnrecognizedProfilePart { part_name } => {
            format!("ignored an unrecognized part in the embedded-profiles area: {part_name}")
        }
        ReadProjectWarning::ForeignProjectData { dialect } => format!(
            "the file carries project data written by {}; only the model data was loaded",
            dialect_name(*dialect),
        ),
    }
}

fn dialect_name(dialect: ForeignDialect) -> &'static str {
    match dialect {
        ForeignDialect::PrusaSlicer => "PrusaSlicer",
        ForeignDialect::BambuStudioOrca => "Bambu Studio / OrcaSlicer",
        ForeignDialect::Cura => "Cura",
    }
}

/// Whether a profile-reference line is the softer, informational
/// tier: the profile is not installed, but the file carries an
/// embedded copy of the document.
pub(crate) fn profile_ref_warning_is_informational(warning: &ProfileRefWarning) -> bool {
    match warning {
        ProfileRefWarning::PrinterEmbeddedOnly { .. }
        | ProfileRefWarning::ResinEmbeddedOnly { .. } => true,
        ProfileRefWarning::PrinterMissing { .. } | ProfileRefWarning::ResinMissing { .. } => false,
    }
}

pub(crate) fn describe_profile_ref_warning(warning: &ProfileRefWarning) -> String {
    match warning {
        ProfileRefWarning::PrinterMissing { id } => format!(
            "the project references printer profile \"{id}\", which is not installed and has no embedded copy in the file"
        ),
        ProfileRefWarning::PrinterEmbeddedOnly { id } => format!(
            "the project references printer profile \"{id}\", which is not installed; the file carries an embedded copy"
        ),
        ProfileRefWarning::ResinMissing { id } => format!(
            "the project references resin profile \"{id}\", which is not installed and has no embedded copy in the file"
        ),
        ProfileRefWarning::ResinEmbeddedOnly { id } => format!(
            "the project references resin profile \"{id}\", which is not installed; the file carries an embedded copy"
        ),
    }
}

#[cfg(test)]
mod scene_loader_tests {
    use super::*;

    fn loader() -> SceneLoader {
        SceneLoader::new_for_test(DialogManager::new())
    }

    /// Stub deposit payload used by FSM-only tests, only the
    /// stamp-check / gate-release / slot-drain paths are exercised
    /// here, never the `Ok(PreparedScene)` arm of `take_finished_load`
    /// (which would require a real `wgpu::Device` to construct a
    /// `MeshGpu`).
    fn stub_err() -> anyhow::Result<PreparedScene> {
        Err(anyhow::anyhow!("stub"))
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
        ticket.deposit(PathBuf::from("/tmp/fake.obj"), stub_err());

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
        // a stale stamp. Stamp mismatch returns `None` before either
        // payload arm runs, so the stub is never inspected.
        ticket.deposit(PathBuf::from("/tmp/fake.obj"), stub_err());

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
    fn drain_returns_payload_for_matching_stamp_and_releases_gate() {
        let loader = loader();
        let ticket = loader.begin_load().expect("begin");
        ticket.deposit(PathBuf::from("/tmp/fake.obj"), stub_err());

        let drained = loader.drain_matching_deposit();
        assert!(
            drained.is_some(),
            "a matching-stamp deposit must drain the slot"
        );
        assert!(
            loader.begin_load().is_some(),
            "the ticket dropped during drain must have released the gate"
        );
    }

    #[test]
    fn drain_on_empty_slot_is_none() {
        let loader = loader();
        let ticket = loader.begin_load().expect("begin");
        // Ticket is alive but nothing is in the slot yet.
        assert!(loader.drain_matching_deposit().is_none());
        drop(ticket);
    }

    #[test]
    fn deposit_after_loader_drop_frees_the_slot() {
        // Window-close-mid-load, scenario A: the worker holds the
        // ticket, the user closes the window (loader is dropped), then
        // the worker finishes parsing and calls `deposit`. With the
        // self-cycle bug, the deposited guard would keep the slot
        // alive forever; with this fix the slot Arc must reach
        // strong_count == 0 by the time `deposit` returns.
        let loader = loader();
        let ticket = loader.begin_load().expect("begin");
        let slot_weak = Arc::downgrade(&loader.load_slot);
        drop(loader);
        ticket.deposit(PathBuf::from("/tmp/fake.obj"), stub_err());
        assert!(
            slot_weak.upgrade().is_none(),
            "the slot Arc must have been freed once the only remaining \
             owner (the local `slot` clone in `deposit`) drops"
        );
    }

    #[test]
    fn loader_drop_after_deposit_frees_the_slot() {
        // Window-close-mid-load, scenario B: the worker has already
        // deposited but the UI hasn't drained yet, then the user
        // closes the window. With the self-cycle bug, the deposited
        // guard would keep the slot alive forever; with this fix the
        // slot Arc must reach strong_count == 0 once the loader is
        // dropped.
        let loader = loader();
        let ticket = loader.begin_load().expect("begin");
        let slot_weak = Arc::downgrade(&loader.load_slot);
        ticket.deposit(PathBuf::from("/tmp/fake.obj"), stub_err());
        drop(loader);
        assert!(
            slot_weak.upgrade().is_none(),
            "the slot Arc must have been freed once the loader (its \
             only remaining strong owner) drops"
        );
    }

    #[test]
    fn begin_progress_ui_rearms_the_progress_cell_headless() {
        let loader = loader();
        loader.progress_sink().set(WorkerLoadPhase::GpuUpload, 1, 1);
        assert_ne!(
            loader.progress_value_for_test(),
            0,
            "a finished load leaves a non-zero packed value behind"
        );
        loader.begin_progress_ui();
        assert_eq!(
            loader.progress_value_for_test(),
            0,
            "dispatch must rearm the cell to (Parse, 0)"
        );
    }

    #[test]
    fn clear_gpu_empties_the_snapshot() {
        let loader = loader();
        // `new_for_test` starts with `None`; verify the initial state
        // and the post-clear state both read as empty. (Populating the
        // slot with a real `(Device, Queue)` pair would require a
        // `wgpu::Instance`, which belongs in an integration test.)
        assert!(
            loader.gpu_snapshot().is_none(),
            "a freshly constructed loader has no gpu snapshot"
        );
        loader.clear_gpu();
        assert!(
            loader.gpu_snapshot().is_none(),
            "clear_gpu on an already-empty slot must stay empty"
        );
    }

    #[test]
    fn a_fresh_loader_is_awaiting_setup() {
        let loader = loader();
        assert_eq!(
            loader.graphics_unavailable(),
            Some(GraphicsUnavailable::AwaitingSetup),
            "before any setup, a device may still arrive"
        );
    }

    #[test]
    fn an_unsupported_api_is_remembered() {
        let loader = loader();
        loader.note_unsupported_graphics_api("GraphicsAPI::NativeOpenGL".to_string());
        assert_eq!(
            loader.graphics_unavailable(),
            Some(GraphicsUnavailable::UnsupportedApi(
                "GraphicsAPI::NativeOpenGL".to_string()
            ))
        );
    }

    #[test]
    fn a_second_unsupported_api_replaces_the_first() {
        // The window can only render through one API at a time, so the
        // latest setup is the one that describes the current state.
        let loader = loader();
        loader.note_unsupported_graphics_api("GraphicsAPI::NativeOpenGL".to_string());
        loader.note_unsupported_graphics_api("GraphicsAPI::WebGL".to_string());
        assert_eq!(
            loader.graphics_unavailable(),
            Some(GraphicsUnavailable::UnsupportedApi(
                "GraphicsAPI::WebGL".to_string()
            ))
        );
    }

    #[test]
    fn an_unsupported_api_survives_a_teardown() {
        // A teardown does not make an unusable renderer usable, so the
        // reason has to outlive it; otherwise the next request would
        // claim a device is still on its way.
        let loader = loader();
        loader.note_unsupported_graphics_api("GraphicsAPI::NativeOpenGL".to_string());
        loader.clear_gpu();
        assert_eq!(
            loader.graphics_unavailable(),
            Some(GraphicsUnavailable::UnsupportedApi(
                "GraphicsAPI::NativeOpenGL".to_string()
            ))
        );
    }
}
