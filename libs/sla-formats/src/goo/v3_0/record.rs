/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! One layer's record: the 70 fixed bytes, its image, and the
//! delimiter that closes it.
//!
//! The firmware follows these records rather than the header's copy
//! of the same settings, so every layer states its own exposure,
//! motion and light in full.

use std::io::{self, Write};

use super::wire::{write_f32, write_u16, write_u32};
use crate::layer_values::LayerValues;
use crate::print_program::LayerProgram;

const DELIMITER: [u8; 2] = [0x0D, 0x0A];

/// Fixed part of a layer record, before its image.
pub const RECORD_SIZE: usize = 70;

/// Read only under the light-off delay mode, which this writer does
/// not select.
const LIGHT_OFF_SEC: f32 = 0.0;
const PAUSE_AT_THIS_LAYER: u16 = 0;

/// Write one layer: its record, the image bytes, and a delimiter.
///
/// `park_z_mm` is where the platform would go if the layer carried a
/// pause, which none written here does.
///
/// # Errors
///
/// Propagates `out`'s failures, and rejects an image longer than the
/// record's length field can count.
pub fn write_layer(
    layer: &LayerProgram,
    park_z_mm: f32,
    image: &[u8],
    out: &mut impl Write,
) -> io::Result<()> {
    let image_length = u32::try_from(image.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "layer image longer than its length field can count",
        )
    })?;
    let values = LayerValues::of(Some(layer));

    write_u16(out, PAUSE_AT_THIS_LAYER)?;
    write_f32(out, park_z_mm)?;
    write_f32(out, layer.slab_top_mm.value() as f32)?;
    write_f32(out, values.exposure_sec)?;
    write_f32(out, LIGHT_OFF_SEC)?;
    write_f32(out, values.wait_before_lift_sec)?;
    write_f32(out, values.wait_after_lift_sec)?;
    write_f32(out, values.wait_after_retract_sec)?;
    write_f32(out, values.lift_distance_mm)?;
    write_f32(out, values.lift_speed_mm_min)?;
    write_f32(out, values.second_lift_distance_mm)?;
    write_f32(out, values.second_lift_speed_mm_min)?;
    write_f32(out, values.retract_distance_mm)?;
    write_f32(out, values.retract_speed_mm_min)?;
    write_f32(out, values.second_retract_distance_mm)?;
    write_f32(out, values.second_retract_speed_mm_min)?;
    write_u16(out, values.light_pwm)?;
    out.write_all(&DELIMITER)?;
    write_u32(out, image_length)?;
    out.write_all(image)?;
    out.write_all(&DELIMITER)
}
