/*
 * SPDX-FileCopyrightText: 2020-2026 PTRTECH, Tiago Conceição (sn4k3) and UVtools contributors
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 *
 * Ported to Rust from UVtools UVtools.Core/FileFormats/QDTFile.cs, UVtools
 * v7.1.1, commit 8b5c91fd41b60bd6265ba1c1f2b2b64636b873e8, modified for
 * Breadslicer: the scan lines are read off the slicer's run-length rows (the
 * line finder UVtools calls lives in a library it depends on; its lines here
 * are what UVtools' own reader draws back), and the tenth-size copy is
 * averaged in bilinear steps as OpenCV resizes.
 *
 * This program is free software: you can redistribute it and/or modify it
 * under the terms of the GNU Affero General Public License as published by the
 * Free Software Foundation, either version 3 of the License, or (at your
 * option) any later version. It is distributed WITHOUT ANY WARRANTY; see the
 * GNU Affero General Public License (LICENSE.md) for details.
 */

//! Emake3D's QDT: `.qdt` for the Galaxy 1 (#778).
//!
//! Text, CRLF lines. A header (`JieHe,<layer µm>,<rows>,<columns>,2,019,0,FA`),
//! then per layer its number, a tenth-size copy, `FB`, the layer and `FC`;
//! `FD` and the layer count end it. A layer is its lit pixels as vertical
//! lines, column by column from the left: `x,y,0` moves (to an absolute row,
//! or by a gap within the same column), `x,length,1` draws. Odd layers run
//! mirrored, their columns counted `width + 1 - x`.

use std::fmt::Write as _;

use sc_sl1cer::contour::ContourStack;
use sc_sl1cer::raster::rle::RleRun;
use sc_sla_formats::layer_images::write_layer_images;
use sc_sla_formats::{PrintProgram, PrinterFileSink, WriteFailure};

use crate::program::round_away;

/// A vertical run of lit pixels: its column and its first and last rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Line {
    x: i64,
    start_y: i64,
    end_y: i64,
}

/// The vertical runs of pixels brighter than 127, column by column from the
/// left, each column's runs from the top; `x_offset` is added to each column.
fn scan_lines(pixels: &[u8], width: usize, height: usize, x_offset: i64) -> Vec<Line> {
    let mut lines = Vec::new();
    for x in 0..width {
        let mut start: Option<usize> = None;
        for y in 0..height {
            let lit = pixels[y * width + x] > 127;
            match (lit, start) {
                (true, None) => start = Some(y),
                (false, Some(from)) => {
                    lines.push(Line {
                        x: x as i64 + x_offset,
                        start_y: from as i64,
                        end_y: y as i64 - 1,
                    });
                    start = None;
                }
                _ => {}
            }
        }
        if let Some(from) = start {
            lines.push(Line {
                x: x as i64 + x_offset,
                start_y: from as i64,
                end_y: height as i64 - 1,
            });
        }
    }
    lines
}

/// Resizes to a tenth, bilinearly, as OpenCV's default resize does.
fn tenth(pixels: &[u8], width: usize, height: usize) -> (Vec<u8>, usize, usize) {
    let (w, h) = (
        ((width as f64) * 0.1).round().max(1.0) as usize,
        ((height as f64) * 0.1).round().max(1.0) as usize,
    );
    let mut out = vec![0_u8; w * h];
    for y in 0..h {
        let fy = ((y as f64 + 0.5) / 0.1 - 0.5).clamp(0.0, (height - 1) as f64);
        let (y0, ty) = (fy.floor() as usize, fy - fy.floor());
        let y1 = (y0 + 1).min(height - 1);
        for x in 0..w {
            let fx = ((x as f64 + 0.5) / 0.1 - 0.5).clamp(0.0, (width - 1) as f64);
            let (x0, tx) = (fx.floor() as usize, fx - fx.floor());
            let x1 = (x0 + 1).min(width - 1);
            let at = |xx: usize, yy: usize| f64::from(pixels[yy * width + xx]);
            let top = at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx;
            let bottom = at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx;
            out[y * w + x] = (top * (1.0 - ty) + bottom * ty).round().clamp(0.0, 255.0) as u8;
        }
    }
    (out, w, h)
}

fn write_lines(text: &mut String, lines: &[Line]) {
    let Some(first) = lines.first() else {
        return;
    };
    let _ = write!(text, "{},{},0\r\n", first.x, first.start_y);
    let _ = write!(text, "{},{},1\r\n", first.x, first.end_y - first.start_y);
    let mut current = *first;
    for pair in lines.windows(2) {
        let (previous, line) = (pair[0], pair[1]);
        current = line;
        if line.x == previous.x {
            let _ = write!(text, "{},{},0\r\n", line.x, line.start_y - previous.end_y);
        } else {
            let _ = write!(text, "{},{},0\r\n", previous.x, previous.start_y);
            let _ = write!(text, "{},{},0\r\n", line.x, line.start_y);
        }
        let _ = write!(text, "{},{},1\r\n", line.x, line.end_y - line.start_y);
    }
    let _ = write!(text, "{},{},0\r\n", current.x, current.end_y);
}

/// One layer's block: its number, the tenth-size copy, then the layer.
pub(crate) fn layer_block(index: usize, rows: &[RleRun], width: u32, height: u32) -> String {
    let (w, h) = (width as usize, height as usize);
    let mut pixels = Vec::with_capacity(w * h);
    for run in rows {
        pixels.extend(std::iter::repeat_n(run.value, run.length as usize));
    }
    let mut text = format!("{}\r\n", index + 1);
    if pixels.iter().any(|&p| p > 127) {
        let (small, sw, sh) = tenth(&pixels, w, h);
        write_lines(&mut text, &scan_lines(&small, sw, sh, 0));
        text.push_str("FB\r\n");
        let lines = if !index.is_multiple_of(2) {
            let mut flipped = vec![0_u8; pixels.len()];
            for y in 0..h {
                for x in 0..w {
                    flipped[y * w + x] = pixels[y * w + (w - 1 - x)];
                }
            }
            scan_lines(&flipped, w, h, 2)
        } else {
            scan_lines(&pixels, w, h, 0)
        };
        write_lines(&mut text, &lines);
    } else {
        text.push_str("FB\r\n");
    }
    text.push_str("FC\r\n");
    text
}

/// Writes `stack` as a QDT running the settings in `program`.
///
/// # Errors
///
/// [`WriteFailure::Io`] when the file cannot be written.
pub fn write_file(
    program: &PrintProgram,
    stack: &ContourStack,
    out: &mut dyn PrinterFileSink,
) -> Result<(), WriteFailure> {
    let panel = program.raster.panel_resolution();
    let (width, height) = (panel.x(), panel.y());
    let layer_um = round_away(program.layer_height_mm.value() as f32 * 1000.0, 0) as u32;
    out.write_all(format!("JieHe,{layer_um},{height},{width},2,019,0,FA\r\n").as_bytes())?;
    write_layer_images(
        stack,
        &program.raster,
        128,
        |index, rows| layer_block(index, rows, width, height),
        |_, block: String| -> Result<(), WriteFailure> {
            out.write_all(block.as_bytes())?;
            Ok(())
        },
    )?;
    out.write_all(format!("FD\r\n{}\r\n", program.layers.len()).as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests_qdt;
