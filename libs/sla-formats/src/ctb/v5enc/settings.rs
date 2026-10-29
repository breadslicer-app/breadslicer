/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The 288 bytes that replace the plain revision's 4 fixed blocks.
//!
//! One flat run of fields with no slack, holding the plate geometry,
//! the schedule summary, the material, the addresses of every
//! section after it, and the key its layer images are obfuscated
//! with. It is handed back rather than written, because it reaches
//! the file encrypted as one piece.

use std::io::{self, Cursor};
use std::num::NonZeroU32;

use super::layout::{Layout, SETTINGS_SIZE};
use crate::ctb::disclaimer::DISCLAIMER_SIZE;
use crate::ctb::totals::{overall_height_mm, still_time_sec};
use crate::ctb::wire::{write_f32, write_u8, write_u16, write_u32, write_u64, write_zeros};
use crate::ctb::{LIGHT_OFF_SEC, MM3_PER_ML};
use crate::layer_values::LayerValues;
use crate::print_program::PrintProgram;

/// The block's length as the cipher needs it, a whole number of
/// 16-byte blocks.
pub const SETTINGS_BYTES: usize = SETTINGS_SIZE as usize;

const ANTI_ALIAS_OFF: u8 = 0x07;
const ANTI_ALIAS_ON: u8 = 0x0F;

/// Lets a layer override the summary in this block, which every
/// layer this writer emits does. The plain revision spells the same
/// permission `0x50`.
const PER_LAYER_SETTINGS: u8 = 0x40;

/// CBDDLP reached anti-aliasing by repeating its whole layer table
/// once per level, and this counts those passes. CTB stores a grey
/// value per pixel instead, so it always has one.
const LEVEL_SET_COUNT: u32 = 1;

/// Nothing in this workspace prices a print, and the machine only
/// displays this.
const RESIN_COST: f32 = 0.0;

/// Slots no reader identifies, written as the vendor's own slicer
/// writes them.
const UNIDENTIFIED_FOUR: f32 = 4.0;
const UNIDENTIFIED_ONE: u32 = 1;

const SEC_PER_MIN: u64 = 60;

/// Build the settings block for `program`, stating the addresses
/// `layout` measured, the key its layer images are obfuscated with,
/// and the value the file's signature is taken over.
///
/// # Errors
///
/// Propagates the failures of filling the block, which a block of
/// this size does not produce.
pub fn settings_block(
    program: &PrintProgram,
    layout: &Layout,
    layer_key: NonZeroU32,
    signed_value: u64,
) -> io::Result<[u8; SETTINGS_BYTES]> {
    let raster = &program.raster;
    let plate = raster.build_volume();
    let resolution = raster.panel_resolution();
    let (panel_x_mm, panel_y_mm) = raster.panel_size_mm();
    let bottom = LayerValues::of(program.layers.first());
    let normal = LayerValues::of(program.layers.last());
    let aa = raster.aa();
    let layer_count = layer_count(program);

    let out = &mut Cursor::new([0; SETTINGS_BYTES]);

    write_u64(out, signed_value)?;
    write_u32(out, layout.layer_pointer_table)?;
    write_f32(out, panel_x_mm as f32)?;
    write_f32(out, panel_y_mm as f32)?;
    write_f32(out, plate.z_mm() as f32)?;
    write_zeros(out, 8)?;
    write_f32(out, overall_height_mm(program))?;
    write_f32(out, program.layer_height_mm.value() as f32)?;
    write_f32(out, normal.exposure_sec)?;
    write_f32(out, bottom.exposure_sec)?;
    write_f32(out, LIGHT_OFF_SEC)?;
    write_u32(out, program.bottom_layer_count)?;
    write_u32(out, resolution.x())?;
    write_u32(out, resolution.y())?;
    write_u32(out, layer_count)?;
    write_u32(out, layout.large_preview_record)?;
    write_u32(out, layout.small_preview_record)?;
    write_u32(out, program.print_time_sec.value() as u32)?;
    write_u32(out, u32::from(raster.mirror().x))?;
    write_f32(out, lift_travel_mm(&bottom))?;
    write_f32(out, bottom.lift_speed_mm_min)?;
    write_f32(out, lift_travel_mm(&normal))?;
    write_f32(out, normal.lift_speed_mm_min)?;
    write_f32(out, normal.retract_speed_mm_min)?;
    write_f32(out, (program.volume_mm3.value() / MM3_PER_ML) as f32)?;
    write_f32(out, program.mass_g.value() as f32)?;
    write_f32(out, RESIN_COST)?;
    write_f32(out, LIGHT_OFF_SEC)?;
    write_u32(out, LEVEL_SET_COUNT)?;
    write_u16(out, normal.light_pwm)?;
    write_u16(out, bottom.light_pwm)?;
    write_u32(out, layer_key.get())?;
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
    write_f32(out, normal.wait_after_retract_sec)?;
    write_f32(out, normal.wait_after_lift_sec)?;
    write_u32(out, program.transition_layer_count)?;
    write_f32(out, bottom.retract_speed_mm_min)?;
    write_f32(out, bottom.second_retract_speed_mm_min)?;
    write_u32(out, 0)?;
    write_f32(out, UNIDENTIFIED_FOUR)?;
    write_u32(out, 0)?;
    write_f32(out, UNIDENTIFIED_FOUR)?;
    write_f32(out, normal.wait_after_retract_sec)?;
    write_f32(out, normal.wait_after_lift_sec)?;
    write_f32(out, normal.wait_before_lift_sec)?;
    write_f32(out, bottom.second_retract_distance_mm)?;
    write_f32(out, still_time_sec(program))?;
    // Both real files carry a number near 25,400 in the first of
    // these 2 slots and zero in the second. Nothing in either file
    // correlates with the first, so it goes out unset rather than
    // guessed at.
    write_zeros(out, 8)?;
    write_u32(out, layer_count)?;
    write_zeros(out, 16)?;
    write_u32(out, layout.disclaimer)?;
    write_u32(out, DISCLAIMER_SIZE)?;
    write_u32(out, UNIDENTIFIED_ONE)?;
    write_u32(out, layout.resin_parameters)?;
    write_zeros(out, 8)?;

    Ok(*out.get_ref())
}

/// How far the platform rises in total, both stages together, which
/// is what this block records and the second stage is taken from.
fn lift_travel_mm(values: &LayerValues) -> f32 {
    values.lift_distance_mm + values.second_lift_distance_mm
}

fn anti_alias_flag(factor: u8) -> u8 {
    if factor > 1 {
        ANTI_ALIAS_ON
    } else {
        ANTI_ALIAS_OFF
    }
}

/// When the file was written, in the minutes this block counts in
/// rather than the seconds the plain revision's header does.
fn modified_minutes(program: &PrintProgram) -> u32 {
    u32::try_from(program.created_at.unix_sec / SEC_PER_MIN).unwrap_or_default()
}

fn layer_count(program: &PrintProgram) -> u32 {
    u32::try_from(program.layers.len()).unwrap_or(u32::MAX)
}
