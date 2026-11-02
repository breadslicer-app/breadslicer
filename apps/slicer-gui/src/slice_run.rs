/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Per-window driver for the Slice action.
//!
//! The Slice button snapshots the scene and the chosen inputs on the UI thread,
//! then runs the projection and the slicing engine on a worker thread so the
//! event loop stays responsive. Progress is published through shared atomics
//! that a `slint::Timer` polls into the modal; the finished result is deposited
//! into a slot and picked up on the UI thread via `invoke_slice_finished`. A
//! `CancelToken` lets the modal's Cancel button stop the run between layers.
//!
//! The window's `slicing-in-progress` property is the single gate: the button
//! is disabled while it is set, and it is cleared on every exit path
//! (completion, cancellation, error, worker panic). The worker wraps its body
//! in `catch_unwind` so a panic still deposits a result and the modal always
//! closes.
//!
//! Right after the engine finishes, the worker reviews the per-leaf stacks
//! (`crate::review`, #90) before merging them, and the findings travel to
//! the Review card with the slice. A review that fails leaves the slice as
//! it is.
//!
//! The cavities marked to fill in the Review card (#132) travel with the run:
//! the worker cuts them on the run's schedule, reviews them as model and
//! merges them into the plate, so the preview, the printer file and its
//! estimates all cure them. A Slice press with fills to change re-slices
//! even when nothing else changed. Fix All (#156) adds thin features to
//! thicken, cured the same way, and specks to erase: their stack is cut from
//! the model leaves and the fills and merged with them, which removes the
//! specks from all three.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use sc_print_glue::{
    PlateContours, SlicingParams, SupportOwners, slice_input_fingerprint, slice_plate,
};
use sc_print_primitives::{AaLevel, MaterialId, PositiveFiniteMm, PrinterId, ScaleCompensation};
use sc_review::{CavityFill, fill_stack};
use sc_scene::{NodeId, Scene};
use sc_sl1cer::error::{InvalidLeafGeometry, LeafValidationFailure, SliceSceneError};
use sc_sl1cer::{CancelToken, SliceProgress};
use slint::ComponentHandle;

use crate::MainWindow;
use crate::error_dialog::{DialogManager, ErrorTexts};
use crate::i18n::ctr;
use crate::mesh_loader::panic_message;
use crate::preferences_bus::PreferencesBus;
use crate::print_selection::PrintDefaults;
use crate::repair_report::object_names_by_leaf;
use crate::review::{ReviewOutcome, erased_stack, review_slices};
use crate::review_panel::ReviewPanel;
use crate::scene_document::SceneDocument;
use crate::slice_projection::{SliceFailure, current_layer_values, load_selected_profiles};
use crate::slice_result::{
    LayerValues, ReSliceDecision, SliceParams, SliceResult, decide_re_slice, layers_changed,
};
use crate::support_persist;

/// Cadence of the UI-side progress poll while a slice is in flight.
const PROGRESS_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long the "already up to date" hint stays up after an unchanged Slice
/// press before it fades back out.
const UP_TO_DATE_HINT_DURATION: Duration = Duration::from_secs(10);

/// Shared completed/total layer counters: the worker writes them, the UI polls
/// them into the modal. Both count plate layers, not the engine's layer-slices
/// (one per leaf and layer, #355), so the modal's total is the layer count the
/// Result card shows. `completed` climbs monotonically (`fetch_max`); `total`
/// is the run's fixed denominator. `reviewing` is raised once every layer is
/// cut, while the review runs.
#[derive(Default)]
struct SliceProgressCell {
    completed: AtomicU32,
    total: AtomicU32,
    reviewing: AtomicBool,
}

impl SliceProgressCell {
    /// Publishes an engine report in plate layers. Reports arrive out of
    /// order from the layer tasks, so `completed` only ever rises.
    fn record(&self, progress: SliceProgress) {
        self.completed
            .fetch_max(progress.layers_completed() as u32, Ordering::Relaxed);
        self.total.store(progress.layers as u32, Ordering::Relaxed);
    }
}

/// A completed slice, already merged, fingerprinted and reviewed on the worker
/// thread so the event loop never does that work.
struct CompletedSlice {
    contours: PlateContours,
    token: String,
    review: ReviewOutcome,
    /// The values the run's profiles resolved to (#280).
    layers: Option<LayerValues>,
}

/// The worker's finished payload, past the engine's raw per-leaf output: a
/// completed, merged, fingerprinted slice, or a cancellation. The slice is
/// boxed so a cancellation does not carry its size; one allocation per run.
enum SliceRunOutcome {
    Completed(Box<CompletedSlice>),
    Cancelled,
}

