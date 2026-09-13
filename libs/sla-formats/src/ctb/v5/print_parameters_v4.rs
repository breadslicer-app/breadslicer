/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The 464-byte block format version 4 added: 21 words of content
//! and 380 reserved bytes.
//!
//! It completes what the earlier blocks leave out, the bottom
//! layer's retract and the second stage of it, and it is where the
//! disclaimer and the resin parameters are located from.

use std::io::{self, Write};

use super::layout::{DISCLAIMER_SIZE, Layout};
use crate::ctb::wire::{write_f32, write_u32, write_zeros};
use crate::layer_values::LayerValues;
use crate::print_program::PrintProgram;

/// Two slots no reader identifies. Every file the vendor's own
/// slicer writes carries 4.0 in both, and nothing in such a file
/// correlates with them.
const UNIDENTIFIED: f32 = 4.0;

const RESERVED_BYTES: usize = 380;

/// Write the v4 print parameters for `program`, recording where
/// `layout` put the disclaimer and the resin parameters.
///
/// # Errors
///
/// Propagates `out`'s failures.
pub fn write_print_parameters_v4(
    program: &PrintProgram,
    layout: &Layout,
    out: &mut impl Write,
) -> io::Result<()> {
    let bottom = LayerValues::of(program.layers.first());
    let normal = LayerValues::of(program.layers.last());

    write_f32(out, bottom.retract_speed_mm_min)?;
    write_f32(out, bottom.second_retract_speed_mm_min)?;
    write_u32(out, 0)?;
    write_f32(out, UNIDENTIFIED)?;
    write_u32(out, 0)?;
    write_f32(out, UNIDENTIFIED)?;
    write_f32(out, normal.wait_after_retract_sec)?;
    write_f32(out, normal.wait_after_lift_sec)?;
    write_f32(out, normal.wait_before_lift_sec)?;
    write_f32(out, bottom.second_retract_distance_mm)?;
    write_f32(out, still_time_sec(program))?;
    write_u32(out, 0)?;
    write_u32(out, 0)?;
    write_u32(out, layer_count(program))?;
    write_zeros(out, 16)?;
    write_u32(out, layout.disclaimer)?;
    write_u32(out, DISCLAIMER_SIZE)?;
    write_u32(out, layout.resin_parameters)?;
    write_zeros(out, RESERVED_BYTES)
}

/// Every second of the print the platform spends still: the
/// exposures and the waits, with none of the travel.
fn still_time_sec(program: &PrintProgram) -> f32 {
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

fn layer_count(program: &PrintProgram) -> u32 {
    u32::try_from(program.layers.len()).unwrap_or(u32::MAX)
}
