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
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use sc_print_glue::{PlateContours, SlicingParams, slice_input_fingerprint, slice_plate};
use sc_print_primitives::{AaLevel, MaterialId, PositiveFiniteMm, PrinterId};
use sc_review::{CavityFill, fill_stack};
use sc_scene::Scene;
use sc_sl1cer::{CancelToken, SliceProgress};
use slint::ComponentHandle;

use crate::MainWindow;
use crate::error_dialog::{DialogManager, ErrorTexts};
use crate::i18n::ctr;
use crate::mesh_loader::panic_message;
use crate::preferences_bus::PreferencesBus;
use crate::print_selection::PrintDefaults;
use crate::review::{ReviewOutcome, erased_stack, review_slices};
use crate::review_panel::ReviewPanel;
use crate::scene_document::SceneDocument;
use crate::slice_projection::{SliceFailure, load_selected_profiles};
use crate::slice_result::{ReSliceDecision, SliceParams, SliceResult, decide_re_slice};

/// Cadence of the UI-side progress poll while a slice is in flight.
const PROGRESS_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long the "already up to date" hint stays up after an unchanged Slice
/// press before it fades back out.
const UP_TO_DATE_HINT_DURATION: Duration = Duration::from_secs(10);

/// Shared completed/total layer-slice counters: the worker writes them, the UI
/// polls them into the modal. `completed` climbs monotonically (`fetch_max`);
/// `total` is the run's fixed denominator. `reviewing` is raised once every
/// layer is cut, while the review runs.
#[derive(Default)]
struct SliceProgressCell {
    completed: AtomicU32,
    total: AtomicU32,
    reviewing: AtomicBool,
}

/// A completed slice, already merged, fingerprinted and reviewed on the worker
/// thread so the event loop never does that work.
struct CompletedSlice {
    contours: PlateContours,
    token: String,
    review: ReviewOutcome,
}

/// The worker's finished payload, past the engine's raw per-leaf output: a
/// completed, merged, fingerprinted slice, or a cancellation.
enum SliceRunOutcome {
    Completed(CompletedSlice),
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
            run.review.set_reslice(move || {
                if let Some(run) = weak.upgrade() {
                    run.start_with(true);
                }
            });
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
    fn start_with(self: &Rc<Self>, by_fix_all: bool) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if ui.get_slicing_in_progress() {
            return;
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
            return;
        };

        let held = self.document.slice_params();
        let geometry_changed = held
            .as_ref()
            .is_some_and(|held| self.document.slice_geometry_changed(held.geometry));
        match decide_re_slice(held.as_ref(), &current, geometry_changed) {
            ReSliceDecision::FirstSlice => self.launch(current, by_fix_all),
            // Fills to add or drop: the scene is the same, the slice is not.
            ReSliceDecision::Unchanged if self.review.fills_pending() => {
                self.launch(current, by_fix_all)
            }
            ReSliceDecision::Unchanged => {
                // The held slice already matches the inputs; nothing to
                // recompute. Surface the transient "up to date" hint.
                self.show_up_to_date_hint();
            }
            ReSliceDecision::Changed if by_fix_all => self.launch(current, true),
            ReSliceDecision::Changed => {
                let this = self.clone();
                self.dialogs.show_slice_overwrite_confirm(move || {
                    this.launch(current, false);
                });
            }
        }
    }

    /// Clones the scene, resets the modal readout, captures the run's inputs,
    /// and spawns the worker. Reached for a first slice, or once a re-slice
    /// over stale data is confirmed. The scene clone and the geometry marker
    /// are taken together here so the marker recorded with the result matches
    /// the geometry actually sliced.
    fn launch(self: &Rc<Self>, mut current: SliceParams, by_fix_all: bool) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let Some(scene) = self.document.with_scene(|spatial| spatial.graph().clone()) else {
            return;
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
                fills: patches.fills,
                erasures: patches.erasures,
            },
            cancel,
        );
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
                    self.document.set_slice_result(SliceResult {
                        contours: completed.contours,
                        token: completed.token,
                        params,
                    });
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
            }
            Err(err) => {
                self.dialogs.show_error(move || slice_failure_texts(&err));
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

/// The texts of the dialog that reports a failed slice run.
fn slice_failure_texts(failure: &SliceFailure) -> ErrorTexts {
    ErrorTexts {
        heading: ctr!("ErrorDialog", "Slicing failed"),
        context: None,
        message: failure.to_string().into(),
        details: Default::default(),
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
        cancel,
        |p: SliceProgress| {
            progress
                .completed
                .fetch_max(p.completed as u32, Ordering::Relaxed);
            progress.total.store(p.total as u32, Ordering::Relaxed);
        },
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
                inputs.raft_top_mm,
            );
            crate::breadcrumbs::note(&format!(
                "slice: reviewed ok={}",
                matches!(review, ReviewOutcome::Reviewed { .. })
            ));
            if cancel.is_cancelled() {
                return Ok(SliceRunOutcome::Cancelled);
            }
            let contours = plate
                .merge_with(filled.into_iter().chain(erased))
                .map_err(|_| SliceFailure::SliceMerge)?;
            capture(scene, contours, review).map(SliceRunOutcome::Completed)
        }
    }
}

