/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Per-window driver for the Slice action.
//!
//! The Slice button snapshots the scene and the chosen inputs on the UI
//! thread, then runs the projection and the slicing engine on a worker
//! thread so the event loop stays responsive. Progress is published
//! through shared atomics that a `slint::Timer` polls into the modal;
//! the finished result is deposited into a slot and picked up on the UI
//! thread via `invoke_slice_finished`. A `CancelToken` lets the modal's
//! Cancel button stop the run between layers.
//!
//! The window's `slicing-in-progress` property is the single gate: the
//! button is disabled while it is set, and it is cleared on every exit
//! path (completion, cancellation, error, worker panic). The worker
//! wraps its body in `catch_unwind` so a panic still deposits a result
//! and the modal always closes.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use sc_mesh_core::scene::Scene;
use sc_print_primitives::{AaLevel, MaterialId, PositiveFiniteMm, PrinterId};
use sc_sl1cer::{CancelToken, SliceOutcome, SliceProgress, slice_scene_with_progress};
use slint::ComponentHandle;

use crate::MainWindow;
use crate::error_dialog::DialogManager;
use crate::mesh_loader::panic_message;
use crate::preferences_bus::PreferencesBus;
use crate::scene_document::SceneDocument;
use crate::slice_projection::{SliceFailure, build_slice_profile, load_selected_profiles};

/// Cadence of the UI-side progress poll while a slice is in flight.
const PROGRESS_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Shared completed/total layer-slice counters: the worker writes them,
/// the UI polls them into the modal. `completed` climbs monotonically
/// (`fetch_max`); `total` is the run's fixed denominator.
#[derive(Default)]
struct SliceProgressCell {
    completed: AtomicU32,
    total: AtomicU32,
}

type SliceSlot = Arc<Mutex<Option<Result<SliceOutcome, SliceFailure>>>>;

pub(crate) struct SliceRun {
    ui: slint::Weak<MainWindow>,
    bus: Rc<PreferencesBus>,
    document: Rc<SceneDocument>,
    dialogs: Rc<DialogManager>,
    progress: Arc<SliceProgressCell>,
    slot: SliceSlot,
    // The current run's cancel token, so the modal's Cancel can trip it.
    // `None` when no slice is in flight.
    cancel: RefCell<Option<CancelToken>>,
    // Poll timer driving the modal's progress readout. Behind an `Rc` so
    // its own callback can stop it through a `Weak` without a cycle.
    poll_timer: Rc<slint::Timer>,
}

