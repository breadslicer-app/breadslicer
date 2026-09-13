/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Where each section of the file begins.
//!
//! Sections abut with no alignment and are reached only through
//! offsets recorded in the sections before them, so the header cannot
//! be written until the whole chain has been measured. Four lengths
//! are not fixed by the format: the two compressed previews, the
//! machine name and the resin parameters. Everything else is a
//! constant here, and each block's own tests pin what it writes
//! against the constant it claims.

use crate::ctb::disclaimer::DISCLAIMER_SIZE;

/// Bytes from the start of the file to the first preview record.
pub const HEADER_SIZE: u32 = 112;
/// Width, height, and the offset and length of one preview image.
pub const PREVIEW_RECORD_SIZE: u32 = 32;
pub const PRINT_PARAMETERS_SIZE: u32 = 60;
pub const SLICER_INFO_SIZE: u32 = 76;
pub const PRINT_PARAMETERS_V4_SIZE: u32 = 464;
/// One entry of the table the machine reads to find a layer.
pub const LAYER_TABLE_RECORD_SIZE: u32 = 36;

/// The fuller record repeated immediately before a layer's own
/// image: the table's 36 bytes again, then 48 more of motion.
pub const LAYER_RECORD_SIZE: u32 = 84;

/// The lengths the format leaves to the writer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VariableSections {
    pub large_preview_bytes: u32,
    pub small_preview_bytes: u32,
    /// The name's own bytes. Sections abut and a string carries no
    /// terminator, so this is exactly the length the slicer info
    /// records.
    pub machine_name_bytes: u32,
    pub resin_parameters_bytes: u32,
    pub layer_count: u32,
}

/// Where every section of a plain v5 file begins, as absolute file
/// offsets, and the lengths it was measured from. Blocks that record
/// a section's length as well as its address read both from here, so
/// the two cannot come from different measurements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub variable: VariableSections,
    pub large_preview_record: u32,
    pub large_preview_image: u32,
    pub small_preview_record: u32,
    pub small_preview_image: u32,
    pub print_parameters: u32,
    pub slicer_info: u32,
    pub machine_name: u32,
    pub disclaimer: u32,
    pub print_parameters_v4: u32,
    pub resin_parameters: u32,
    pub layer_table: u32,
    /// First byte past the layer table, where the first layer's own
    /// record starts.
    pub layer_data: u32,
}

impl Layout {
    /// Measure the whole section chain from the lengths only the
    /// writer knows.
    pub fn plan(variable: VariableSections) -> Self {
        let large_preview_record = HEADER_SIZE;
        let large_preview_image = large_preview_record + PREVIEW_RECORD_SIZE;

        let small_preview_record = large_preview_image + variable.large_preview_bytes;
        let small_preview_image = small_preview_record + PREVIEW_RECORD_SIZE;

        let print_parameters = small_preview_image + variable.small_preview_bytes;
        let slicer_info = print_parameters + PRINT_PARAMETERS_SIZE;
        let machine_name = slicer_info + SLICER_INFO_SIZE;
        let disclaimer = machine_name + variable.machine_name_bytes;
        let print_parameters_v4 = disclaimer + DISCLAIMER_SIZE;
        let resin_parameters = print_parameters_v4 + PRINT_PARAMETERS_V4_SIZE;
        let layer_table = resin_parameters + variable.resin_parameters_bytes;
        let layer_data = layer_table + LAYER_TABLE_RECORD_SIZE * variable.layer_count;

        Self {
            variable,
            large_preview_record,
            large_preview_image,
            small_preview_record,
            small_preview_image,
            print_parameters,
            slicer_info,
            machine_name,
            disclaimer,
            print_parameters_v4,
            resin_parameters,
            layer_table,
            layer_data,
        }
    }
}
