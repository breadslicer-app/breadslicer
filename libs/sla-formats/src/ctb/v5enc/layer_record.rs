/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The 2 records a layer gets: a pointer, and the record it points
//! at.
//!
//! The pointer table is what the settings block locates, and each
//! entry gives the address of a layer's own record, which in turn
//! gives the address of its image. Where the plain revision repeats
//! the table entry in front of the image, this revision splits the
//! two apart, puts the page number before the data length rather
//! than after it, and adds the range of the image that carries a
//! second pass of encryption.

use std::io::{self, Write};

use super::layout::LAYER_RECORD_SIZE;
use crate::ctb::LIGHT_OFF_SEC;
use crate::ctb::page_address::PageAddress;
use crate::ctb::wire::{write_f32, write_u32};
use crate::layer_values::LayerValues;
use crate::print_program::LayerProgram;

/// The part of a layer's image that carries a second pass of
/// encryption, as an offset into the image and a length in bytes.
/// A layer with none declares a length of zero, which is what the
/// default is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EncryptedRange {
    pub offset: u32,
    pub length: u32,
}

/// Where a layer's image sits, how long it is, and which part of it
/// is encrypted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayerImage {
    address: PageAddress,
    length: u32,
    encrypted: EncryptedRange,
}

impl LayerImage {
    /// Locate an image of `length` bytes at an absolute file
    /// `address`, `encrypted` naming the part of it the cipher
    /// covers.
    pub fn new(address: u64, length: u32, encrypted: EncryptedRange) -> Self {
        Self {
            address: PageAddress::of(address),
            length,
            encrypted,
        }
    }
}

/// Write the pointer table's entry for a layer whose record sits at
/// `record_at`.
///
/// # Errors
///
/// Propagates `out`'s failures.
pub fn write_layer_pointer(record_at: u64, out: &mut impl Write) -> io::Result<()> {
    let address = PageAddress::of(record_at);

    write_u32(out, address.offset)?;
    write_u32(out, address.page)?;
    write_u32(out, LAYER_RECORD_SIZE)?;
    write_u32(out, 0)
}

/// Write the record that precedes `layer`'s own image, holding the
/// motion this layer runs under.
///
/// # Errors
///
/// Propagates `out`'s failures.
pub fn write_layer_record(
    layer: &LayerProgram,
    image: LayerImage,
    out: &mut impl Write,
) -> io::Result<()> {
    let values = LayerValues::of(Some(layer));

    write_u32(out, LAYER_RECORD_SIZE)?;
    write_f32(out, layer.slab_top_mm.value() as f32)?;
    write_f32(out, values.exposure_sec)?;
    write_f32(out, LIGHT_OFF_SEC)?;
    write_u32(out, image.address.offset)?;
    write_u32(out, image.address.page)?;
    write_u32(out, image.length)?;
    write_u32(out, 0)?;
    write_u32(out, image.encrypted.offset)?;
    write_u32(out, image.encrypted.length)?;
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
    write_f32(out, f32::from(values.light_pwm))?;
    write_u32(out, 0)
}