impl SliceRun {
    /// Wires the Slice / Cancel / finished callbacks and returns the
    /// driver, which the caller keeps alive for the window's lifetime.
    pub(crate) fn install(
        ui: &MainWindow,
        bus: Rc<PreferencesBus>,
        document: Rc<SceneDocument>,
        dialogs: Rc<DialogManager>,
    ) -> Rc<Self> {
        let run = Rc::new(Self {
            ui: ui.as_weak(),
            bus,
            document,
            dialogs,
            progress: Arc::new(SliceProgressCell::default()),
            slot: Arc::new(Mutex::new(None)),
            cancel: RefCell::new(None),
            poll_timer: Rc::new(slint::Timer::default()),
        });
        {
            let run = run.clone();
            ui.on_slice_requested(move || run.start());
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

    /// Snapshots the inputs and dispatches a slice run. A no-op when a
    /// slice is already in flight, or when no printer/resin is chosen
    /// (the button gates both, but the checks keep the path safe).
    fn start(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if ui.get_slicing_in_progress() {
            return;
        }
        // The Slice panel prefills its pickers with the scene's own
        // choice, else the app-wide default (see `slice_selection`).
        // Slicing resolves the printer and resin the same way, so the
        // selection shown in the panel is the one that runs rather than
        // requiring an explicit re-pick of a default already on display.
        let snapshot = self.bus.snapshot();
        let printer = self
            .document
            .selected_printer()
            .or_else(|| snapshot.default_printer.clone());
        let resin = self
            .document
            .selected_resin()
            .or_else(|| snapshot.default_resin.clone());
        let (Some(printer), Some(resin)) = (printer, resin) else {
            self.dialogs.show_error(
                "Cannot slice",
                None,
                "Choose a printer and a resin before slicing.".to_string(),
                String::new(),
            );
            return;
        };
        let Some(scene) = self.document.with_scene(|spatial| spatial.graph().clone()) else {
            return;
        };
        let first_layer_height = self.document.selected_first_layer_height();
        let aa_level = snapshot.aa_level;

        self.progress.completed.store(0, Ordering::Relaxed);
        self.progress.total.store(0, Ordering::Relaxed);
        ui.set_slice_completed(0);
        ui.set_slice_total(0);
        ui.set_slice_progress(0.0);
        ui.set_slicing_in_progress(true);

        let cancel = CancelToken::new();
        *self.cancel.borrow_mut() = Some(cancel.clone());

        self.start_poll();
        self.spawn(scene, printer, resin, first_layer_height, aa_level, cancel);
    }

    /// Spawns the worker: run the projection + slicing, then deposit the
    /// result and wake the UI thread. `catch_unwind` converts a panic
    /// into a deposited error so the modal always closes.
    fn spawn(
        &self,
        scene: Scene,
        printer: PrinterId,
        resin: MaterialId,
        first_layer_height: Option<PositiveFiniteMm>,
        aa_level: AaLevel,
        cancel: CancelToken,
    ) {
        let slot = self.slot.clone();
        let progress = self.progress.clone();
        let ui = self.ui.clone();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_slice(
                    &scene,
                    &printer,
                    &resin,
                    first_layer_height,
                    aa_level,
                    &cancel,
                    &progress,
                )
            }))
            .unwrap_or_else(|payload| Err(SliceFailure::Panicked(panic_message(&payload))));
            *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(result);
            let _ = ui.upgrade_in_event_loop(|ui| ui.invoke_slice_finished());
        });
    }

    /// Starts the modal's progress poll. Stops itself once the window's
    /// `slicing-in-progress` flips false (the finished handler), mirroring
    /// the load-progress poll.
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

    /// Drains the deposited result on the UI thread: closes the modal,
    /// and either reports a completed slice, silently discards a
    /// cancelled one, or surfaces the failure.
    fn finish(&self) {
        let taken = self
            .slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        *self.cancel.borrow_mut() = None;
        if let Some(ui) = self.ui.upgrade() {
            ui.set_slicing_in_progress(false);
        }
        let Some(result) = taken else {
            return;
        };
        match result {
            Ok(SliceOutcome::Completed(slices)) => {
                let layers = slices.first().map_or(0, |leaf| leaf.stack().layers().len());
                eprintln!("Sliced {} model(s): {layers} layers", slices.len());
            }
            Ok(SliceOutcome::Cancelled) => {
                // A cancelled run carries no data; nothing to keep.
            }
            Err(err) => {
                self.dialogs
                    .show_error("Slicing failed", None, err.to_string(), String::new());
            }
        }
    }
}

/// Worker body: load the profiles, build the slice profile, and slice
/// the scene, publishing progress into `progress` as layers complete.
fn run_slice(
    scene: &Scene,
    printer: &PrinterId,
    resin: &MaterialId,
    first_layer_height: Option<PositiveFiniteMm>,
    aa_level: AaLevel,
    cancel: &CancelToken,
    progress: &SliceProgressCell,
) -> Result<SliceOutcome, SliceFailure> {
    let (printer_profile, resin_profile) = load_selected_profiles(printer, resin)?;
    let profile = build_slice_profile(
        &printer_profile,
        &resin_profile,
        first_layer_height,
        aa_level,
        scene,
    )?;
    slice_scene_with_progress(scene, &profile, cancel, |p: SliceProgress| {
        progress
            .completed
            .fetch_max(p.completed as u32, Ordering::Relaxed);
        progress.total.store(p.total as u32, Ordering::Relaxed);
    })
    .map_err(SliceFailure::Slice)
}
