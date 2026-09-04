/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The whole file: the header, one record per layer, the footer.
//!
//! Layers are rasterised and encoded one at a time, so the largest
//! buffer alive is a single layer's image.

use std::fmt;
use std::io::{self, Write};

use sc_sl1cer::contour::ContourStack;
use sc_sl1cer::raster::pixel_map::PixelMap;
use sc_sl1cer::raster::rasterize_layer;

use super::header::{FILE_MAGIC, write_header};
use super::payload::encode_payload;
use super::record::write_layer;
use crate::print_program::PrintProgram;

/// Precedes the magic that closes the file.
const FOOTER_PADDING: [u8; 3] = [0x00, 0x00, 0x00];

/// Write `stack` as a GOO V3.0 file running the schedule in
/// `program`, whose records pair with the stack's layers in order.
///
/// The first row of every layer image is the plate's minimum Y edge,
/// which no print has yet checked against the panel. The raster
/// setup's mirror flags, which the header declares, are what turns an
/// image around.
///
/// # Errors
///
/// [`WriteFailure::LayerCountMismatch`] when the program and the
/// stack describe different numbers of layers, before any byte is
/// written.
pub fn write_file(
    program: &PrintProgram,
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

    let raster = &program.raster;
    let plate = raster.build_volume();
    let pixel_map = PixelMap::new(
        plate.x(),
        plate.y(),
        *raster.pixel_resolution(),
        raster.mirror(),
    );
    let park_z_mm = plate.z_mm() as f32;

    write_header(program, out)?;
    let mut image = Vec::new();
    for (layer, contours) in program.layers.iter().zip(stack.layers()) {
        let pixels = rasterize_layer(contours, &pixel_map, raster.aa(), raster.min_alpha_pct());
        image.clear();
        encode_payload(pixels.runs(), &mut image);
        write_layer(layer, park_z_mm, &image, out)?;
    }
    out.write_all(&FOOTER_PADDING)?;
    out.write_all(&FILE_MAGIC)?;
    Ok(())
}

#[derive(Debug)]
pub enum WriteFailure {
    /// The print program and the contour stack describe different
    /// numbers of layers.
    LayerCountMismatch {
        program_layers: usize,
        stack_layers: usize,
    },
    Io(io::Error),
}

impl fmt::Display for WriteFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LayerCountMismatch {
                program_layers,
                stack_layers,
            } => write!(
                f,
                "the print program holds {program_layers} layer(s) and the contour stack {stack_layers}",
            ),
            Self::Io(err) => write!(f, "writing the file failed: {err}"),
        }
    }
}

impl std::error::Error for WriteFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::LayerCountMismatch { .. } => None,
        }
    }
}

impl From<io::Error> for WriteFailure {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}