type SliceSlot = Arc<Mutex<Option<Result<SliceRunOutcome, SliceFailure>>>>;

pub(crate) struct SliceRun {
    ui: slint::Weak<MainWindow>,
    bus: Rc<PreferencesBus>,
    document: Rc<SceneDocument>,
    dialogs: Rc<DialogManager>,
    // The Review card, which receives each completed slice's findings.
    review: Rc<ReviewPanel>,
    progress: Arc<SliceProgressCell>,
    slot: SliceSlot,
    // The current run's cancel token, so the modal's Cancel can trip it. `None`
    // when no slice is in flight.
    cancel: RefCell<Option<CancelToken>>,
    // Inputs of the in-flight run, captured at launch so the finished handler
    // can record them alongside the produced contours. `None` when no slice is
    // in flight.
    pending_params: RefCell<Option<SliceParams>>,
    // The ids of the Review card's fills the in-flight run cures, for the
    // card once it lands.
    pending_fills: RefCell<Vec<u64>>,
    // Poll timer driving the modal's progress readout. Behind an `Rc` so its
    // own callback can stop it through a `Weak` without a cycle.
    poll_timer: Rc<slint::Timer>,
    // Single-shot timer that lowers the "already up to date" hint after a
    // delay. Restarted on each unchanged press so repeated presses keep it
    // visible; stopped when a real slice starts.
    tooltip_timer: Rc<slint::Timer>,
}

impl SliceRun {
    /// Wires the Slice / Cancel / finished callbacks and returns the driver,
    /// which the caller keeps alive for the window's lifetime.
    pub(crate) fn install(
        ui: &MainWindow,
        bus: Rc<PreferencesBus>,
        document: Rc<SceneDocument>,
        dialogs: Rc<DialogManager>,
        review: Rc<ReviewPanel>,
    ) -> Rc<Self> {
        let run = Rc::new(Self {
            ui: ui.as_weak(),
            bus,
            document,
            dialogs,
            review,
            progress: Arc::new(SliceProgressCell::default()),
            slot: Arc::new(Mutex::new(None)),
            cancel: RefCell::new(None),
            pending_params: RefCell::new(None),
            pending_fills: RefCell::new(Vec::new()),
            poll_timer: Rc::new(slint::Timer::default()),
            tooltip_timer: Rc::new(slint::Timer::default()),
        });
        {
            let run = run.clone();
            ui.on_slice_requested(move || run.start());
        }
        {
            // Fix All (#156) slices again on its own: no overwrite to confirm.
            let weak = Rc::downgrade(&run);
            run.review
                .set_reslice(move || weak.upgrade().is_some_and(|run| run.start_with(true)));
        }
        {
            let run = run.clone();
            ui.on_slice_cancel_requested(move || run.cancel());
        }
        {
            let run = run.clone();
            ui.on_slice_finished(move || run.finish());
        }
        run
    }

    /// Handles a Slice press. Resolves the inputs in force, then decides
    /// against the held result: a first slice or a changed input launches the
    /// run (a change first confirms the overwrite); an unchanged press does
    /// nothing. A no-op while a slice is already in flight; an error dialog
    /// when no printer or no resin is in force (the button gates both, but the
    /// checks keep the path safe).
    fn start(self: &Rc<Self>) {
        self.start_with(false);
    }

    /// [`start`](Self::start), for the Slice button (`by_fix_all` false) or
    /// for the Review card's Fix All, which promised a new slice: a changed
    /// input then launches without asking to overwrite the held slice.
    /// Returns whether a slice started.
    fn start_with(self: &Rc<Self>, by_fix_all: bool) -> bool {
        let Some(ui) = self.ui.upgrade() else {
            return false;
        };
        // While Fix All runs, the Slice button waits: the slices of its
        // passes come from the run itself (#390, #163).
        if ui.get_slicing_in_progress() || (!by_fix_all && self.review.fixing()) {
            return false;
        }
        let snapshot = self.bus.snapshot();
        let selection = self
            .document
            .print_selection()
            .in_force(&PrintDefaults::from(&snapshot));
        let Some(current) = SliceParams::for_selection(
            &selection,
            snapshot.aa_level,
            self.document.scene_revision(),
        ) else {
            self.dialogs.show_error(missing_selection_texts);
            return false;
        };

        let held = self.document.slice_params();
        let geometry_changed = held
            .as_ref()
            .is_some_and(|held| self.document.slice_geometry_changed(held.geometry));
        // A profile edited in place keeps its id: read what it resolves to now.
        let layers_changed = held.is_some()
            && layers_changed(
                self.document.slice_layer_values(),
                current_layer_values(&current),
            );
        match decide_re_slice(held.as_ref(), &current, geometry_changed, layers_changed) {
            ReSliceDecision::FirstSlice => self.launch(current, by_fix_all),
            // Fills to add or drop: the scene is the same, the slice is not.
            ReSliceDecision::Unchanged if self.review.fills_pending() => {
                self.launch(current, by_fix_all)
            }
            ReSliceDecision::Unchanged => {
                // The held slice already matches the inputs; nothing to
                // recompute. Surface the transient "up to date" hint.
                self.show_up_to_date_hint();
                false
            }
            ReSliceDecision::Changed if by_fix_all => self.launch(current, true),
            ReSliceDecision::Changed => {
                let this = self.clone();
                self.dialogs.show_slice_overwrite_confirm(move || {
                    this.launch(current, false);
                });
                false
            }
        }
    }