/// Fingerprints the slice inputs off the event loop and wraps the merged plate
/// contours and their review for the UI hand-off. The review's outcome never
/// decides whether the slice completes.
fn capture(
    scene: &Scene,
    contours: PlateContours,
    review: ReviewOutcome,
) -> Result<CompletedSlice, SliceFailure> {
    let token = slice_input_fingerprint(scene, contours.schedule())
        .map_err(SliceFailure::Fingerprint)?
        .to_token();
    Ok(CompletedSlice {
        contours,
        token,
        review,
    })
}

#[cfg(test)]
mod capture_tests {
    use super::*;

    use sc_sl1cer::contour::{ClosedRing, ContourLayer, ContourStack, Polygon};
    use sc_sl1cer::geom2d::Point2D;
    use sc_sl1cer::profile::LayerSchedule;

    use crate::test_support::scenes::cube_scene;

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
        let completed = capture(&scene, contours(), clean_review()).expect("capture");
        assert_eq!(completed.contours.stack().layers().len(), 2);
        assert_eq!(completed.contours.schedule(), &schedule());
        assert_eq!(
            completed.token,
            slice_input_fingerprint(&scene, &schedule())
                .expect("the cube fingerprints")
                .to_token(),
        );
    }

    #[test]
    fn capture_is_deterministic() {
        let scene = cube_scene();
        let first = capture(&scene, contours(), clean_review()).expect("first capture");
        let second = capture(&scene, contours(), clean_review()).expect("second capture");
        assert_eq!(first.token, second.token);
    }

    #[test]
    fn capture_fails_when_the_scene_has_no_geometry() {
        // An empty scene has no sliceable leaf, so the fingerprint, and thus
        // the capture, cannot be computed.
        let result = capture(&Scene::new(), contours(), clean_review());
        assert!(matches!(result, Err(SliceFailure::Fingerprint(_))));
    }

    #[test]
    fn a_failed_review_still_completes_the_slice() {
        let scene = cube_scene();
        let failed = ReviewOutcome::Failed("the checks fell over".to_owned());
        let completed = capture(&scene, contours(), failed.clone()).expect("the slice lands");
        assert_eq!(completed.contours.stack().layers().len(), 2);
        assert_eq!(completed.review, failed);
        assert_eq!(
            completed.token,
            capture(&scene, contours(), clean_review())
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
        let texts = slice_failure_texts(&SliceFailure::EmptySceneGeometry);
        assert_eq!(texts.heading, "Slicing failed");
        assert_eq!(texts.context, None);
        assert_eq!(texts.message, "The scene has no geometry to slice.");
        assert_eq!(texts.details, "");
    }
}
