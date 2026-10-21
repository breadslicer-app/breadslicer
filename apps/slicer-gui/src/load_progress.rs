/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Worker-side load-progress accounting.
//!
//! The load pipeline runs 3 phases on the worker thread (parse,
//! spatial priming, GPU upload), each with its own unit of measure.
//! Progress is published through one shared `AtomicU32` cell packing
//! `(phase, permille)` with the phase in the high bits, so a phase
//! advance always outranks any within-phase value and `fetch_max`
//! keeps the published value monotonic across the whole load: a
//! straggler tick from an earlier phase can never regress it.
//!
//! [`LoadProgressSink`] is the write side, handed to the worker at
//! dispatch. The UI side polls the cell and decodes it with
//! [`decode`].

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// Phases of the load pipeline, in execution order. The discriminant
/// is the packed cell's high half, so declaration order is display
/// order and later phases compare greater.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LoadPhase {
    Parse = 0,
    Priming = 1,
    GpuUpload = 2,
}

/// Upper bound of the within-phase progress scale.
pub(crate) const PROGRESS_PERMILLE_MAX: u32 = 1000;

/// Packs a phase and a within-phase permille into one cell value.
/// Ordering property: for any permilles `a`, `b` in range, a later
/// phase packed with `a` compares greater than an earlier phase
/// packed with `b`.
fn pack(phase: LoadPhase, permille: u32) -> u32 {
    ((phase as u32) << 16) | permille.min(PROGRESS_PERMILLE_MAX)
}

/// Decodes a cell value back into `(phase, permille)`. Values only
/// ever come from [`pack`]; an out-of-range phase half (impossible
/// through the sink) saturates to the last phase.
pub(crate) fn decode(value: u32) -> (LoadPhase, u32) {
    let permille = value & 0xFFFF;
    let phase = match value >> 16 {
        0 => LoadPhase::Parse,
        1 => LoadPhase::Priming,
        _ => LoadPhase::GpuUpload,
    };
    (phase, permille)
}

/// Within-phase fraction on the permille scale. A zero denominator
/// counts as complete (a phase with nothing to do is done), and
/// `done` past `total` clamps to the ceiling (a forged 3MF size
/// claim can over-run its announced total). Shared with the
/// save-progress accounting.
pub(crate) fn permille(done: u64, total: u64) -> u32 {
    if total == 0 {
        return PROGRESS_PERMILLE_MAX;
    }
    let done = done.min(total);
    ((u128::from(done) * u128::from(PROGRESS_PERMILLE_MAX)) / u128::from(total)) as u32
}

/// Write side of the shared progress cell. Cheap to clone; clones
/// share the cell. The worker calls [`LoadProgressSink::set`] as its
/// phases advance; [`LoadProgressSink::reset`] rearms the cell at
/// dispatch, before the worker spawns, so a new load never briefly
/// shows the previous load's final value.
#[derive(Clone)]
pub(crate) struct LoadProgressSink {
    cell: Arc<AtomicU32>,
}

impl LoadProgressSink {
    pub(crate) fn new(cell: Arc<AtomicU32>) -> Self {
        Self { cell }
    }

    /// Rearm the cell to the start of the parse phase.
    pub(crate) fn reset(&self) {
        self.cell.store(0, Ordering::Relaxed);
    }

    /// Record `done` of `total` units inside `phase`. `fetch_max`
    /// keeps the published value monotonic; see the module doc.
    pub(crate) fn set(&self, phase: LoadPhase, done: u64, total: u64) {
        self.cell
            .fetch_max(pack(phase, permille(done, total)), Ordering::Relaxed);
    }
}

#[cfg(test)]
mod packing_tests {
    use super::*;

    #[test]
    fn permille_scales_and_clamps() {
        assert_eq!(permille(0, 1000), 0);
        assert_eq!(permille(500, 1000), 500);
        assert_eq!(permille(1000, 1000), PROGRESS_PERMILLE_MAX);
        assert_eq!(
            permille(2000, 1000),
            PROGRESS_PERMILLE_MAX,
            "over-run clamps"
        );
        assert_eq!(permille(1, 3), 333, "fractions truncate");
    }

    #[test]
    fn zero_total_counts_as_complete() {
        assert_eq!(permille(0, 0), PROGRESS_PERMILLE_MAX);
    }

    #[test]
    fn later_phases_outrank_any_earlier_permille() {
        assert!(pack(LoadPhase::Priming, 0) > pack(LoadPhase::Parse, PROGRESS_PERMILLE_MAX));
        assert!(pack(LoadPhase::GpuUpload, 0) > pack(LoadPhase::Priming, PROGRESS_PERMILLE_MAX));
    }

    #[test]
    fn decode_round_trips_every_phase() {
        for phase in [LoadPhase::Parse, LoadPhase::Priming, LoadPhase::GpuUpload] {
            for permille in [0, 1, 500, PROGRESS_PERMILLE_MAX] {
                assert_eq!(decode(pack(phase, permille)), (phase, permille));
            }
        }
    }
}

#[cfg(test)]
mod sink_tests {
    use super::*;

    fn sink_and_cell() -> (LoadProgressSink, Arc<AtomicU32>) {
        let cell = Arc::new(AtomicU32::new(0));
        (LoadProgressSink::new(cell.clone()), cell)
    }

    fn published(cell: &AtomicU32) -> (LoadPhase, u32) {
        decode(cell.load(Ordering::Relaxed))
    }

    #[test]
    fn set_publishes_phase_and_permille() {
        let (sink, cell) = sink_and_cell();
        sink.set(LoadPhase::Parse, 250, 1000);
        assert_eq!(published(&cell), (LoadPhase::Parse, 250));
    }

    #[test]
    fn within_phase_regressions_are_absorbed() {
        let (sink, cell) = sink_and_cell();
        sink.set(LoadPhase::Parse, 900, 1000);
        sink.set(LoadPhase::Parse, 100, 1000);
        assert_eq!(published(&cell), (LoadPhase::Parse, 900));
    }

    #[test]
    fn phase_advance_outranks_a_high_earlier_permille() {
        let (sink, cell) = sink_and_cell();
        sink.set(LoadPhase::Parse, 999, 1000);
        sink.set(LoadPhase::Priming, 0, 100);
        assert_eq!(published(&cell), (LoadPhase::Priming, 0));
    }

    #[test]
    fn reset_rearms_for_the_next_load() {
        let (sink, cell) = sink_and_cell();
        sink.set(LoadPhase::GpuUpload, 1, 1);
        sink.reset();
        assert_eq!(published(&cell), (LoadPhase::Parse, 0));
    }
}
