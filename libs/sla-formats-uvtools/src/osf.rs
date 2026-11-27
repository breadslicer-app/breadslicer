/*
 * SPDX-FileCopyrightText: 2020-2026 PTRTECH, Tiago Conceição (sn4k3) and UVtools contributors
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 *
 * Ported to Rust from UVtools UVtools.Core/FileFormats/OSFFile.cs (with the
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

//! Vlare's Open File Format: `.osf` for the Peopoly Phenom Forge and XXL v2
//! (#778).
//!
//! Big-endian throughout. A 7-byte header (the length of everything before the
//! layers, the version, 2), four RGB565 previews each behind a 24-bit length,
//! the 126 bytes of settings in hundredths of a
//! second and thousandths of a millimetre, then each layer: `0D 0A`, its run
//! count, its first lit row and its runs. A run is a grey with its low bit
//! clear for a single pixel, or set and followed by a length of 1 to 4 bytes.
//! A layer's runs start at its first lit row and end on the row of its last
//! lit pixel.

use sc_sl1cer::contour::ContourStack;
use sc_sl1cer::raster::rle::RleRun;
use sc_sla_formats::layer_images::write_layer_images;
use sc_sla_formats::{
    PreviewImage, PreviewSize, PreviewSizes, PrintPreviews, PrintProgram, PrinterFileSink,
    WriteFailure,
};

use crate::chitubox_zip::previews_or_black;
use crate::program::{ProgramSummary, StageSettings, round_away};
use crate::resample::downscale;

/// The two previews the slicer renders: the file's smallest and largest. The
/// 2 sizes between are the largest scaled down, as UVtools fills them.
pub const PREVIEW_SIZES: PreviewSizes = PreviewSizes {
    small: PreviewSize {
        width: 148,
        height: 80,
    },
    large: PreviewSize {
        width: 404,
        height: 240,
    },
};

const MIDDLE_SIZES: [PreviewSize; 2] = [
    PreviewSize {
        width: 300,
        height: 140,
    },
    PreviewSize {
        width: 208,
        height: 116,
    },
];

const VERSION: u16 = 4;
const HEADER_BYTES: u32 = 7;
const SETTINGS_BYTES: u32 = 126;
const PREVIEW_SLOTS: u32 = 4;
const LAYER_MARK: u16 = 0x0D0A;

/// The grey a pixel is stored at: its low bit carries whether a run follows.
pub(crate) fn stored_grey(grey: u8) -> u8 {
    grey & 0xfe
}

fn u24(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes()[1..]);
}

fn u16_be(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn rgb565(image: &PreviewImage) -> Vec<u8> {
    let mut out = Vec::with_capacity(image.rgb().len() / 3 * 2);
    for pixel in image.rgb().as_chunks::<3>().0 {
        let (r, g, b) = (
            u16::from(pixel[0]),
            u16::from(pixel[1]),
            u16::from(pixel[2]),
        );
        let packed = ((r & 0b1111_1000) << 8) | ((g & 0b1111_1100) << 3) | (b >> 3);
        out.extend_from_slice(&packed.to_le_bytes());
    }
    out
}

/// One set of settings as the file keeps it: the first stage slow, the second
/// fast, both lifts in thousandths of a millimetre.
struct Motion {
    stage: StageSettings,
    lift_slow: u32,
    lift_total: u32,
    retract_slow: u32,
    retract_total: u32,
    lift_speed_fast: u16,
    retract_speed_slow: u16,
}

impl Motion {
    fn of(stage: StageSettings, second_retract_mm: f32) -> Self {
        let (lift2, lift2_speed, retract2_speed) =
            stage
                .second_stage
                .unwrap_or((0.0, stage.lift_mm_min, stage.retract_mm_min));
        let lift1 = round_away(stage.lift_mm, 2);
        let lift2 = round_away(lift2, 2);
        let total = round_away(lift1 + lift2, 2);
        let lift_slow = (lift1 * 1000.0) as u32;
        let retract2 = round_away(second_retract_mm, 2).clamp(0.0, total);
        Self {
            stage,
            lift_slow,
            lift_total: lift_slow + (lift2 * 1000.0) as u32,
            retract_slow: (retract2 * 1000.0) as u32,
            retract_total: (total * 1000.0) as u32,
            lift_speed_fast: lift2_speed as u16,
            retract_speed_slow: retract2_speed as u16,
        }
    }
}

fn second_retract_mm(program: &PrintProgram, index: Option<usize>) -> f32 {
    index
        .and_then(|i| program.layers.get(i))
        .and_then(|layer| layer.second_retract)
        .map_or(0.0, |stage| stage.distance_mm.value() as f32)
}

fn hundredths(value: f32) -> u32 {
    (value * 100.0) as u32
}

pub(crate) fn settings(program: &PrintProgram) -> Vec<u8> {
    let summary = ProgramSummary::of(program);
    let first_normal = program
        .bottom_layer_count
        .saturating_add(program.transition_layer_count) as usize;
    let normal_index = if first_normal < program.layers.len() {
        Some(first_normal)
    } else {
        program.layers.len().checked_sub(1)
    };
    let bottom = Motion::of(summary.bottom, second_retract_mm(program, Some(0)));
    let normal = Motion::of(summary.normal, second_retract_mm(program, normal_index));
    let panel = program.raster.panel_resolution();
    let (width_mm, height_mm) = program.raster.panel_size_mm();
    let pixel_um = (width_mm as f32 / panel.x() as f32 * 1000.0)
        .max(height_mm as f32 / panel.y() as f32 * 1000.0);
    let mirror = program.raster.mirror();
    let count = program.layers.len() as u32;

    let mut out = Vec::with_capacity(SETTINGS_BYTES as usize);
    u16_be(&mut out, panel.x() as u16);
    u16_be(&mut out, panel.y() as u16);
    u16_be(&mut out, (round_away(pixel_um, 3) * 100.0) as u16);
    out.push(u8::from(mirror.x) | (u8::from(mirror.y) << 1));
    out.push(bottom.stage.pwm);
    out.push(normal.stage.pwm);
    // Anti-aliasing, distortion and delayed exposure: UVtools leaves them off.
    out.extend_from_slice(&[0, 0, 0]);
    out.extend_from_slice(&count.to_be_bytes());
    u16_be(&mut out, 1);
    out.extend_from_slice(&count.saturating_sub(1).to_be_bytes());
    u24(
        &mut out,
        u32::from((program.layer_height_mm.value() as f32 * 100_000.0) as u16),
    );
    out.push(program.bottom_layer_count.min(255) as u8);
    u24(&mut out, hundredths(normal.stage.exposure_sec));
    u24(&mut out, hundredths(bottom.stage.exposure_sec));
    // The support delays, at UVtools' default.
    u24(&mut out, 50);
    u24(&mut out, 50);
    out.push(program.transition_layer_count.min(255) as u8);
    // A linear transition, with no interval step.
    out.push(0);
    u24(&mut out, 0);
    u24(&mut out, hundredths(normal.stage.wait_after_cure_sec));
    u24(&mut out, hundredths(normal.stage.wait_after_lift_sec));
    u24(&mut out, hundredths(normal.stage.wait_before_cure_sec));
    for motion in [&bottom, &normal] {
        u24(&mut out, motion.lift_slow);
        u24(&mut out, motion.lift_total);
    }
    for motion in [&bottom, &normal] {
        u24(&mut out, motion.retract_slow);
        u24(&mut out, motion.retract_total);
    }
    // S-shaped acceleration, the only one the machine runs.
    out.push(0);
    for motion in [&bottom, &normal] {
        u16_be(&mut out, 80);
        u16_be(&mut out, motion.stage.lift_mm_min as u16);
        u16_be(&mut out, motion.lift_speed_fast);
        out.push(5);
    }
    for motion in [&bottom, &normal] {
        u16_be(&mut out, 80);
        u16_be(&mut out, motion.retract_speed_slow);
        u16_be(&mut out, motion.stage.retract_mm_min as u16);
        out.push(5);
    }
    // The end speeds and decelerations, at UVtools' default.
    for _ in 0..4 {
        u16_be(&mut out, 7);
        out.push(5);
    }
    u16_be(
        &mut out,
        hundredths(bottom.stage.wait_after_cure_sec) as u16,
    );
    u16_be(
        &mut out,
        hundredths(bottom.stage.wait_after_lift_sec) as u16,
    );
    u16_be(
        &mut out,
        hundredths(bottom.stage.wait_before_cure_sec) as u16,
    );
    // Reserved, then the protocol.
    u16_be(&mut out, 0);
    out.push(0);
    debug_assert_eq!(out.len(), SETTINGS_BYTES as usize);
    out
}

/// Appends one run, as `AddRep` does.
fn add_run(out: &mut Vec<u8>, stride: u32, grey: u8, lines: &mut u32) {
    if stride == 0 {
        return;
    }
    *lines += 1;
    if stride == 1 {
        out.push(grey & 0xfe);
        return;
    }
    out.push(grey | 0x01);
    if stride <= 0x7f {
        out.push(stride as u8);
    } else if stride <= 0x3fff {
        out.extend_from_slice(&[(stride >> 8) as u8 | 0x80, stride as u8]);
    } else if stride <= 0x1f_ffff {
        out.extend_from_slice(&[
            (stride >> 16) as u8 | 0xc0,
            (stride >> 8) as u8,
            stride as u8,
        ]);
    } else {
        out.extend_from_slice(&[
            (stride >> 24) as u8 | 0xe0,
            (stride >> 16) as u8,
            (stride >> 8) as u8,
            stride as u8,
        ]);
    }
}

/// One layer: its record, then its runs.
pub(crate) fn encode_layer(runs: &[RleRun], width: u32) -> Vec<u8> {
    let width = u64::from(width);
    // The first and last lit pixels.
    let (mut first, mut last, mut at) = (None, 0_u64, 0_u64);
    for run in runs {
        if run.value != 0 && run.length > 0 {
            first.get_or_insert(at);
            last = at + u64::from(run.length) - 1;
        }
        at += u64::from(run.length);
    }
    let total = at;
    let start_y = first.map_or(0, |index| index / width);

    let mut body = Vec::new();
    let mut lines = 0_u32;
    let (mut grey, mut stride) = (0_u8, 0_u32);
    let (from, to) = (start_y * width, last + 1);
    let mut at = 0_u64;
    for run in runs {
        let (begin, end) = (at.max(from), (at + u64::from(run.length)).min(to));
        at += u64::from(run.length);
        if begin >= end {
            continue;
        }
        let length = (end - begin) as u32;
        if run.value == grey {
            stride += length;
        } else {
            add_run(&mut body, stride, grey, &mut lines);
            grey = run.value;
            stride = length;
        }
    }
    if grey != 0 {
        add_run(&mut body, stride, grey, &mut lines);
    }
    // The rest of the last lit pixel's row.
    let rest = ((total - last - 1) % width) as u32;
    add_run(&mut body, rest, 0, &mut lines);

    let mut out = Vec::with_capacity(8 + body.len());
    u16_be(&mut out, LAYER_MARK);
    out.extend_from_slice(&lines.to_be_bytes());
    u16_be(&mut out, start_y as u16);
    out.extend_from_slice(&body);
    out
}

/// Writes `stack` as an OSF running the settings in `program`.
///
/// Without `previews` the machine shows black previews.
///
/// # Errors
///
/// [`WriteFailure::PreviewSize`] when a rendering is not the size this format
/// fixes, before any byte is written, and [`WriteFailure::Io`] when the file
/// cannot be written.
pub fn write_file(
    program: &PrintProgram,
    previews: Option<&PrintPreviews>,
    stack: &ContourStack,
    out: &mut dyn PrinterFileSink,
) -> Result<(), WriteFailure> {
    let (small, large) = previews_or_black(previews, PREVIEW_SIZES)?;
    let images = [
        rgb565(&small),
        rgb565(&downscale(&large, MIDDLE_SIZES[0])),
        rgb565(&downscale(&large, MIDDLE_SIZES[1])),
        rgb565(&large),
    ];
    let header_length = HEADER_BYTES
        + SETTINGS_BYTES
        + 3 * PREVIEW_SLOTS
        + images.iter().map(|image| image.len() as u32).sum::<u32>();
    let mut head = Vec::new();
    head.extend_from_slice(&header_length.to_be_bytes());
    u16_be(&mut head, VERSION);
    head.push(2);
    for image in &images {
        u24(&mut head, image.len() as u32);
        head.extend_from_slice(image);
    }
    head.extend_from_slice(&settings(program));
    debug_assert_eq!(head.len(), header_length as usize);
    out.write_all(&head)?;
    let width = program.raster.panel_resolution().x();
    write_layer_images(
        stack,
        &program.raster,
        2,
        |_, rows| encode_layer(rows, width),
        |_, bytes: Vec<u8>| -> Result<(), WriteFailure> {
            out.write_all(&bytes)?;
            Ok(())
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests_osf;
