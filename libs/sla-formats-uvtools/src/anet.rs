/*
 * SPDX-FileCopyrightText: 2020-2026 PTRTECH, Tiago Conceição (sn4k3) and UVtools contributors
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 *
 * Ported to Rust from UVtools UVtools.Core/FileFormats/AnetFile.cs (with the
 * preview packing of UVtools.Core/FileFormats/FileFormat.cs), UVtools v7.1.1,
 * commit 8b5c91fd41b60bd6265ba1c1f2b2b64636b873e8, modified for Breadslicer:
 * the layers are encoded from the slicer's run-length rows and the settings
 * come from a Breadslicer print program.
 *
 * This program is free software: you can redistribute it and/or modify it
 * under the terms of the GNU Affero General Public License as published by the
 * Free Software Foundation, either version 3 of the License, or (at your
 * option) any later version. It is distributed WITHOUT ANY WARRANTY; see the
 * GNU Affero General Public License (LICENSE.md) for details.
 */

//! Anet's N4 and N7 files: `.n4` and `.n7` (#778).
//!
//! Big-endian throughout: the version and the job name as UTF-16 strings, the
//! pixel size and layer height as doubles, whole seconds and millimetres for
//! the exposure and lift, a 260 x 140 BGR565 bitmap, then each layer's lit
//! count, bounds and a bit stream of black-and-white runs (no greys).

use sc_sl1cer::contour::ContourStack;
use sc_sl1cer::raster::rle::RleRun;
use sc_sla_formats::layer_images::write_layer_images;
use sc_sla_formats::{
    OtherFormat, PreviewImage, PreviewSize, PreviewSizes, PrintPreviews, PrintProgram,
    PrinterFileSink, WriteFailure,
};

use crate::chitubox_zip::previews_or_black;
use crate::layer_stats::LayerStats;
use crate::program::ProgramSummary;

/// The one preview, 260 x 140; the small size is unused.
pub const PREVIEW_SIZES: PreviewSizes = PreviewSizes {
    small: PreviewSize {
        width: 260,
        height: 140,
    },
    large: PreviewSize {
        width: 260,
        height: 140,
    },
};

/// The bitmap header in front of the preview's pixels.
const BMP_HEADER: [u8; 66] = [
    66, 77, 162, 4, 0, 0, 0, 0, 0, 0, 66, 0, 0, 0, 40, 0, 0, 0, 4, 1, 0, 0, 140, 0, 0, 0, 1, 0, 16,
    0, 3, 0, 0, 0, 96, 4, 0, 0, 18, 11, 0, 0, 18, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 248, 0, 0,
    224, 7, 0, 0, 31, 0, 0, 0,
];
const PREVIEW_BYTES: usize = 72_866;
const JOB_NAME: &str = "breadslicer";

/// The panel each machine takes.
fn panel(format: OtherFormat) -> (u32, u32) {
    match format {
        OtherFormat::AnetN7 => (2560, 1600),
        _ => (1440, 2560),
    }
}

/// The grey a pixel is stored at: black or white.
pub(crate) fn stored_grey(grey: u8) -> u8 {
    if grey > 127 { 255 } else { 0 }
}

fn utf16(text: &str, out: &mut Vec<u8>) {
    let units: Vec<u16> = text.encode_utf16().collect();
    out.extend_from_slice(&((units.len() * 2) as i32).to_be_bytes());
    for unit in units {
        out.extend_from_slice(&unit.to_be_bytes());
    }
}

/// Rounds halves away from zero, in double precision.
fn round_away_f64(value: f64, digits: i32) -> f64 {
    let power = 10_f64.powi(digits);
    (value * power).round() / power
}

/// Rounds halves to even, as .NET's `Math.Round` does for doubles.
fn round_even(value: f64, digits: i32) -> f64 {
    let power = 10_f64.powi(digits);
    (value * power).round_ties_even() / power
}

fn preview(image: &PreviewImage) -> Vec<u8> {
    let mut out = vec![0_u8; PREVIEW_BYTES];
    out[..BMP_HEADER.len()].copy_from_slice(&BMP_HEADER);
    let mut at = BMP_HEADER.len();
    for pixel in image.rgb().as_chunks::<3>().0 {
        let (r, g, b) = (
            u16::from(pixel[0]),
            u16::from(pixel[1]),
            u16::from(pixel[2]),
        );
        let packed = ((b & 0b1111_1000) << 8) | ((g & 0b1111_1100) << 3) | (r >> 3);
        if at + 2 > out.len() {
            break;
        }
        out[at..at + 2].copy_from_slice(&packed.to_le_bytes());
        at += 2;
    }
    out
}

