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
use sc_sl1cer::raster::pixel_map::PixelMap;
use sc_sl1cer::raster::rasterize_layer;

use super::header::{FILE_MAGIC, SizedPreviews, write_header};
use super::payload::encode_payload;
use super::record::write_layer;
use crate::error::WriteFailure;
use crate::preview::PrintPreviews;
use crate::print_program::PrintProgram;
use crate::row_order::reverse_rows;

/// Precedes the magic that closes the file.
const FOOTER_PADDING: [u8; 3] = [0x00, 0x00, 0x00];

/// Write `stack` as a GOO V3.0 file running the schedule in
/// `program`, whose records pair with the stack's layers in order.
///
/// The first row of every layer image is the plate's maximum Y edge,
/// the order this container stores its previews in. The raster
/// setup's mirror flags, which the header declares, are what turns an
/// image around.
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

    let raster = &program.raster;
    let plate = raster.build_volume();
    let pixel_map = PixelMap::new(
        plate.x(),
        plate.y(),
        *raster.pixel_resolution(),
        raster.mirror(),
    );
    let park_z_mm = plate.z_mm() as f32;

    write_header(program, previews, out)?;
    let mut image = Vec::new();
    let mut rows = Vec::new();
    for (layer, contours) in program.layers.iter().zip(stack.layers()) {
        let pixels = rasterize_layer(contours, &pixel_map, raster.aa(), raster.min_alpha_pct());
        reverse_rows(pixels.runs(), raster.pixel_resolution().x(), &mut rows);
        image.clear();
        encode_payload(&rows, &mut image);
        write_layer(layer, park_z_mm, &image, out)?;
    }
    out.write_all(&FOOTER_PADDING)?;
    out.write_all(&FILE_MAGIC)?;
    Ok(())
}
