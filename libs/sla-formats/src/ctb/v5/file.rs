/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The whole file: the fixed sections, the layer table, then a
//! record and an image per layer.
//!
//! Every entry in the layer table holds the address of an image that
//! has not been compressed yet, so the table is written last even
//! though it comes first. Its size is known from the layer count, so
//! the sink is left a gap of the right length and seeks back to fill
//! it once the layers are out. Layers are rasterised and compressed
//! one at a time, so the largest buffer alive is a single layer's
//! image beside the table.

use std::io::{Seek, SeekFrom, Write};

use sc_sl1cer::contour::ContourStack;

use super::header::write_header;
use super::layer_record::{ImageLocation, write_layer_record, write_table_record};
use super::layout::{LAYER_RECORD_SIZE, LAYER_TABLE_RECORD_SIZE, Layout, VariableSections};
use super::preview_record::write_preview_record;
use super::print_parameters::write_print_parameters;
use super::print_parameters_v4::write_print_parameters_v4;
use super::slicer_info::write_slicer_info;
use crate::ctb::PREVIEW_SIZES;
use crate::ctb::disclaimer::write_disclaimer;
use crate::ctb::resin_parameters::{byte_length, write_resin_parameters};
use crate::ctb::rle7;
use crate::ctb::rle15;
use crate::ctb::wire::{counted, write_zeros};
use crate::error::WriteFailure;
use crate::layer_images::write_layer_images;
use crate::preview::{PreviewImage, PrintPreviews};
use crate::print_program::PrintProgram;

/// Write `stack` as a plain CTB v5 file running the schedule in
/// `program`, whose records pair with the stack's layers in order.
///
/// The first row of every layer image is the plate's maximum Y edge,
/// the order this container stores its previews in. The raster
/// setup's mirror flags, which the header declares, are what turns an
/// image around.
///
/// Without `previews` the machine displays a black thumbnail for the
/// file, since the container has no way to say a preview is absent.
///
/// `out` is seeked back over once, to fill the layer table, so it
/// must be positioned at the start of the file.
///
/// # Errors
///
/// [`WriteFailure::LayerCountMismatch`] when the program and the
/// stack describe different numbers of layers, and
/// [`WriteFailure::PreviewSize`] when a rendering is not the size
/// this writer produces. Both are raised before any byte is written.
pub fn write_file(
    program: &PrintProgram,
    previews: Option<&PrintPreviews>,
    stack: &ContourStack,
    out: &mut (impl Write + Seek),
) -> Result<(), WriteFailure> {
    let stack_layers = stack.layers().len();
    if program.layers.len() != stack_layers {
        return Err(WriteFailure::LayerCountMismatch {
            program_layers: program.layers.len(),
            stack_layers,
        });
    }

    let black_large;
    let black_small;
    let (large, small) = match previews {
        Some(previews) => {
            previews.large.check_size(PREVIEW_SIZES.large)?;
            previews.small.check_size(PREVIEW_SIZES.small)?;
            (&previews.large, &previews.small)
        }
        None => {
            black_large = PreviewImage::black(PREVIEW_SIZES.large);
            black_small = PreviewImage::black(PREVIEW_SIZES.small);
            (&black_large, &black_small)
        }
    };

    let mut large_blob = Vec::new();
    rle15::encode_preview(large, &mut large_blob);
    let mut small_blob = Vec::new();
    rle15::encode_preview(small, &mut small_blob);

    let machine_name = program.machine_name.as_str().as_bytes();
    let layout = Layout::plan(VariableSections {
        large_preview_bytes: counted(large_blob.len(), "preview image")?,
        small_preview_bytes: counted(small_blob.len(), "preview image")?,
        machine_name_bytes: counted(machine_name.len(), "machine name")?,
        resin_parameters_bytes: byte_length(program),
        layer_count: counted(program.layers.len(), "layer table")?,
    });

    write_header(program, &layout, out)?;
    write_preview_record(large, &large_blob, layout.large_preview_image, out)?;
    out.write_all(&large_blob)?;
    write_preview_record(small, &small_blob, layout.small_preview_image, out)?;
    out.write_all(&small_blob)?;
    write_print_parameters(program, out)?;
    write_slicer_info(program, &layout, out)?;
    out.write_all(machine_name)?;
    write_disclaimer(out)?;
    write_print_parameters_v4(program, &layout, out)?;
    write_resin_parameters(program, layout.resin_parameters, out)?;

    let table_bytes = LAYER_TABLE_RECORD_SIZE as usize * program.layers.len();
    write_zeros(out, table_bytes)?;
    let mut table = Vec::with_capacity(table_bytes);

    let mut record_at = u64::from(layout.layer_data);
    write_layer_images(
        stack,
        &program.raster,
        |_, rows| {
            let mut image = Vec::new();
            rle7::encode_layer(rows, &mut image);
            image
        },
        |index, image| -> Result<(), WriteFailure> {
            let layer = &program.layers[index];
            let length = counted(image.len(), "layer image")?;
            let location = ImageLocation::new(record_at + u64::from(LAYER_RECORD_SIZE), length);
            write_table_record(layer, location, &mut table)?;
            write_layer_record(layer, location, out)?;
            out.write_all(&image)?;
            record_at += u64::from(LAYER_RECORD_SIZE) + u64::from(length);
            Ok(())
        },
    )?;

    out.seek(SeekFrom::Start(u64::from(layout.layer_table)))?;
    out.write_all(&table)?;
    out.seek(SeekFrom::End(0))?;
    Ok(())
}
