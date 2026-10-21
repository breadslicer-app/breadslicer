/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Per-window driver for the floating export-progress panel.
//!
//! Owns the shared progress cell and the UI-side poll timer, mirroring
//! [`crate::save_progress_panel::SaveProgressPanel`]. Export and save
//! share one busy flag (`saving-in-progress`) and one panel: the two
//! flows are mutually exclusive (each disables the other's menu items
//! while it runs), so the panel only ever shows one of them.
//! [`begin`](ExportProgressPanel::begin) shows the panel and starts
//! polling at export dispatch; the worker publishes through
//! [`sink`](ExportProgressPanel::sink);
//! [`end`](ExportProgressPanel::end) hides the panel, which stops the
//! poll on its next tick. Kept out of [`crate::export_progress`] so
//! that module stays framework-free.

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use crate::MainWindow;
use crate::export_progress::{ExportPhase, ExportProgressSink, decode};
use crate::load_progress::PROGRESS_PERMILLE_MAX;

/// Cadence of the UI-side poll while an export is in flight.
const PROGRESS_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Maps a worker export phase to the `SavePhase` value the shared
/// panel in `main-window.slint` displays.
fn ui_export_phase(phase: ExportPhase) -> crate::SavePhase {
    match phase {
        ExportPhase::Flatten => crate::SavePhase::Flattening,
        ExportPhase::Serialize => crate::SavePhase::Serializing,
        ExportPhase::Compress => crate::SavePhase::Compressing,
        ExportPhase::Write => crate::SavePhase::Writing,
    }
}

pub(crate) struct ExportProgressPanel {
    ui: slint::Weak<MainWindow>,
    cell: Arc<AtomicU32>,
    timer: Rc<slint::Timer>,
}

impl ExportProgressPanel {
    pub(crate) fn new(ui: slint::Weak<MainWindow>) -> Self {
        Self {
            ui,
            cell: Arc::new(AtomicU32::new(0)),
            timer: Rc::new(slint::Timer::default()),
        }
    }

    /// Sink handle for the shared progress cell, handed to the worker at
    /// dispatch time.
    pub(crate) fn sink(&self) -> ExportProgressSink {
        ExportProgressSink::new(self.cell.clone())
    }

    /// Rearm the cell, show the panel, and start the poll timer. Called
    /// right before the write worker spawns, so the panel never shows
    /// while the file dialog is open. Headless callers (unit tests) have
    /// no window: the cell still rearms and the UI side is skipped.
    ///
    /// The poll stops itself: [`end`](ExportProgressPanel::end) flips
    /// the window's `saving-in-progress` to false, and the next tick
    /// observes that and calls `Timer::stop` through the `Weak` (no
    /// timer/closure `Rc` cycle).
    pub(crate) fn begin(&self) {
        self.sink().reset();
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        ui.set_save_phase(ui_export_phase(ExportPhase::Flatten));
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
                ui.set_save_phase(ui_export_phase(phase));
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
