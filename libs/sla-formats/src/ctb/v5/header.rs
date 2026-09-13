/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The 112-byte header the file opens with.
//!
//! It carries the plate geometry and a summary of the schedule, and
//! it is where the offsets of the preview records, the print
//! parameters, the slicer info and the layer table are recorded. The
//! machine follows the per-layer records rather than the summary here.

use std::io::{self, Write};

use super::layout::{Layout, PRINT_PARAMETERS_SIZE, SLICER_INFO_SIZE};
use crate::ctb::LIGHT_OFF_SEC;
use crate::ctb::wire::{write_f32, write_u16, write_u32};
use crate::layer_values::LayerValues;
use crate::print_program::PrintProgram;

/// Opens every plain file at format version 4 and 5. Earlier
/// revisions and the encrypted form each have their own.
const MAGIC: u32 = 0x12FD_0106;
const VERSION: u32 = 5;

/// CBDDLP reached anti-aliasing by repeating its whole layer table
/// once per level, and this counts those passes. CTB stores a grey
/// value per pixel instead, so it always has one.
const LEVEL_SET_COUNT: u32 = 1;

/// Zero disables the obfuscation of the layer images, which is what
/// lets a writer produce this format without the vendor's cipher.
const LAYER_XOR_KEY: u32 = 0;

/// Write the header for `program`, recording the offsets `layout`
/// measured.
///
/// # Errors
///
/// Propagates `out`'s failures, and rejects a program holding more
/// layers than the header's count can express.
pub fn write_header(
    program: &PrintProgram,
    layout: &Layout,
    out: &mut impl Write,
) -> io::Result<()> {
    let layer_count = u32::try_from(program.layers.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "more layers than a header can count",
        )
    })?;
    let raster = &program.raster;
    let plate = raster.build_volume();
    let bottom = LayerValues::of(program.layers.first());
    let normal = LayerValues::of(program.layers.last());
    let created_sec = u32::try_from(program.created_at.unix_sec).unwrap_or_default();

    write_u32(out, MAGIC)?;
    write_u32(out, VERSION)?;
    write_f32(out, plate.x_mm() as f32)?;
    write_f32(out, plate.y_mm() as f32)?;
    write_f32(out, plate.z_mm() as f32)?;
    write_u32(out, created_sec)?;
    write_u32(out, created_sec)?;
    write_f32(out, overall_height_mm(program))?;
    write_f32(out, program.layer_height_mm.value() as f32)?;
    write_f32(out, normal.exposure_sec)?;
    write_f32(out, bottom.exposure_sec)?;
    write_f32(out, LIGHT_OFF_SEC)?;
    write_u32(out, program.bottom_layer_count)?;
    write_u32(out, raster.pixel_resolution().x())?;
    write_u32(out, raster.pixel_resolution().y())?;
    write_u32(out, layout.large_preview_record)?;
    write_u32(out, layout.layer_table)?;
    write_u32(out, layer_count)?;
    write_u32(out, layout.small_preview_record)?;
    write_u32(out, program.print_time_sec.value() as u32)?;
    write_u32(out, u32::from(raster.mirror().x))?;
    write_u32(out, layout.print_parameters)?;
    write_u32(out, PRINT_PARAMETERS_SIZE)?;
    write_u32(out, LEVEL_SET_COUNT)?;
    write_u16(out, normal.light_pwm)?;
    write_u16(out, bottom.light_pwm)?;
    write_u32(out, LAYER_XOR_KEY)?;
    write_u32(out, layout.slicer_info)?;
    write_u32(out, SLICER_INFO_SIZE)
}

/// How far the platform travels over the whole print.
fn overall_height_mm(program: &PrintProgram) -> f32 {
    program
        .layers
        .last()
        .map_or(0.0, |layer| layer.slab_top_mm.value() as f32)
}