    /// Clones the scene, resets the modal readout, captures the run's inputs,
    /// and spawns the worker. Reached for a first slice, or once a re-slice
    /// over stale data is confirmed. The scene clone and the geometry marker
    /// are taken together here so the marker recorded with the result matches
    /// the geometry actually sliced.
    /// Returns whether the slice started.
    fn launch(self: &Rc<Self>, mut current: SliceParams, by_fix_all: bool) -> bool {
        let Some(ui) = self.ui.upgrade() else {
            return false;
        };
        let sessions = self.document.support_sessions();
        let Some((scene, supports)) = self.document.with_scene(|spatial| {
            (
                spatial.graph().clone(),
                support_persist::support_owners(spatial, &sessions),
            )
        }) else {
            return false;
        };
        // A real slice supersedes the "up to date" hint.
        self.hide_up_to_date_hint();
        current.geometry = self.document.scene_revision();

        self.progress.completed.store(0, Ordering::Relaxed);
        self.progress.total.store(0, Ordering::Relaxed);
        self.progress.reviewing.store(false, Ordering::Relaxed);
        ui.set_slice_reviewing(false);
        ui.set_slice_completed(0);
        ui.set_slice_total(0);
        ui.set_slice_progress(0.0);
        ui.set_slicing_in_progress(true);

        let cancel = CancelToken::new();
        *self.cancel.borrow_mut() = Some(cancel.clone());
        *self.pending_params.borrow_mut() = Some(current.clone());
        self.review.slice_started(by_fix_all);
        let patches = self.review.fills_for_slice();
        *self.pending_fills.borrow_mut() = patches.ids;

        self.start_poll();
        // The raft the scene's supports stand on, for the review (#131).
        let raft_top_mm = self.review.raft_top_mm();
        self.spawn(
            scene,
            SliceInputs {
                printer: current.printer,
                resin: current.resin,
                first_layer_height: current.first_layer_height,
                aa_level: current.aa_level,
                raft_top_mm,
                supports,
                fills: patches.fills,
                erasures: patches.erasures,
            },
            cancel,
        );
        true
    }

