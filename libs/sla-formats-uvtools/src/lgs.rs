/*
 * SPDX-FileCopyrightText: 2020-2026 PTRTECH, Tiago Conceição (sn4k3) and UVtools contributors
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 *
 * Ported to Rust from UVtools UVtools.Core/FileFormats/LGSFile.cs (with the
 * preview packing of UVtools.Core/FileFormats/FileFormat.cs), UVtools v7.1.1,
 * commit 8b5c91fd41b60bd6265ba1c1f2b2b64636b873e8, modified for Breadslicer:
 * the layers are encoded from the slicer's run-length rows, each layer is
 * written once (UVtools 7.1.1 appends every layer record a second time, empty),
 * and the settings come from a Breadslicer print program.
 *
 * This program is free software: you can redistribute it and/or modify it
 * under the terms of the GNU Affero General Public License as published by the
 * Free Software Foundation, either version 3 of the License, or (at your
 * option) any later version. It is distributed WITHOUT ANY WARRANTY; see the
 * GNU Affero General Public License (LICENSE.md) for details.
 */

//! Longer's LGS: `.lgs`, `.lgs30`, `.lgs120` and `.lgs4k` for the Orange 10,
//! 30, 120 and 4K (#778).
//!
//! A 180-byte header of floats (the panel, the exposures, the lift), a 120 x
//! 150 preview as big-endian RGB565, on the Orange 120 a second preview as a
//! PNG, then each layer as its byte count and its runs. A run is a grey's
//! high nibble (16 greys) with its length written 4 bits a byte, most
//! significant first; the Orange 4K's layers are turned a quarter clockwise.

use std::io;

use sc_sl1cer::contour::ContourStack;
use sc_sl1cer::raster::rle::RleRun;
use sc_sla_formats::layer_images::write_layer_images;
use sc_sla_formats::{
    OtherFormat, PreviewImage, PreviewSize, PreviewSizes, PrintPreviews, PrintProgram,
    PrinterFileSink, WriteFailure,
};

use crate::chitubox_zip::previews_or_black;
use crate::program::{ProgramSummary, round_away, round_display, round_height};
use crate::resample::png_bytes;

/// The 120 x 150 preview every LGS carries, and the 1200 x 1600 one the
/// Orange 120 adds.
pub const PREVIEW_SIZES: PreviewSizes = PreviewSizes {
    small: PreviewSize {
        width: 120,
        height: 150,
    },
    large: PreviewSize {
        width: 1200,
        height: 1600,
    },
};

pub(crate) const NAME: &[u8; 8] = b"Longer3D";

/// The model code the header states and the height the machine prints to.
fn machine(format: OtherFormat) -> (u32, f32) {
    match format {
        OtherFormat::Lgs30 => (30, 170.0),
        OtherFormat::Lgs120 => (120, 150.0),
        OtherFormat::Lgs4k => (4500, 190.0),
        _ => (10, 140.0),
    }
}

/// The grey a pixel is stored at: its high nibble, repeated.
pub(crate) fn stored_grey(grey: u8) -> u8 {
    (grey >> 4) * 17
}

