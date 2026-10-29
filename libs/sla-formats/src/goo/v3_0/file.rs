/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The whole file: the header, one record per layer, the footer.
//!
//! Layers are rasterised and encoded one at a time, so the largest
//! buffer alive is a single layer's image.

use std::io::Write;

use sc_sl1cer::contour::ContourStack;

use super::header::{FILE_MAGIC, SizedPreviews, write_header};
use super::payload::encode_payload;
use super::record::write_layer;
use crate::error::WriteFailure;
use crate::layer_images::write_layer_images;
use crate::preview::PrintPreviews;
use crate::print_program::PrintProgram;

/// Precedes the magic that closes the file.
const FOOTER_PADDING: [u8; 3] = [0x00, 0x00, 0x00];

/// Write `stack` as a GOO V3.0 file running the schedule in
/// `program`, whose records pair with the stack's layers in order.
///
/// The first row of every layer image is the plate's maximum Y edge,
/// the order this container stores its previews in. The raster
/// setup's mirror flags, which the header declares, are what turns an
/// image around; its panel rotation turns it as
/// [`write_layer_images`](crate::layer_images::write_layer_images)
/// describes.
///
/// Without `previews` the machine displays an empty thumbnail for the
/// file.
///
/// # Errors
///
/// [`WriteFailure::LayerCountMismatch`] when the program and the
/// stack describe different numbers of layers, and
/// [`WriteFailure::PreviewSize`] when a rendering is not the size
/// this container fixes. Both are raised before any byte is written.
pub fn write_file(
    program: &PrintProgram,
    previews: Option<&PrintPreviews>,
    stack: &ContourStack,
    out: &mut impl Write,
) -> Result<(), WriteFailure> {
    let stack_layers = stack.layers().len();
    if program.layers.len() != stack_layers {
        return Err(WriteFailure::LayerCountMismatch {
            program_layers: program.layers.len(),
            stack_layers,
        });
    }
    let previews = previews.map(SizedPreviews::check).transpose()?;

    let park_z_mm = program.raster.build_volume().z_mm() as f32;

    write_header(program, previews, out)?;
    write_layer_images(
        stack,
        &program.raster,
        |_, rows| {
            let mut image = Vec::new();
            encode_payload(rows, &mut image);
            image
        },
        |index, image| -> Result<(), WriteFailure> {
            write_layer(&program.layers[index], park_z_mm, &image, out)?;
            Ok(())
        },
    )?;
    out.write_all(&FOOTER_PADDING)?;
    out.write_all(&FILE_MAGIC)?;
    Ok(())
}
