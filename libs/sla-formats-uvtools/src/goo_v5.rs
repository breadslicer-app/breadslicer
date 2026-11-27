/*
 * SPDX-FileCopyrightText: 2020-2026 PTRTECH, Tiago Conceição (sn4k3) and UVtools contributors
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 *
 * Ported to Rust from UVtools UVtools.Core/FileFormats/GooV5File.cs (with the
 * preview packing of UVtools.Core/FileFormats/FileFormat.cs), UVtools v7.1.1,
 * commit 8b5c91fd41b60bd6265ba1c1f2b2b64636b873e8, modified for Breadslicer:
 * the layers are encoded from the slicer's run-length rows, the settings come
 * from a Breadslicer print program, and the file carries no extension table.
 *
 * This program is free software: you can redistribute it and/or modify it
 * under the terms of the GNU Affero General Public License as published by the
 * Free Software Foundation, either version 3 of the License, or (at your
 * option) any later version. It is distributed WITHOUT ANY WARRANTY; see the
 * GNU Affero General Public License (LICENSE.md) for details.
 */

//! Elegoo's GOO V5.1: `.goo` for the Jupiter 2 (#778).
//!
//! Little-endian. The GOO header (with 116 x 116 and 290 x 290 RGB565
//! previews) grows the addresses of 3 tables that follow it: one entry
//! (offset, size) per layer record, one per image, and one for the resin
//! block. Each layer's image is cut down the middle into 2 halves, each
//! encoded on its own in the 3-bit VUF codec, framed by `0x55` and a
//! checksum byte and followed by `0D 0A`. The file ends with the MD5 of
//! everything before it, as 32 lowercase hexadecimal digits.

use md5::{Digest, Md5};
use sc_sl1cer::contour::ContourStack;
use sc_sl1cer::raster::rle::RleRun;
use sc_sla_formats::layer_images::write_layer_images;
use sc_sla_formats::{
    LayerProgram, PreviewImage, PreviewSize, PreviewSizes, PrintPreviews, PrintProgram,
    PrinterFileSink, WriteFailure,
};

use crate::chitubox_zip::previews_or_black;
use crate::program::{ProgramSummary, pwm_byte, round_away, round_display, uses_per_layer_list};

/// The two previews the header carries.
pub const PREVIEW_SIZES: PreviewSizes = PreviewSizes {
    small: PreviewSize {
        width: 116,
        height: 116,
    },
    large: PreviewSize {
        width: 290,
        height: 290,
    },
};

const VERSION: &[u8; 4] = b"V5.1";
const MAGIC: [u8; 8] = [0x07, 0x00, 0x00, 0x00, 0x44, 0x4C, 0x50, 0x00];
const DELIMITER: [u8; 2] = [0x0D, 0x0A];
const SOFTWARE_NAME: &str = "Breadslicer";
const LAYER_MAGIC: u8 = 0x55;
const RESIN_MAGIC: u8 = 0x66;
const LDT_MAGIC: u8 = 0xA1;
const IEDT_MAGIC: u8 = 0xA2;
const RDT_MAGIC: u8 = 0xA3;
/// The halves each layer's image is cut into, side by side.
const PARTITIONS: u32 = 2;
/// The bits of grey each pixel keeps.
const PIXEL_BITS: u8 = 3;
const GREY_MAX: u8 = (1 << PIXEL_BITS) - 1;
const HEADER_BYTES: u32 = 195_496;
/// The header of GOO V3, which V5 extends; its length is kept as the
/// header's layer-content offset.
const V3_HEADER_BYTES: u32 = HEADER_BYTES - 19;
const LAYER_DEF_BYTES: u32 = 66;
const RESIN_BYTES: usize = 270;
/// The waits run as waits, not as a light-off delay.
const DELAY_MODE_WAIT_TIME: u8 = 1;

/// The 3-bit level a grey is stored at.
fn quantise(grey: u8) -> u8 {
    ((u32::from(grey) * u32::from(GREY_MAX) + 127) / 255) as u8
}

/// The grey a pixel reads back as once stored at 3 bits.
pub(crate) fn stored_grey(grey: u8) -> u8 {
    let level = u32::from(quantise(grey));
    let max = u32::from(GREY_MAX);
    ((level * 255 + max / 2) / max).min(255) as u8
}