fn header(format: OtherFormat, program: &PrintProgram) -> Vec<u8> {
    let (model, machine_z) = machine(format);
    let summary = ProgramSummary::of(program);
    let (b, n) = (summary.bottom, summary.normal);
    let raster = &program.raster;
    let panel = raster.panel_resolution();
    let (width_mm, height_mm) = raster.panel_size_mm();
    let ppmm = |pixels: u32, mm: f64| pixels as f32 / round_display(mm as f32);
    let layer_height = round_height(program.layer_height_mm.value() as f32);
    let ms = |seconds: f32| round_away(seconds * 1000.0, 2);
    let mut out = Vec::with_capacity(180);
    out.extend_from_slice(NAME);
    let mut u = |value: u32| out.extend_from_slice(&value.to_le_bytes());
    u(1);
    u(1);
    u(model);
    u(0);
    u(34);
    let floats = [
        ppmm(panel.x(), width_mm),
        ppmm(panel.y(), height_mm),
        panel.x() as f32,
        panel.y() as f32,
        layer_height,
        ms(n.exposure_sec),
        ms(b.exposure_sec),
        10.0,
        ms(n.wait_before_cure_sec),
        ms(b.wait_before_cure_sec),
        program.bottom_layer_count as f32 * layer_height,
        0.6,
        round_away(b.total_lift_mm(), 2),
        round_away(n.total_lift_mm(), 2),
        round_away(n.lift_mm_min, 2),
        round_away(n.lift_mm_min, 2),
        round_away(b.lift_mm_min, 2),
        round_away(b.lift_mm_min, 2),
        5.0,
        60.0,
        10.0,
        600.0,
        600.0,
        2.0,
        0.2,
        60.0,
        1.0,
        6.0,
        150.0,
        1001.0,
        machine_z,
    ];
    for value in floats {
        out.extend_from_slice(&value.to_le_bytes());
    }
    for value in [
        0,
        0,
        0,
        program.layers.len() as u32,
        4,
        PREVIEW_SIZES.small.width,
        PREVIEW_SIZES.small.height,
    ] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

/// A preview as big-endian RGB565.
fn rgb565(image: &PreviewImage) -> Vec<u8> {
    let mut out = Vec::with_capacity(image.rgb().len() / 3 * 2);
    for pixel in image.rgb().chunks_exact(3) {
        let (r, g, b) = (
            u16::from(pixel[0]),
            u16::from(pixel[1]),
            u16::from(pixel[2]),
        );
        let packed = ((r & 0b1111_1000) << 8) | ((g & 0b1111_1100) << 3) | (b >> 3);
        out.extend_from_slice(&packed.to_be_bytes());
    }
    out
}

/// Encodes one layer's pixels, `width` per row, as LGS runs.
pub(crate) fn encode_layer(pixels: &[u8]) -> Vec<u8> {
    fn add(out: &mut Vec<u8>, mut span: u32, color: u8) {
        let mut chunk = [0_u8; 8];
        let mut index = chunk.len();
        while span > 0 {
            index -= 1;
            chunk[index] = (span & 0xf) as u8 | (color & 0xf0);
            span >>= 4;
        }
        out.extend_from_slice(&chunk[index..]);
    }
    let mut out = Vec::new();
    let mut span = 0_u32;
    let mut last = 0_u8;
    let mut at = 0;
    while at < pixels.len() {
        let color = pixels[at] & 0xf0;
        let mut end = at + 1;
        while end < pixels.len() && pixels[end] & 0xf0 == color {
            end += 1;
        }
        let run = (end - at) as u32;
        if color == last {
            span += run;
        } else {
            add(&mut out, span, last);
            span = run;
        }
        last = color;
        at = end;
    }
    add(&mut out, span, last);
    out
}

/// The frame's pixels, turned a quarter clockwise when the machine reads
/// them so.
fn layer_pixels(rows: &[RleRun], width: u32, height: u32, turn: bool) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(width as usize * height as usize);
    for run in rows {
        pixels.extend(std::iter::repeat_n(run.value, run.length as usize));
    }
    if !turn {
        return pixels;
    }
    let (w, h) = (width as usize, height as usize);
    let mut turned = vec![0_u8; pixels.len()];
    // Clockwise: the new image is `h` wide and `w` high, and its pixel
    // (x, y) is the old (y, h - 1 - x).
    for y in 0..w {
        for x in 0..h {
            turned[y * h + x] = pixels[(h - 1 - x) * w + y];
        }
    }
    turned
}

/// Writes `stack` as an LGS of `format` running the settings in `program`.
///
/// # Errors
///
/// [`WriteFailure::PreviewSize`] when a rendering is not the size this format
/// fixes, raised before any byte is written, and [`WriteFailure::Io`] when the
/// file cannot be written.
pub fn write_file(
    format: OtherFormat,
    program: &PrintProgram,
    previews: Option<&PrintPreviews>,
    stack: &ContourStack,
    out: &mut dyn PrinterFileSink,
) -> Result<(), WriteFailure> {
    let (small, large) = previews_or_black(previews, PREVIEW_SIZES)?;
    let (model, _) = machine(format);
    let panel = program.raster.panel_resolution();
    let (width, height) = (panel.x(), panel.y());
    out.write_all(&header(format, program))?;
    out.write_all(&rgb565(&small))?;
    if model == 120 {
        let png = png_bytes(&large).map_err(|err| WriteFailure::Io(io::Error::other(err)))?;
        out.write_all(&(png.len() as u32).to_le_bytes())?;
        out.write_all(&png)?;
        out.write_all(&0_u16.to_le_bytes())?;
    }
    let turn = model == 4000 || model == 4500;
    write_layer_images(
        stack,
        &program.raster,
        16,
        |_, rows| encode_layer(&layer_pixels(rows, width, height, turn)),
        |_, bytes: Vec<u8>| -> Result<(), WriteFailure> {
            out.write_all(&(bytes.len() as u32).to_le_bytes())?;
            out.write_all(&bytes)?;
            Ok(())
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests_lgs;
