/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The 2 records a layer gets.
//!
//! The machine reads the layer table, a run of 36-byte records the
//! header locates. The same 36 bytes are repeated immediately before
//! the layer's own image, followed by 48 more that give the layer its
//! own motion, and the image begins after that. So every layer states
//! its whole schedule rather than deferring to the header.

use std::io::{self, Write};

use super::layout::LAYER_RECORD_SIZE;
use crate::ctb::LIGHT_OFF_SEC;
use crate::ctb::wire::{write_f32, write_u32, write_zeros};
use crate::layer_values::LayerValues;
use crate::print_program::LayerProgram;

const RESERVED_BYTES: usize = 8;

/// Where a layer's image sits and how long it is.
///
/// Addresses are 32-bit and these files may pass 4 GiB, so a record
/// stores a page number beside an offset inside that page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageLocation {
    page: u32,
    offset: u32,
    length: u32,
}

impl ImageLocation {
    /// Split an absolute file address into the page and offset a
    /// record stores.
    pub fn new(address: u64, length: u32) -> Self {
        Self {
            page: (address >> u32::BITS) as u32,
            offset: address as u32,
            length,
        }
    }
}

/// Write the 36-byte record the layer table holds for `layer`.
///
/// # Errors
///
/// Propagates `out`'s failures.
pub fn write_table_record(
    layer: &LayerProgram,
    image: ImageLocation,
    out: &mut impl Write,
) -> io::Result<()> {
    let values = LayerValues::of(Some(layer));

    write_f32(out, layer.slab_top_mm.value() as f32)?;
    write_f32(out, values.exposure_sec)?;
    write_f32(out, LIGHT_OFF_SEC)?;
    write_u32(out, image.offset)?;
    write_u32(out, image.length)?;
    write_u32(out, image.page)?;
    write_u32(out, LAYER_RECORD_SIZE)?;
    write_zeros(out, RESERVED_BYTES)
}

/// Write the 84-byte record that precedes `layer`'s own image: the
/// table's 36 bytes again, then the motion this layer runs under.
///
/// # Errors
///
/// Propagates `out`'s failures.
pub fn write_layer_record(
    layer: &LayerProgram,
    image: ImageLocation,
    out: &mut impl Write,
) -> io::Result<()> {
    write_table_record(layer, image, out)?;

    let values = LayerValues::of(Some(layer));

    write_u32(out, LAYER_RECORD_SIZE + image.length)?;
    write_f32(
        out,
        values.lift_distance_mm + values.second_lift_distance_mm,
    )?;
    write_f32(out, values.lift_speed_mm_min)?;
    write_f32(out, values.second_lift_distance_mm)?;
    write_f32(out, values.second_lift_speed_mm_min)?;
    write_f32(out, values.retract_speed_mm_min)?;
    write_f32(out, values.second_retract_distance_mm)?;
    write_f32(out, values.second_retract_speed_mm_min)?;
    write_f32(out, values.wait_before_lift_sec)?;
    write_f32(out, values.wait_after_lift_sec)?;
    write_f32(out, values.wait_after_retract_sec)?;
    write_f32(out, f32::from(values.light_pwm))
}
