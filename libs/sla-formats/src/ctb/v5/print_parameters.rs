/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The 60-byte block of travel, material and pause settings.
//!
//! Its lift heights are the whole travel rather than the first stage:
//! a reader takes the first stage as this total less the second
//! stage, which the slicer info holds. Writing a first stage here
//! would ask for a shorter move than the program describes.

use std::io::{self, Write};

use crate::ctb::wire::{write_f32, write_u32, write_zeros};
use crate::ctb::{LIGHT_OFF_SEC, MM3_PER_ML};
use crate::layer_values::LayerValues;
use crate::print_program::PrintProgram;

/// Nothing in this workspace prices a print, and the machine only
/// displays this.
const RESIN_COST: f32 = 0.0;

const PADDING_BYTES: usize = 16;

/// Write the print parameters for `program`.
///
/// # Errors
///
/// Propagates `out`'s failures.
pub fn write_print_parameters(program: &PrintProgram, out: &mut impl Write) -> io::Result<()> {
    let bottom = LayerValues::of(program.layers.first());
    let normal = LayerValues::of(program.layers.last());

    write_f32(out, lift_travel_mm(&bottom))?;
    write_f32(out, bottom.lift_speed_mm_min)?;
    write_f32(out, lift_travel_mm(&normal))?;
    write_f32(out, normal.lift_speed_mm_min)?;
    write_f32(out, normal.retract_speed_mm_min)?;
    write_f32(out, (program.volume_mm3.value() / MM3_PER_ML) as f32)?;
    write_f32(out, program.mass_g.value() as f32)?;
    write_f32(out, RESIN_COST)?;
    write_f32(out, LIGHT_OFF_SEC)?;
    write_f32(out, LIGHT_OFF_SEC)?;
    write_u32(out, program.bottom_layer_count)?;
    write_zeros(out, PADDING_BYTES)
}

/// How far the platform rises in total, both stages together.
fn lift_travel_mm(values: &LayerValues) -> f32 {
    values.lift_distance_mm + values.second_lift_distance_mm
}
