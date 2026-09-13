/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The 76-byte block holding the second travel stage and the file's
//! own metadata.
//!
//! The first stage lives in the print parameters, which stores the
//! two stages added together, so the values here are what a reader
//! subtracts from that total.

use std::io::{self, Write};

use super::layout::Layout;
use crate::ctb::wire::{write_f32, write_u8, write_u16, write_u32, write_zeros};
use crate::layer_values::LayerValues;
use crate::print_program::PrintProgram;

const ANTI_ALIAS_OFF: u8 = 0x07;
const ANTI_ALIAS_ON: u8 = 0x0F;

/// The high nibble carries the revision and the low one allows a
/// layer to override the header's settings, which every layer this
/// writer emits does.
const PER_LAYER_SETTINGS: u8 = 0x50;

/// What the vendor's own slicer reports at this revision, as
/// `0xMMmmpp00`.
const SOFTWARE_VERSION: u32 = 0x0200_0000;

const SEC_PER_MIN: u64 = 60;
const RESERVED_BYTES: usize = 8;

/// Write the slicer info for `program`, recording where `layout` put
/// the machine name and the v4 print parameters.
///
/// # Errors
///
/// Propagates `out`'s failures.
pub fn write_slicer_info(
    program: &PrintProgram,
    layout: &Layout,
    out: &mut impl Write,
) -> io::Result<()> {
    let bottom = LayerValues::of(program.layers.first());
    let normal = LayerValues::of(program.layers.last());
    let aa = program.raster.aa();

    write_f32(out, bottom.second_lift_distance_mm)?;
    write_f32(out, bottom.second_lift_speed_mm_min)?;
    write_f32(out, normal.second_lift_distance_mm)?;
    write_f32(out, normal.second_lift_speed_mm_min)?;
    write_f32(out, normal.second_retract_distance_mm)?;
    write_f32(out, normal.second_retract_speed_mm_min)?;
    write_f32(out, normal.wait_after_lift_sec)?;
    write_u32(out, layout.machine_name)?;
    write_u32(out, layout.variable.machine_name_bytes)?;
    write_u8(out, anti_alias_flag(aa.factor()))?;
    write_u16(out, 0)?;
    write_u8(out, PER_LAYER_SETTINGS)?;
    write_u32(out, modified_minutes(program))?;
    write_u32(out, u32::from(aa.factor()))?;
    write_u32(out, SOFTWARE_VERSION)?;
    write_f32(out, normal.wait_after_retract_sec)?;
    // The same wait as offset 24; both reference writers fill both.
    write_f32(out, normal.wait_after_lift_sec)?;
    write_u32(out, program.transition_layer_count)?;
    write_u32(out, layout.print_parameters_v4)?;
    write_zeros(out, RESERVED_BYTES)
}

fn anti_alias_flag(factor: u8) -> u8 {
    if factor > 1 {
        ANTI_ALIAS_ON
    } else {
        ANTI_ALIAS_OFF
    }
}

/// When the file was written, in the minutes this block counts in
/// rather than the seconds the header does.
fn modified_minutes(program: &PrintProgram) -> u32 {
    u32::try_from(program.created_at.unix_sec / SEC_PER_MIN).unwrap_or_default()
}
