/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Quantities about the whole print that a block reports and the
//! layers decide.
//!
//! Every revision carries both, and a machine follows its per-layer
//! records rather than these, so they are what the display shows
//! before the print starts.

use crate::print_program::PrintProgram;

/// How far the platform travels over the whole print.
pub fn overall_height_mm(program: &PrintProgram) -> f32 {
    program
        .layers
        .last()
        .map_or(0.0, |layer| layer.slab_top_mm.value() as f32)
}

/// Every second of the print the platform spends still: the
/// exposures and the waits, with none of the travel.
pub fn still_time_sec(program: &PrintProgram) -> f32 {
    program
        .layers
        .iter()
        .map(|layer| {
            layer.exposure_sec.value()
                + layer.wait_before_cure_sec.value()
                + layer.wait_after_cure_sec.value()
                + layer.wait_after_lift_sec.value()
        })
        .sum::<f64>() as f32
}