    /// Spawns the worker: run the projection + slicing, then deposit the result
    /// and wake the UI thread. `catch_unwind` converts a panic into a deposited
    /// error so the modal always closes.
    fn spawn(&self, scene: Scene, inputs: SliceInputs, cancel: CancelToken) {
        let slot = self.slot.clone();
        let progress = self.progress.clone();
        let ui = self.ui.clone();
        crate::breadcrumbs::started("slice");
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_slice(&scene, &inputs, &cancel, &progress)
            }))
            .unwrap_or_else(|payload| Err(SliceFailure::Panicked(panic_message(&payload))));
            *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(result);
            let _ = ui.upgrade_in_event_loop(|ui| ui.invoke_slice_finished());
        });
    }

    /// Starts the modal's progress poll. Stops itself once the window's
    /// `slicing-in-progress` flips false (the finished handler), mirroring the
    /// load-progress poll.
    fn start_poll(&self) {
        let progress = self.progress.clone();
        let ui_weak = self.ui.clone();
        let timer_weak = Rc::downgrade(&self.poll_timer);
        self.poll_timer.start(
            slint::TimerMode::Repeated,
            PROGRESS_POLL_INTERVAL,
            move || {
                let Some(ui) = ui_weak.upgrade() else {
                    if let Some(timer) = timer_weak.upgrade() {
                        timer.stop();
                    }
                    return;
                };
                if !ui.get_slicing_in_progress() {
                    if let Some(timer) = timer_weak.upgrade() {
                        timer.stop();
                    }
                    return;
                }
                let completed = progress.completed.load(Ordering::Relaxed);
                let total = progress.total.load(Ordering::Relaxed);
                ui.set_slice_reviewing(progress.reviewing.load(Ordering::Relaxed));
                ui.set_slice_completed(completed as i32);
                ui.set_slice_total(total as i32);
                ui.set_slice_progress(if total == 0 {
                    0.0
                } else {
                    completed as f32 / total as f32
                });
            },
        );
    }

    /// Requests cancellation of the in-flight slice, if any. The worker
    /// observes the token between layers and returns a cancelled outcome.
    fn cancel(&self) {
        if let Some(token) = self.cancel.borrow().as_ref() {
            token.cancel();
        }
    }

    /// Raises the "already up to date" hint and schedules its dismissal.
    /// Restarting the single-shot timer keeps the hint up for the full delay
    /// when the user presses Slice again while it is still showing.
    fn show_up_to_date_hint(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        ui.set_slice_up_to_date(true);
        let ui_weak = self.ui.clone();
        self.tooltip_timer.start(
            slint::TimerMode::SingleShot,
            UP_TO_DATE_HINT_DURATION,
            move || {
                if let Some(ui) = ui_weak.upgrade() {
                    ui.set_slice_up_to_date(false);
                }
            },
        );
    }

    /// Lowers the "already up to date" hint at once and cancels its pending
    /// dismissal.
    fn hide_up_to_date_hint(&self) {
        self.tooltip_timer.stop();
        if let Some(ui) = self.ui.upgrade() {
            ui.set_slice_up_to_date(false);
        }
    }

    /// Drains the deposited result on the UI thread: closes the modal, and
    /// either reports a completed slice, silently discards a cancelled one, or
    /// surfaces the failure.
    fn finish(&self) {
        let taken = self
            .slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        *self.cancel.borrow_mut() = None;
        let pending = self.pending_params.borrow_mut().take();
        let fill_ids = std::mem::take(&mut *self.pending_fills.borrow_mut());
        if let Some(ui) = self.ui.upgrade() {
            ui.set_slicing_in_progress(false);
        }
        let Some(result) = taken else {
            return;
        };
        let ended = match &result {
            Ok(SliceRunOutcome::Completed(completed)) => format!(
                "ok, {} layers",
                completed.contours.schedule().total_layers()
            ),
            Ok(SliceRunOutcome::Cancelled) => "cancelled".to_owned(),
            Err(err) => format!("failed: {err}"),
        };
        crate::breadcrumbs::finished("slice", &ended);
        match result {
            Ok(SliceRunOutcome::Completed(completed)) => {
                if let Some(params) = pending {
                    let layer_count = completed.contours.schedule().total_layers();
                    let replaced = self.document.set_slice_result(SliceResult {
                        contours: completed.contours,
                        token: completed.token,
                        params,
                        layers: completed.layers,
                    });
                    // A Fix All run keeps the slice of before its press, for
                    // Cancel (#163).
                    self.review.slice_replaced(replaced);
                    // Size and enable the height slider, parking the selected
                    // layer at the top.
                    if let Some(ui) = self.ui.upgrade() {
                        ui.set_slice_layer_count(layer_count as i32);
                        ui.set_selected_layer_index(layer_count.saturating_sub(1) as f32);
                    }
                    // After the slice is stored: the panel ties the findings
                    // to the slice now held, which cured these fills.
                    self.review.fills_landed(fill_ids);
                    self.review.show(completed.review);
                }
            }
            Ok(SliceRunOutcome::Cancelled) => {
                // A cancelled run carries no data; any prior result stays.
                // A Fix All run whose slice this was is cancelled with it
                // (#163).
                self.review.slice_ended_without_result(true);
            }
            Err(err) => {
                self.review.slice_ended_without_result(false);
                // The objects a failure names, as the object list names them
                // now.
                let names = self
                    .document
                    .with_scene(|spatial| object_names_by_leaf(spatial.graph()))
                    .unwrap_or_default();
                self.dialogs
                    .show_error(move || slice_failure_texts(&err, &names));
            }
        }
    }
}

/// The texts of the dialog that refuses a slice with no printer or no resin.
fn missing_selection_texts() -> ErrorTexts {
    ErrorTexts {
        heading: ctr!("ErrorDialog", "Cannot slice"),
        context: None,
        message: ctr!(
            "ErrorDialog",
            "Choose a printer and a resin before slicing."
        ),
        details: Default::default(),
    }
}

