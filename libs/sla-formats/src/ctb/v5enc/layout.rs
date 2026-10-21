/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Where each section of the file begins.
//!
//! The plain revision's header, print parameters, slicer info and v4
//! block are replaced here by one encrypted settings block, and a
//! second indirection is added: a table of layer pointers that the
//! layer records sit behind. Everything after the settings block is
//! in the clear, laid out with no alignment and reached only through
//! addresses recorded before it.
//!
//! The file ends with 3 sections this cannot place, because their
//! addresses follow from the compressed size of the layers: the
//! marker, the signature and the trailer.

use crate::ctb::disclaimer::DISCLAIMER_SIZE;

/// Bytes from the start of the file to the settings block.
pub const HEADER_SIZE: u32 = 48;

/// 18 cipher blocks, which is what makes the block a whole number of
/// them.
pub const SETTINGS_SIZE: u32 = 288;

/// Width, height, and the offset and length of one preview image.
/// Half the plain revision's record, which spends the difference on
/// reserved words.
pub const PREVIEW_RECORD_SIZE: u32 = 16;

/// One entry of the table that locates a layer's record.
pub const LAYER_POINTER_SIZE: u32 = 16;

/// The record before a layer's own image: 4 more bytes than the
/// plain revision's, spent on the range of the image that carries a
/// second pass of encryption.
pub const LAYER_RECORD_SIZE: u32 = 88;

/// A quantity nobody has identified, written after the last layer.
pub const POST_LAYER_MARKER_SIZE: u32 = 8;

/// The encrypted digest that closes the file, before the trailer.
pub const SIGNATURE_SIZE: u32 = 32;

/// The checksum over everything before it.
pub const TRAILER_SIZE: u32 = 4;

/// The lengths the format leaves to the writer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VariableSections {
    pub large_preview_bytes: u32,
    pub small_preview_bytes: u32,
    /// The name's own bytes. Sections abut and a string carries no
    /// terminator, so this is exactly the length the settings block
    /// records.
    pub machine_name_bytes: u32,
    pub resin_parameters_bytes: u32,
    pub layer_count: u32,
}

/// Where every section of an encrypted v5 file begins, as absolute
/// file offsets, and the lengths it was measured from. The settings
/// block records both a section's address and its length from here,
/// so the two cannot come from different measurements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub variable: VariableSections,
    pub settings: u32,
    pub large_preview_record: u32,
    pub large_preview_image: u32,
    pub small_preview_record: u32,
    pub small_preview_image: u32,
    pub machine_name: u32,
    pub disclaimer: u32,
    pub resin_parameters: u32,
    pub layer_pointer_table: u32,
    /// First byte past the pointer table, where the first layer's
    /// own record starts.
    pub layer_data: u32,
}

impl Layout {
    /// Measure the section chain from the lengths only the writer
    /// knows.
    pub fn plan(variable: VariableSections) -> Self {
        let settings = HEADER_SIZE;

        let large_preview_record = settings + SETTINGS_SIZE;
        let large_preview_image = large_preview_record + PREVIEW_RECORD_SIZE;

        let small_preview_record = large_preview_image + variable.large_preview_bytes;
        let small_preview_image = small_preview_record + PREVIEW_RECORD_SIZE;

        let machine_name = small_preview_image + variable.small_preview_bytes;
        let disclaimer = machine_name + variable.machine_name_bytes;
        let resin_parameters = disclaimer + DISCLAIMER_SIZE;
        let layer_pointer_table = resin_parameters + variable.resin_parameters_bytes;
        let layer_data = layer_pointer_table + LAYER_POINTER_SIZE * variable.layer_count;

        Self {
            variable,
            settings,
            large_preview_record,
            large_preview_image,
            small_preview_record,
            small_preview_image,
            machine_name,
            disclaimer,
            resin_parameters,
            layer_pointer_table,
            layer_data,
        }
    }
}