fn header(program: &PrintProgram, preview: &[u8]) -> Vec<u8> {
    let summary = ProgramSummary::of(program);
    let (b, n) = (summary.bottom, summary.normal);
    let (width_mm, height_mm) = program.raster.panel_size_mm();
    let panel = program.raster.panel_resolution();
    let pixel_mm = (width_mm / f64::from(panel.x())).max(height_mm / f64::from(panel.y()));
    let mut out = Vec::new();
    utf16("3", &mut out);
    utf16(JOB_NAME, &mut out);
    utf16("", &mut out);
    out.extend_from_slice(&round_even(pixel_mm, 3).to_be_bytes());
    // In double precision, as UVtools rounds it: 0.05, not the float's 0.0500000007.
    out.extend_from_slice(&round_away_f64(program.layer_height_mm.value(), 3).to_be_bytes());
    out.extend_from_slice(&0_u32.to_be_bytes());
    out.extend_from_slice(&0_u32.to_be_bytes());
    let whole = |value: f32| (f64::from(value)).round_ties_even() as u32;
    out.extend_from_slice(&whole(n.exposure_sec).to_be_bytes());
    out.extend_from_slice(&whole(b.exposure_sec).to_be_bytes());
    out.extend_from_slice(&program.bottom_layer_count.to_be_bytes());
    out.extend_from_slice(&whole(n.lift_mm_min / 60.0).to_be_bytes());
    out.extend_from_slice(&whole(n.total_lift_mm()).to_be_bytes());
    out.extend_from_slice(&260_u32.to_be_bytes());
    out.extend_from_slice(&140_u32.to_be_bytes());
    out.extend_from_slice(&(PREVIEW_BYTES as u32).to_be_bytes());
    out.extend_from_slice(preview);
    let ml = round_even(program.volume_mm3.value() / 1000.0, 3);
    out.extend_from_slice(&round_even(ml * 1000.0, 3).to_be_bytes());
    out.extend_from_slice(&(program.print_time_sec.value() as u32).to_be_bytes());
    out.extend_from_slice(&(program.layers.len() as u32).to_be_bytes());
    out
}

/// Writes fields most significant bit first into bytes filled from their
/// least significant bit, as the machine reads them.
#[derive(Default)]
struct Bits {
    out: Vec<u8>,
    accumulator: u64,
    held: u32,
    total: u32,
}

impl Bits {
    fn put(&mut self, mut value: u32, count: u32) {
        let mut mirrored = 0_u32;
        for _ in 0..count {
            mirrored = (mirrored << 1) | (value & 1);
            value >>= 1;
        }
        self.accumulator |= u64::from(mirrored) << self.held;
        self.held += count;
        self.total += count;
        while self.held >= 8 {
            self.out.push(self.accumulator as u8);
            self.accumulator >>= 8;
            self.held -= 8;
        }
    }

    fn finish(mut self) -> (Vec<u8>, u32) {
        if self.held > 0 {
            self.out.push(self.accumulator as u8);
        }
        (self.out, self.total)
    }
}

/// One layer: its record, then its bit stream.
pub(crate) fn encode_layer(runs: &[RleRun], width: u32, height: u32) -> Vec<u8> {
    let stats = LayerStats::of_greys_from(runs, width, 128);
    // Runs of black and of white, merged across greys of one side.
    let mut runs_bw: Vec<(bool, u32)> = Vec::new();
    for run in runs {
        if run.length == 0 {
            continue;
        }
        let white = run.value > 127;
        match runs_bw.last_mut() {
            Some((last, length)) if *last == white => *length += run.length,
            _ => runs_bw.push((white, run.length)),
        }
    }
    let mut bits = Bits::default();
    bits.put(width, 16);
    bits.put(height, 16);
    bits.put(
        u32::from(runs_bw.first().is_some_and(|&(white, _)| white)),
        1,
    );
    let mut white_pixels = 0_u32;
    for &(white, length) in &runs_bw {
        if white {
            white_pixels += length;
        }
        let size = if length <= 1 {
            0
        } else {
            32 - (length - 1).leading_zeros()
        };
        bits.put(size, 5);
        bits.put(length, size + 1);
    }
    let (stream, total_bits) = bits.finish();
    let (x0, y0, x1, y1) = stats.bounds.map_or((0, 0, 0, 0), |b| {
        (b.x0 as i32, b.y0 as i32, b.x1 as i32 - 1, b.y1 as i32 - 1)
    });
    let mut out = Vec::with_capacity(24 + stream.len());
    out.extend_from_slice(&white_pixels.to_be_bytes());
    for value in [x0, y0, x1, y1] {
        out.extend_from_slice(&value.to_be_bytes());
    }
    out.extend_from_slice(&total_bits.to_be_bytes());
    out.extend_from_slice(&stream);
    out
}

/// Writes `stack` as an Anet file of `format` running the settings in
/// `program`.
///
/// # Errors
///
/// [`WriteFailure::Unencodable`] when the panel is not the machine's,
/// [`WriteFailure::PreviewSize`] when a rendering is not the size this format
/// fixes, both before any byte is written, and [`WriteFailure::Io`] when the
/// file cannot be written.
pub fn write_file(
    format: OtherFormat,
    program: &PrintProgram,
    previews: Option<&PrintPreviews>,
    stack: &ContourStack,
    out: &mut dyn PrinterFileSink,
) -> Result<(), WriteFailure> {
    let resolution = program.raster.panel_resolution();
    let (width, height) = (resolution.x(), resolution.y());
    if (width, height) != panel(format) {
        let (want_x, want_y) = panel(format);
        return Err(WriteFailure::Unencodable(format!(
            "the .{} panel is {want_x} x {want_y} pixels, not {width} x {height}",
            format.extension()
        )));
    }
    let (image, _) = previews_or_black(previews, PREVIEW_SIZES)?;
    out.write_all(&header(program, &preview(&image)))?;
    write_layer_images(
        stack,
        &program.raster,
        128,
        |_, rows| encode_layer(rows, width, height),
        |_, bytes: Vec<u8>| -> Result<(), WriteFailure> {
            out.write_all(&bytes)?;
            Ok(())
        },
    )?;
    // The supports count, always 0.
    out.write_all(&[0; 4])?;
    Ok(())
}

#[cfg(test)]
mod tests_anet;