/// The texts of the dialog that reports a failed slice run. `names` holds the
/// object-list name of each object, for a failure that names objects.
fn slice_failure_texts(failure: &SliceFailure, names: &HashMap<NodeId, String>) -> ErrorTexts {
    if let SliceFailure::Slice(SliceSceneError::LeavesValidationFailed(failures)) = failure
        && !failures.is_empty()
    {
        return invalid_objects_texts(failures, names, failure);
    }
    ErrorTexts {
        heading: ctr!("ErrorDialog", "Slicing failed"),
        context: None,
        message: failure.to_string().into(),
        details: Default::default(),
    }
}

/// The texts for objects whose meshes the slicer refused: which objects, and
/// what to do. The slicer's own words go in the details.
fn invalid_objects_texts(
    failures: &[LeafValidationFailure],
    names: &HashMap<NodeId, String>,
    failure: &SliceFailure,
) -> ErrorTexts {
    let name = |f: &LeafValidationFailure| {
        names
            .get(&f.leaf)
            .cloned()
            .or_else(|| f.leaf_name.clone())
            .unwrap_or_else(|| ctr!("ErrorDialog", "Object {}", f.leaf.get()).to_string())
    };
    // Repair cannot give a vertex a coordinate: such a model has to be
    // exported again.
    let bad_numbers =
        |f: &&LeafValidationFailure| matches!(f.geometry, InvalidLeafGeometry::Vertices(_));
    let (heading, message) = match failures {
        [only] if bad_numbers(&only) => (
            ctr!("ErrorDialog", "Can't slice “{}”", name(only)),
            ctr!(
                "ErrorDialog",
                "“{}” has vertices whose coordinates are not numbers. Repair can't fix that: export the model again from its 3D app and open it again.",
                name(only)
            ),
        ),
        [only] => (
            ctr!("ErrorDialog", "Can't slice “{}”", name(only)),
            ctr!(
                "ErrorDialog",
                "“{}” is not a closed mesh: it has holes, cracks or faces that don't line up. Press Repair meshes in the Transform tab, then slice again.",
                name(only)
            ),
        ),
        _ => {
            let listed: Vec<String> = failures.iter().map(name).collect();
            let message = if failures.iter().all(|f| bad_numbers(&f)) {
                ctr!(
                    "ErrorDialog",
                    "These objects have vertices whose coordinates are not numbers: {}. Repair can't fix that: export them again from their 3D app.",
                    listed.join(", ")
                )
            } else {
                ctr!(
                    "ErrorDialog",
                    "These objects are not closed meshes: {}. Press Repair meshes in the Transform tab, then slice again.",
                    listed.join(", ")
                )
            };
            (
                ctr!(
                    "ErrorDialog",
                    "Can't slice {n} object",
                    "Can't slice {n} objects",
                    failures.len()
                ),
                message,
            )
        }
    };
    ErrorTexts {
        heading,
        context: None,
        message,
        details: failure.to_string().into(),
    }
}

/// What a run slices with, besides the scene.
struct SliceInputs {
    printer: PrinterId,
    resin: MaterialId,
    first_layer_height: Option<PositiveFiniteMm>,
    aa_level: AaLevel,
    /// The top of the raft the scene's supports stand on, for the review
    /// (#131).
    raft_top_mm: Option<f64>,
    /// The support leaves and the model each stands on, which the scale
    /// compensation scales them with (#227).
    supports: SupportOwners,
    /// The cavities to fill (#132) and the thin features to thicken (#156).
    fills: Vec<Arc<CavityFill>>,
    /// The specks to erase (#156).
    erasures: Vec<Arc<CavityFill>>,
}