fn put_f32(out: &mut Vec<u8>, value: f32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// A string in a field of `length` bytes, cut to leave room for its
/// terminating zero.
fn put_text(out: &mut Vec<u8>, text: &str, length: usize) {
    let mut bytes = text.as_bytes().to_vec();
    bytes.truncate(length - 1);
    bytes.resize(length, 0);
    out.extend_from_slice(&bytes);
}

fn rgb565(out: &mut Vec<u8>, image: &PreviewImage) {
    for pixel in image.rgb().as_chunks::<3>().0 {
        let (r, g, b) = (
            u16::from(pixel[0]),
            u16::from(pixel[1]),
            u16::from(pixel[2]),
        );
        put_u16(
            out,
            ((r & 0b1111_1000) << 8) | ((g & 0b1111_1100) << 3) | (b >> 3),
        );
    }
}

/// One set of motions, both stages, as the header and the layer records name
/// them: the second retract is the slow one onto the film.
#[derive(Clone, Copy, Debug, Default)]
struct Motion {
    exposure_sec: f32,
    wait_after_cure_sec: f32,
    wait_after_lift_sec: f32,
    wait_before_cure_sec: f32,
    lift_mm: f32,
    lift_mm_min: f32,
    lift2_mm: f32,
    lift2_mm_min: f32,
    retract_mm: f32,
    retract_mm_min: f32,
    retract2_mm: f32,
    retract2_mm_min: f32,
    pwm: u16,
}

impl Motion {
    fn of(layer: Option<&LayerProgram>) -> Self {
        let Some(layer) = layer else {
            return Self::default();
        };
        let r2 = |value: f64| round_away(value as f32, 2);
        let stage = |stage: Option<sc_sla_formats::MotionStage>| {
            stage.map_or((0.0, 0.0), |s| {
                (r2(s.distance_mm.value()), r2(s.speed_mm_min.value()))
            })
        };
        let (lift_mm, lift_mm_min) = stage(layer.lift);
        let (lift2_mm, lift2_mm_min) = stage(layer.second_lift);
        let (_, retract_mm_min) = stage(layer.retract);
        let (retract2_mm, retract2_mm_min) = stage(layer.second_retract);
        let total = round_away(lift_mm + lift2_mm, 2);
        let retract2_mm = retract2_mm.clamp(0.0, total);
        Self {
            exposure_sec: r2(layer.exposure_sec.value()),
            wait_after_cure_sec: r2(layer.wait_after_cure_sec.value()),
            wait_after_lift_sec: r2(layer.wait_after_lift_sec.value()),
            wait_before_cure_sec: r2(layer.wait_before_cure_sec.value()),
            lift_mm,
            lift_mm_min,
            lift2_mm,
            lift2_mm_min,
            retract_mm: round_away(total - retract2_mm, 2),
            retract_mm_min,
            retract2_mm,
            retract2_mm_min,
            pwm: u16::from(pwm_byte(layer.light_pwm)),
        }
    }
}

/// Where the program's bottom and normal settings come from: the first
/// layer, and the first one past the bottom and transition layers.
fn summary_layers(program: &PrintProgram) -> (Option<&LayerProgram>, Option<&LayerProgram>) {
    let first_normal = program
        .bottom_layer_count
        .saturating_add(program.transition_layer_count) as usize;
    let layers = &program.layers;
    (
        layers.first(),
        layers.get(first_normal).or_else(|| layers.last()),
    )
}

pub(crate) fn header(
    program: &PrintProgram,
    small: &PreviewImage,
    large: &PreviewImage,
    tables: [u32; 3],
) -> Vec<u8> {
    let raster = &program.raster;
    let panel = raster.panel_resolution();
    let (width_mm, height_mm) = raster.panel_size_mm();
    let mirror = raster.mirror();
    let (bottom_layer, normal_layer) = summary_layers(program);
    let (bottom, normal) = (Motion::of(bottom_layer), Motion::of(normal_layer));
    let summary = ProgramSummary::of(program);
    let per_layer = uses_per_layer_list(program, &summary);
    let mut out = Vec::with_capacity(HEADER_BYTES as usize);
    out.extend_from_slice(VERSION);
    out.extend_from_slice(&MAGIC);
    put_text(&mut out, SOFTWARE_NAME, 32);
    put_text(&mut out, program.slicer_version, 24);
    put_text(&mut out, program.created_at.rendered.as_str(), 24);
    put_text(&mut out, program.machine_name.as_str(), 32);
    put_text(&mut out, "DLP", 32);
    put_text(&mut out, program.material_name.as_str(), 32);
    put_u16(&mut out, u16::from(raster.aa().factor()));
    // The grey and blur levels.
    put_u16(&mut out, 1);
    put_u16(&mut out, 0);
    rgb565(&mut out, small);
    out.extend_from_slice(&DELIMITER);
    rgb565(&mut out, large);
    out.extend_from_slice(&DELIMITER);
    put_u32(&mut out, program.layers.len() as u32);
    put_u16(&mut out, panel.x() as u16);
    put_u16(&mut out, panel.y() as u16);
    out.push(u8::from(mirror.x));
    out.push(u8::from(mirror.y));
    put_f32(&mut out, round_display(width_mm as f32));
    put_f32(&mut out, round_display(height_mm as f32));
    put_f32(&mut out, round_away(raster.build_volume().z_mm() as f32, 2));
    put_f32(&mut out, program.layer_height_mm.value() as f32);
    put_f32(&mut out, normal.exposure_sec);
    out.push(DELAY_MODE_WAIT_TIME);
    // The light-off delay: the waits stand in for it.
    put_f32(&mut out, 0.0);
    put_f32(&mut out, bottom.wait_after_cure_sec);
    put_f32(&mut out, bottom.wait_after_lift_sec);
    put_f32(&mut out, bottom.wait_before_cure_sec);
    put_f32(&mut out, normal.wait_after_cure_sec);
    put_f32(&mut out, normal.wait_after_lift_sec);
    put_f32(&mut out, normal.wait_before_cure_sec);
    put_f32(&mut out, bottom.exposure_sec);
    put_u32(&mut out, program.bottom_layer_count);
    for value in [
        bottom.lift_mm,
        bottom.lift_mm_min,
        normal.lift_mm,
        normal.lift_mm_min,
        bottom.retract_mm,
        bottom.retract_mm_min,
        normal.retract_mm,
        normal.retract_mm_min,
        bottom.lift2_mm,
        bottom.lift2_mm_min,
        normal.lift2_mm,
        normal.lift2_mm_min,
        bottom.retract2_mm,
        bottom.retract2_mm_min,
        normal.retract2_mm,
        normal.retract2_mm_min,
    ] {
        put_f32(&mut out, value);
    }
    put_u16(&mut out, bottom.pwm);
    put_u16(&mut out, normal.pwm);
    out.push(u8::from(per_layer));
    put_u32(&mut out, program.print_time_sec.value() as u32);
    put_f32(&mut out, program.volume_mm3.value() as f32);
    put_f32(&mut out, round_away(program.mass_g.value() as f32, 3));
    // The cost and its currency.
    put_f32(&mut out, 0.0);
    put_text(&mut out, "$", 8);
    put_u32(&mut out, V3_HEADER_BYTES);
    // The grey scale level.
    out.push(1);
    put_u16(
        &mut out,
        u16::try_from(program.transition_layer_count).unwrap_or(u16::MAX),
    );
    out.push(PARTITIONS as u8);
    for address in tables {
        put_u32(&mut out, address);
    }
    // Anti-aliasing support, as UVtools leaves it, then the bits a pixel keeps.
    out.push(0);
    out.push(PIXEL_BITS);
    // No extension table.
    put_u32(&mut out, 0);
    debug_assert_eq!(out.len(), HEADER_BYTES as usize);
    out
}

fn layer_def(layer: &LayerProgram, park_z_mm: f32) -> Vec<u8> {
    let motion = Motion::of(Some(layer));
    let mut out = Vec::with_capacity(LAYER_DEF_BYTES as usize);
    // No pause.
    put_u16(&mut out, 0);
    put_f32(&mut out, park_z_mm);
    put_f32(&mut out, layer.slab_top_mm.value() as f32);
    for value in [
        motion.exposure_sec,
        // The light-off delay.
        0.0,
        motion.wait_after_cure_sec,
        motion.wait_after_lift_sec,
        motion.wait_before_cure_sec,
        motion.lift_mm,
        motion.lift_mm_min,
        motion.lift2_mm,
        motion.lift2_mm_min,
        motion.retract_mm,
        motion.retract_mm_min,
        motion.retract2_mm,
        motion.retract2_mm_min,
    ] {
        put_f32(&mut out, value);
    }
    put_u16(&mut out, motion.pwm);
    out.extend_from_slice(&DELIMITER);
    debug_assert_eq!(out.len(), LAYER_DEF_BYTES as usize);
    out
}

fn resin(program: &PrintProgram) -> Vec<u8> {
    let ml = program.volume_mm3.value() as f32 / 1000.0;
    let grams = round_away(program.mass_g.value() as f32, 3);
    let density = if ml > 0.0 && grams > 0.0 {
        grams / ml
    } else {
        1.0
    };
    let mut out = Vec::with_capacity(RESIN_BYTES);
    out.push(RESIN_MAGIC);
    put_text(&mut out, program.material_name.as_str(), 128);
    put_text(&mut out, "", 128);
    out.extend_from_slice(&[0x80, 0x80, 0x80]);
    put_f32(&mut out, density);
    // How sticky the resin is, at UVtools' default.
    put_f32(&mut out, 0.5);
    out.extend_from_slice(&DELIMITER);
    debug_assert_eq!(out.len(), RESIN_BYTES);
    out
}

/// Appends a run of the previous level.
fn run_chunk(out: &mut Vec<u8>, mut size: u32) {
    while size > 0 {
        let length = if size <= 0x10 {
            out.push(0x40 | (size - 1) as u8);
            size
        } else if size <= 0x1000 {
            let n = size - 1;
            out.extend_from_slice(&[0x50 | (n & 0xF) as u8, (n >> 4) as u8]);
            size
        } else if size <= 0x10_0000 {
            let n = size - 1;
            out.extend_from_slice(&[0x60 | (n & 0xF) as u8, (n >> 4) as u8, (n >> 12) as u8]);
            size
        } else {
            let length = size.min(0x1000_0000);
            let n = length - 1;
            out.extend_from_slice(&[
                0x70 | (n & 0xF) as u8,
                (n >> 4) as u8,
                (n >> 12) as u8,
                (n >> 20) as u8,
            ]);
            length
        };
        size -= length;
    }
}

/// Appends one pixel stepped from the previous level.
fn diff_chunk(out: &mut Vec<u8>, diff: i32) {
    // `0x80 | (32 + 32)` would read as a run.
    if diff == 32 {
        out.push(0xA0);
    } else {
        out.push(0x80 | (diff + 32) as u8);
    }
}

/// Appends up to 8 pixels of one level.
fn grey_chunk(out: &mut Vec<u8>, run: u32, level: u8) {
    let run = run.min(8);
    if run <= 7 && level < 7 {
        out.push(((run << 3) as u8) | level);
    } else if run <= 7 && level == GREY_MAX {
        out.push(((run << 3) as u8) | 0x07);
    } else {
        out.extend_from_slice(&[(run - 1) as u8, level]);
    }
}

/// Appends a run of `level`, `previous` being the level before it.
fn chunk(out: &mut Vec<u8>, run: u32, level: u8, previous: u8) {
    let diff = i32::from(level) - i32::from(previous);
    if diff == 0 {
        run_chunk(out, run);
    } else if run == 1 && (-32..=32).contains(&diff) {
        diff_chunk(out, diff);
    } else if run <= 7 {
        // 3 bits a pixel: every level fits a grey chunk.
        grey_chunk(out, run, level);
    } else {
        diff_chunk(out, diff);
        run_chunk(out, run - 1);
    }
}

/// One half of a layer's image as a framed VUF block: `0x55`, the chunks,
/// then the complement of the chunks' byte sum.
pub(crate) fn encode_partition(runs: &[(u8, u32)]) -> Vec<u8> {
    let mut out = vec![LAYER_MAGIC];
    let mut previous = 0_u8;
    let mut level = runs.first().map_or(0, |&(grey, _)| quantise(grey));
    let mut run = 0_u32;
    for &(grey, length) in runs {
        let q = quantise(grey);
        if q == level {
            run += length;
            continue;
        }
        chunk(&mut out, run, level, previous);
        previous = level;
        level = q;
        run = length;
    }
    if run > 0 {
        chunk(&mut out, run, level, previous);
    }
    let sum = out[1..].iter().fold(0_u8, |sum, &b| sum.wrapping_add(b));
    out.push(!sum);
    out
}

/// A layer's image cut into its halves, each as the runs of its rows read
/// one after the other.
fn halves(rows: &[RleRun], width: u32) -> [Vec<(u8, u32)>; 2] {
    let half = width / PARTITIONS;
    let mut parts: [Vec<(u8, u32)>; 2] = [Vec::new(), Vec::new()];
    let mut x = 0_u32;
    let push = |part: &mut Vec<(u8, u32)>, grey: u8, length: u32| match part.last_mut() {
        Some((last, run)) if *last == grey => *run += length,
        _ => part.push((grey, length)),
    };
    for run in rows {
        let mut left = run.length;
        while left > 0 {
            let side = (x / half) as usize;
            let room = half * (side as u32 + 1) - x;
            let take = left.min(room);
            push(&mut parts[side], run.value, take);
            x = (x + take) % width;
            left -= take;
        }
    }
    parts
}

/// The 2 framed image blocks of one layer.
pub(crate) fn encode_layer(rows: &[RleRun], width: u32) -> [Vec<u8>; 2] {
    let [left, right] = halves(rows, width);
    [encode_partition(&left), encode_partition(&right)]
}

/// Writes `stack` as a GOO V5.1 running the settings in `program`.
///
/// Without `previews` the machine shows black previews.
///
/// # Errors
///
/// [`WriteFailure::Unencodable`] when the panel's width does not split into
/// 2 halves, [`WriteFailure::PreviewSize`] when a rendering is not the size
/// this format fixes, both before any byte is written, and
/// [`WriteFailure::Io`] when the file cannot be written.
pub fn write_file(
    program: &PrintProgram,
    previews: Option<&PrintPreviews>,
    stack: &ContourStack,
    out: &mut dyn PrinterFileSink,
) -> Result<(), WriteFailure> {
    let width = program.raster.panel_resolution().x();
    if !width.is_multiple_of(PARTITIONS) {
        return Err(WriteFailure::Unencodable(format!(
            "a GOO V5 layer is cut into 2 halves, and {width} pixels across is odd"
        )));
    }
    let (small, large) = previews_or_black(previews, PREVIEW_SIZES)?;
    let layers = program.layers.len() as u32;
    let images = layers * PARTITIONS;
    let ldt = HEADER_BYTES;
    let iedt = ldt + 1 + layers * 8;
    let rdt = iedt + 1 + images * 8;
    let content = rdt + 1 + 4 + 8;

    // Everything after the tables is laid out first, so the tables can name
    // where each piece lies.
    let park_z = round_away(program.raster.build_volume().z_mm() as f32, 2);
    let mut body = Vec::new();
    let mut layer_entries = Vec::with_capacity(layers as usize);
    for layer in &program.layers {
        layer_entries.push((content + body.len() as u32, LAYER_DEF_BYTES));
        body.extend_from_slice(&layer_def(layer, park_z));
    }
    let mut image_entries = Vec::with_capacity(images as usize);
    write_layer_images(
        stack,
        &program.raster,
        19,
        |_, rows| encode_layer(rows, width),
        |_, blocks: [Vec<u8>; 2]| -> Result<(), WriteFailure> {
            for block in blocks {
                image_entries.push((content + body.len() as u32, block.len() as u32 + 2));
                body.extend_from_slice(&block);
                body.extend_from_slice(&DELIMITER);
            }
            Ok(())
        },
    )?;
    let resin_at = content + body.len() as u32;
    let resin = resin(program);
    body.extend_from_slice(&resin);

    let mut file = header(program, &small, &large, [ldt, iedt, rdt]);
    file.push(LDT_MAGIC);
    for (offset, size) in layer_entries {
        put_u32(&mut file, offset);
        put_u32(&mut file, size);
    }
    file.push(IEDT_MAGIC);
    for (offset, size) in image_entries {
        put_u32(&mut file, offset);
        put_u32(&mut file, size);
    }
    file.push(RDT_MAGIC);
    put_u32(&mut file, 1);
    put_u32(&mut file, resin_at);
    put_u32(&mut file, resin.len() as u32);
    debug_assert_eq!(file.len(), content as usize);
    file.extend_from_slice(&body);
    let digest = Md5::digest(&file);
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    file.extend_from_slice(hex.as_bytes());
    out.write_all(&file)?;
    Ok(())
}

#[cfg(test)]
mod tests_goo_v5;
