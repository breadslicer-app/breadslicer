/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Per-window driver for the floating save-progress panel.
//!
//! Owns the shared progress cell and the UI-side poll timer, mirroring
//! how [`crate::scene_loader::SceneLoader`] drives the load panel.
//! [`begin`](SaveProgressPanel::begin) shows the panel and starts
//! polling at save dispatch; the worker publishes through
//! [`sink`](SaveProgressPanel::sink);
//! [`end`](SaveProgressPanel::end) hides the panel, which stops the poll
//! on its next tick. Kept out of [`crate::save_progress`] so that module
//! stays framework-free.

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use crate::MainWindow;
use crate::load_progress::PROGRESS_PERMILLE_MAX;
use crate::save_progress::{SavePhase, SaveProgressSink, decode};

/// Cadence of the UI-side poll while a save is in flight.
const PROGRESS_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Maps a worker save phase to the matching `SavePhase` in the Slint
/// `main-window.slint` declaration. The Rust and Slint enums share a
/// name; the caller refers to the Rust one here.
fn ui_save_phase(phase: SavePhase) -> crate::SavePhase {
    match phase {
        SavePhase::Serialize => crate::SavePhase::Serializing,
        SavePhase::Compress => crate::SavePhase::Compressing,
        SavePhase::Writing => crate::SavePhase::Writing,
    }
}

pub(crate) struct SaveProgressPanel {
    ui: slint::Weak<MainWindow>,
    cell: Arc<AtomicU32>,
    timer: Rc<slint::Timer>,
}

impl SaveProgressPanel {
    pub(crate) fn new(ui: slint::Weak<MainWindow>) -> Self {
        Self {
            ui,
            cell: Arc::new(AtomicU32::new(0)),
            timer: Rc::new(slint::Timer::default()),
        }
    }

    /// Sink handle for the shared progress cell, handed to the worker at
    /// dispatch time.
    pub(crate) fn sink(&self) -> SaveProgressSink {
        SaveProgressSink::new(self.cell.clone())
    }

    /// Rearm the cell, show the panel, and start the poll timer. Called
    /// right before the write worker spawns, so the panel never shows
    /// while the file dialog is open. Headless callers (unit tests) have
    /// no window: the cell still rearms and the UI side is skipped.
    ///
    /// The poll stops itself: [`end`](SaveProgressPanel::end) flips the
    /// window's `saving-in-progress` to false, and the next tick observes
    /// that and calls `Timer::stop` through the `Weak` (no timer/closure
    /// `Rc` cycle).
    pub(crate) fn begin(&self) {
        self.sink().reset();
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        ui.set_save_phase(ui_save_phase(SavePhase::Serialize));
        ui.set_save_progress(0.0);
        ui.set_saving_in_progress(true);
        let cell = self.cell.clone();
        let ui_weak = self.ui.clone();
        let timer_weak = Rc::downgrade(&self.timer);
        self.timer.start(
            slint::TimerMode::Repeated,
            PROGRESS_POLL_INTERVAL,
            move || {
                let Some(ui) = ui_weak.upgrade() else {
                    if let Some(timer) = timer_weak.upgrade() {
                        timer.stop();
                    }
                    return;
                };
                if !ui.get_saving_in_progress() {
                    if let Some(timer) = timer_weak.upgrade() {
                        timer.stop();
                    }
                    return;
                }
                let (phase, permille) = decode(cell.load(Ordering::Relaxed));
                ui.set_save_phase(ui_save_phase(phase));
                ui.set_save_progress(permille as f32 / PROGRESS_PERMILLE_MAX as f32);
            },
        );
    }

    /// Hide the panel. The poll timer stops itself on its next tick.
    pub(crate) fn end(&self) {
        if let Some(ui) = self.ui.upgrade() {
            ui.set_saving_in_progress(false);
        }
    }
}