/// Loads the profiles and runs the shared slicing service, publishing progress
/// into `progress` as layers complete, then cuts the fills on the run's
/// schedule and reviews the per-leaf stacks with them before merging them
/// all. A cancel that arrives during the review still cancels the run.
fn run_slice(
    scene: &Scene,
    inputs: &SliceInputs,
    cancel: &CancelToken,
    progress: &SliceProgressCell,
) -> Result<SliceRunOutcome, SliceFailure> {
    let (printer_profile, resin_profile) = load_selected_profiles(&inputs.printer, &inputs.resin)?;
    let layers = LayerValues::resolve(&printer_profile, &resin_profile, inputs.first_layer_height);
    let params = SlicingParams {
        layer_height_request: None,
        first_layer_height: inputs.first_layer_height,
        aa_level: inputs.aa_level,
    };
    let run = slice_plate(
        scene,
        &printer_profile,
        &resin_profile,
        &params,
        &inputs.supports,
        cancel,
        |p: SliceProgress| progress.record(p),
    )
    .map_err(SliceFailure::from)?;
    match run {
        sc_print_glue::SliceRun::Cancelled => Ok(SliceRunOutcome::Cancelled),
        sc_print_glue::SliceRun::Completed(plate) => {
            progress.reviewing.store(true, Ordering::Relaxed);
            let filled = fill_stack(inputs.fills.iter().map(|fill| &**fill), plate.schedule());
            let erased = erased_stack(
                plate.leaves(),
                filled.as_ref(),
                inputs.erasures.iter().map(|erasure| &**erasure),
                plate.schedule(),
            );
            let review = review_slices(
                plate.leaves(),
                filled.as_ref(),
                erased.as_ref(),
                plate.profile(),
                // The raft is scaled in Z with its supports.
                inputs
                    .raft_top_mm
                    .map(|top| plate.compensated_height_mm(top)),
            );
            crate::breadcrumbs::note(&format!(
                "slice: reviewed ok={}",
                matches!(review, ReviewOutcome::Reviewed { .. })
            ));
            if cancel.is_cancelled() {
                return Ok(SliceRunOutcome::Cancelled);
            }
            let scale = plate.scale_compensation();
            let contours = plate
                .merge_with(filled.into_iter().chain(erased))
                .map_err(|_| SliceFailure::SliceMerge)?;
            capture(scene, contours, scale, review, layers)
                .map(|completed| SliceRunOutcome::Completed(Box::new(completed)))
        }
    }
}

/// Fingerprints the slice inputs off the event loop, with the scale
/// compensation the leaves were cut under, and wraps the merged plate contours
/// and their review for the UI hand-off. The review's outcome never decides
/// whether the slice completes.
fn capture(
    scene: &Scene,
    contours: PlateContours,
    scale: ScaleCompensation,
    review: ReviewOutcome,
    layers: Option<LayerValues>,
) -> Result<CompletedSlice, SliceFailure> {
    let token = slice_input_fingerprint(scene, contours.schedule(), scale)
        .map_err(SliceFailure::Fingerprint)?
        .to_token();
    Ok(CompletedSlice {
        contours,
        token,
        review,
        layers,
    })
}

#[cfg(test)]
mod capture_tests {
    use super::*;

    use sc_sl1cer::contour::{ClosedRing, ContourLayer, ContourStack, Polygon};
    use sc_sl1cer::geom2d::Point2D;
    use sc_sl1cer::profile::LayerSchedule;

    use sc_print_primitives::ScaleCompensationPercent;

    use crate::test_support::scenes::cube_scene;

    const NONE: ScaleCompensation = ScaleCompensation::NONE;

    fn schedule() -> LayerSchedule {
        LayerSchedule::uniform(0.05, 0.05, 2).expect("a positive, non-empty schedule")
    }

    /// Builds a CCW triangle as 1 solid layer plus an empty layer, standing in
    /// for a merged plate stack.
    fn triangle_stack() -> ContourStack {
        let outer = ClosedRing::new(
            [(0.0, 0.0), (10.0, 0.0), (0.0, 10.0)]
                .into_iter()
                .map(|(x, y)| Point2D::new(x, y))
                .collect(),
        );
        let triangle = Polygon::new(outer, vec![]).expect("the outer ring is CCW");
        let solid = ContourLayer::new(0.025, vec![triangle]);
        let void = ContourLayer::new(0.075, vec![]);
        ContourStack::new(vec![solid, void]).expect("z is strictly increasing")
    }

    fn contours() -> PlateContours {
        PlateContours::new(triangle_stack(), schedule())
            .expect("2 layers pair with a 2-layer schedule")
    }

    fn clean_review() -> ReviewOutcome {
        ReviewOutcome::Reviewed {
            findings: Vec::new(),
            fills: Vec::new(),
            erasures: Vec::new(),
            thickenings: Vec::new(),
            took: Duration::ZERO,
        }
    }

    #[test]
    fn capture_carries_the_stack_and_fingerprints_the_scene() {
        let scene = cube_scene();
        let completed = capture(&scene, contours(), NONE, clean_review(), None).expect("capture");
        assert_eq!(completed.contours.stack().layers().len(), 2);
        assert_eq!(completed.contours.schedule(), &schedule());
        assert_eq!(
            completed.token,
            slice_input_fingerprint(&scene, &schedule(), NONE)
                .expect("the cube fingerprints")
                .to_token(),
        );
    }

    #[test]
    fn capture_fingerprints_the_scale_the_leaves_were_cut_under() {
        // A reopened project compares the token against the compensation the
        // profiles hold then (#295), so the token must carry the run's own.
        let scene = cube_scene();
        let percent = |value| ScaleCompensationPercent::new(value).expect("within +-10 %");
        let scale = ScaleCompensation {
            x: percent(0.5),
            y: percent(0.5),
            z: percent(0.0),
        };
        let completed = capture(&scene, contours(), scale, clean_review(), None).expect("capture");
        assert_eq!(
            completed.token,
            slice_input_fingerprint(&scene, &schedule(), scale)
                .expect("the cube fingerprints")
                .to_token(),
        );
        assert_ne!(
            completed.token,
            capture(&scene, contours(), NONE, clean_review(), None)
                .expect("a capture without compensation")
                .token,
        );
    }

    #[test]
    fn capture_is_deterministic() {
        let scene = cube_scene();
        let first = capture(&scene, contours(), NONE, clean_review(), None).expect("first capture");
        let second =
            capture(&scene, contours(), NONE, clean_review(), None).expect("second capture");
        assert_eq!(first.token, second.token);
    }

    #[test]
    fn capture_fails_when_the_scene_has_no_geometry() {
        // An empty scene has no sliceable leaf, so the fingerprint, and thus
        // the capture, cannot be computed.
        let result = capture(&Scene::new(), contours(), NONE, clean_review(), None);
        assert!(matches!(result, Err(SliceFailure::Fingerprint(_))));
    }

    #[test]
    fn a_failed_review_still_completes_the_slice() {
        let scene = cube_scene();
        let failed = ReviewOutcome::Failed("the checks fell over".to_owned());
        let completed =
            capture(&scene, contours(), NONE, failed.clone(), None).expect("the slice lands");
        assert_eq!(completed.contours.stack().layers().len(), 2);
        assert_eq!(completed.review, failed);
        assert_eq!(
            completed.token,
            capture(&scene, contours(), NONE, clean_review(), None)
                .expect("a reviewed capture")
                .token,
        );
    }
}

#[cfg(test)]
mod dialog_text_tests {
    use super::*;

    #[test]
    fn a_missing_selection_asks_for_a_printer_and_a_resin() {
        let texts = missing_selection_texts();
        assert_eq!(texts.heading, "Cannot slice");
        assert_eq!(texts.context, None);
        assert_eq!(
            texts.message,
            "Choose a printer and a resin before slicing."
        );
        assert_eq!(texts.details, "");
    }

    #[test]
    fn a_failed_run_shows_its_failure() {
        let texts = slice_failure_texts(&SliceFailure::EmptySceneGeometry, &HashMap::new());
        assert_eq!(texts.heading, "Slicing failed");
        assert_eq!(texts.context, None);
        assert_eq!(texts.message, "The scene has no geometry to slice.");
        assert_eq!(texts.details, "");
    }

    /// A scene of 1 open triangle named `name`, and the failure slicing it
    /// gives.
    fn open_object(name: &str) -> (Scene, SliceFailure) {
        use sc_mesh_core::IndexedMesh;
        use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
        use sc_mesh_core::transform::Transform3D;
        use sc_scene::{LeafPayload, SceneMetadata};

        let mut graph = Scene::new();
        let root = graph.add_group(None, Transform3D::Identity);
        graph.add_root(root).unwrap();
        let mesh = IndexedMesh {
            meta: None,
            vertices: [[0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [0.0, 1.0, 2.0]]
                .into_iter()
                .map(Vertex::new)
                .collect(),
            faces: vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
        };
        let leaf = graph
            .add_leaf(
                Some(SceneMetadata {
                    name: Some(name.to_string()),
                    comment: None,
                }),
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .unwrap();
        graph.add_child(root, leaf).unwrap();
        let failure = SliceFailure::Slice(SliceSceneError::LeavesValidationFailed(vec![
            LeafValidationFailure {
                leaf,
                leaf_name: None,
                geometry: InvalidLeafGeometry::Manifold(Vec::new()),
            },
        ]));
        (graph, failure)
    }

    #[test]
    fn a_refused_object_is_named_with_what_to_do() {
        let (graph, failure) = open_object("Wolf");
        let texts = slice_failure_texts(&failure, &object_names_by_leaf(&graph));
        assert_eq!(texts.heading, "Can't slice “Wolf”");
        assert_eq!(
            texts.message,
            "“Wolf” is not a closed mesh: it has holes, cracks or faces that don't line up. Press Repair meshes in the Transform tab, then slice again."
        );
        // The slicer's own words stay in the details.
        assert!(
            texts.details.contains("mesh validation failed"),
            "{}",
            texts.details
        );
    }

    #[test]
    fn several_refused_objects_are_listed() {
        let (graph, failure) = open_object("Wolf");
        let SliceFailure::Slice(SliceSceneError::LeavesValidationFailed(mut failures)) = failure
        else {
            unreachable!()
        };
        let leaf = failures[0].leaf;
        failures.push(LeafValidationFailure {
            leaf,
            leaf_name: None,
            geometry: InvalidLeafGeometry::Vertices(Vec::new()),
        });
        let failure = SliceFailure::Slice(SliceSceneError::LeavesValidationFailed(failures));
        let texts = slice_failure_texts(&failure, &object_names_by_leaf(&graph));
        assert_eq!(texts.heading, "Can't slice 2 objects");
        assert_eq!(
            texts.message,
            "These objects are not closed meshes: Wolf, Wolf. Press Repair meshes in the Transform tab, then slice again."
        );
    }
}

#[cfg(test)]
mod progress_tests {
    use super::*;

    use sc_mesh_core::transform::Transform3D;
    use sc_mesh_fixtures::unit_cube_mesh;
    use sc_scene::LeafPayload;

    use crate::test_support::scenes::cube_scene;

    /// Builds a plate of 4 unit cubes, each its own leaf, as a model with
    /// its supports and raft slices as several leaves.
    fn four_leaf_scene() -> Scene {
        let mut scene = cube_scene();
        for _ in 0..3 {
            let leaf = scene
                .add_leaf(
                    None,
                    Transform3D::Identity,
                    LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
                )
                .expect("inline-mesh leaf");
            scene.add_root(leaf).expect("fresh leaf becomes a root");
        }
        scene
    }

    fn inputs() -> SliceInputs {
        SliceInputs {
            printer: PrinterId::new("elegoo-saturn-4-ultra").expect("valid id"),
            resin: MaterialId::new("standard-405").expect("valid id"),
            first_layer_height: None,
            aa_level: AaLevel::Off,
            raft_top_mm: None,
            supports: SupportOwners::new(),
            fills: Vec::new(),
            erasures: Vec::new(),
        }
    }

    /// The modal counts the plate's layers, the count the Result card shows,
    /// not one per leaf and layer (#355), and it ends full.
    #[test]
    fn progress_counts_the_plate_layers_and_ends_full() {
        let progress = SliceProgressCell::default();
        let outcome = run_slice(
            &four_leaf_scene(),
            &inputs(),
            &CancelToken::new(),
            &progress,
        )
        .expect("the cubes slice");
        let SliceRunOutcome::Completed(completed) = outcome else {
            panic!("not cancelled");
        };
        let layers = completed.contours.schedule().total_layers();
        assert!(layers > 1, "layers: {layers}");
        assert_eq!(progress.total.load(Ordering::Relaxed), layers);
        assert_eq!(progress.completed.load(Ordering::Relaxed), layers);
    }

    /// A cancelled run still stops cleanly and never counts past the plate.
    #[test]
    fn a_cancelled_run_stays_within_the_plate_layers() {
        let progress = SliceProgressCell::default();
        let cancel = CancelToken::new();
        cancel.cancel();
        let outcome = run_slice(&four_leaf_scene(), &inputs(), &cancel, &progress)
            .expect("cancellation is a clean stop");
        assert!(matches!(outcome, SliceRunOutcome::Cancelled));
        assert!(
            progress.completed.load(Ordering::Relaxed) <= progress.total.load(Ordering::Relaxed)
        );
    }

    /// Reports arrive out of order from the layer tasks: the published count
    /// never steps back, and it is in plate layers.
    #[test]
    fn record_publishes_plate_layers_and_never_steps_back() {
        let cell = SliceProgressCell::default();
        let report = |completed| SliceProgress {
            completed,
            total: 2800,
            layers: 700,
        };
        cell.record(report(1400));
        cell.record(report(8));
        assert_eq!(cell.completed.load(Ordering::Relaxed), 350);
        assert_eq!(cell.total.load(Ordering::Relaxed), 700);
        cell.record(report(2800));
        assert_eq!(cell.completed.load(Ordering::Relaxed), 700);
    }
}
