/*
 * SPDX-FileCopyrightText: 2020-2026 PTRTECH, Tiago Conceição (sn4k3) and UVtools contributors
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 *
 * Ported to Rust from UVtools UVtools.Core/FileFormats/FDGFile.cs (with the
 * preview codec of UVtools.Core/FileFormats/ChituboxFile.cs), UVtools v7.1.1,
 * commit 8b5c91fd41b60bd6265ba1c1f2b2b64636b873e8, modified for Breadslicer:
 * the layers are encoded from the slicer's run-length rows, the header states
 * the normal light power where UVtools overwrites the bottom one with it, and
 * the settings come from a Breadslicer print program.
 *
 * This program is free software: you can redistribute it and/or modify it
 * under the terms of the GNU Affero General Public License as published by the
 * Free Software Foundation, either version 3 of the License, or (at your
 * option) any later version. It is distributed WITHOUT ANY WARRANTY; see the
 * GNU Affero General Public License (LICENSE.md) for details.
 */

//! Voxelab's FDG: `.fdg` for the Ceres 8.9, Polaris 5.5 and Proxima 6 (#778).
//!
//! A ChiTuBox-like binary: a 216-byte header, the 2 previews in ChiTuBox's
//! 15-bit run-length code, the machine's name, one 36-byte record per layer
//! (height, exposure, light-off delay, where its image is) and the layer
//! images. A layer image runs row by row in 7-bit greys, a byte with the top
//! bit set naming a grey and the bytes after it how many more pixels take it
//! (125 at most each); a run breaks at the middle of every row. Identical
//! layers share one image.

use std::collections::HashMap;

use sc_sl1cer::contour::ContourStack;
use sc_sl1cer::raster::rle::RleRun;
use sc_sla_formats::ctb::rle15;
use sc_sla_formats::layer_images::write_layer_images;
use sc_sla_formats::{
    PreviewSize, PreviewSizes, PrintPreviews, PrintProgram, PrinterFileSink, WriteFailure,
};

use crate::chitubox_zip::previews_or_black;
use crate::program::{ProgramSummary, StageSettings, round_away, round_display, round_height};

/// The 2 previews: 200 x 125 and 400 x 300.
pub const PREVIEW_SIZES: PreviewSizes = PreviewSizes {
    small: PreviewSize {
        width: 200,
        height: 125,
    },
    large: PreviewSize {
        width: 400,
        height: 300,
    },
};

pub(crate) const MAGIC: u32 = 0xBD3C_7AC8;
const HEADER_LENGTH: u64 = 216;
const PREVIEW_TABLE_LENGTH: u64 = 32;
const LAYER_RECORD_LENGTH: u64 = 36;
const PAGE: u64 = 1 << 32;
/// The software version UVtools states.
const SOFTWARE_VERSION: u32 = 0x0106_0300;

/// The 7-bit grey a pixel is stored at, as the machine reads it back.
pub(crate) fn stored_grey(grey: u8) -> u8 {
    let grey7 = (grey >> 1).min(0x7c);
    let back = (grey7 << 1) | (grey7 & 1);
    if back >= 0xfc { 0xff } else { back }
}

/// Encodes one layer image, `width` pixels a row.
pub(crate) fn encode_layer(runs: &[RleRun], width: u32) -> Vec<u8> {
    fn add(out: &mut Vec<u8>, stride: u32, color: u8) {
        out.push(color | 0x80);
        let mut left = stride - 1;
        while left > 0 {
            let todo = left.min(0x7d);
            out.push(todo as u8);
            left -= todo;
        }
    }
    let half = width / 2;
    let mut out = Vec::new();
    let mut color: Option<u8> = None;
    let mut stride = 0_u32;
    let mut x = 0_u32;
    for run in runs {
        let grey7 = (run.value >> 1).min(0x7c);
        for _ in 0..run.length {
            match color {
                None => {
                    color = Some(grey7);
                    stride = 1;
                }
                Some(current) if current != grey7 || x == half => {
                    add(&mut out, stride, current);
                    color = Some(grey7);
                    stride = 1;
                }
                Some(_) => stride += 1,
            }
            x += 1;
            if x == width {
                if let Some(current) = color {
                    add(&mut out, stride, current);
                }
                color = None;
                x = 0;
            }
        }
    }
    out
}

/// The light-off delay a layer states: the time the platform takes to rise
/// and come back, plus the wait before the exposure.
fn light_off_sec(stage: &StageSettings) -> f32 {
    let lift = round_height(stage.total_lift_mm());
    let mut seconds = round_away(stage.wait_before_cure_sec, 2);
    if lift > 0.0 && stage.lift_mm_min > 0.0 {
        seconds += lift / (stage.lift_mm_min / 60.0);
    }
    if lift > 0.0 && stage.retract_mm_min > 0.0 {
        seconds += lift / (stage.retract_mm_min / 60.0);
    }
    round_away(seconds, 2)
}

struct Offsets {
    small: u32,
    large: u32,
    machine_name: u32,
    layers: u32,
}

fn header(program: &PrintProgram, offsets: &Offsets) -> Vec<u8> {
    let summary = ProgramSummary::of(program);
    let (b, n) = (summary.bottom, summary.normal);
    let raster = &program.raster;
    let panel = raster.panel_resolution();
    let (width_mm, height_mm) = raster.panel_size_mm();
    let mirror = raster.mirror();
    let print_height = program
        .layers
        .last()
        .map_or(0.0, |l| round_height(l.slab_top_mm.value() as f32));
    let minutes = (program.created_at.unix_sec / 60) as u32;
    let mut out = Vec::with_capacity(HEADER_LENGTH as usize);
    let u = |out: &mut Vec<u8>, value: u32| out.extend_from_slice(&value.to_le_bytes());
    let f = |out: &mut Vec<u8>, value: f32| out.extend_from_slice(&value.to_le_bytes());
    u(&mut out, MAGIC);
    u(&mut out, 2);
    u(&mut out, program.layers.len() as u32);
    u(&mut out, program.bottom_layer_count);
    u(&mut out, u32::from(mirror.x || mirror.y));
    u(&mut out, program.bottom_layer_count);
    u(&mut out, panel.x());
    u(&mut out, panel.y());
    f(
        &mut out,
        round_height(program.layer_height_mm.value() as f32),
    );
    f(&mut out, round_away(n.exposure_sec, 2));
    f(&mut out, round_away(b.exposure_sec, 2));
    u(&mut out, offsets.large);
    u(&mut out, offsets.small);
    u(&mut out, offsets.layers);
    u(&mut out, program.print_time_sec.value() as u32);
    u(&mut out, 1);
    out.extend_from_slice(&u16::from(n.pwm).to_le_bytes());
    out.extend_from_slice(&u16::from(b.pwm).to_le_bytes());
    u(&mut out, 0);
    u(&mut out, 0);
    f(&mut out, print_height);
    f(&mut out, round_display(width_mm as f32));
    f(&mut out, round_display(height_mm as f32));
    f(
        &mut out,
        round_away(raster.build_volume().z().value() as f32, 2),
    );
    u(&mut out, 0);
    u(&mut out, raster.aa().factor().into());
    u(&mut out, 0x4c);
    f(
        &mut out,
        round_away((program.volume_mm3.value() / 1000.0) as f32, 3),
    );
    f(&mut out, round_away(program.mass_g.value() as f32, 3));
    f(&mut out, 0.0);
    u(&mut out, offsets.machine_name);
    u(&mut out, program.machine_name.as_str().len() as u32);
    f(&mut out, light_off_sec(&b));
    f(&mut out, light_off_sec(&n));
    u(&mut out, 0);
    f(&mut out, round_away(b.total_lift_mm(), 2));
    f(&mut out, round_away(b.lift_mm_min, 2));
    f(&mut out, round_away(n.total_lift_mm(), 2));
    f(&mut out, round_away(n.lift_mm_min, 2));
    f(&mut out, round_away(n.retract_mm_min, 2));
    for _ in 0..7 {
        u(&mut out, 0);
    }
    u(&mut out, minutes);
    u(&mut out, SOFTWARE_VERSION);
    for _ in 0..6 {
        u(&mut out, 0);
    }
    debug_assert_eq!(out.len() as u64, HEADER_LENGTH);
    out
}

fn preview_block(width: u32, height: u32, at: u64, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(PREVIEW_TABLE_LENGTH as usize + data.len());
    for value in [
        width,
        height,
        (at + PREVIEW_TABLE_LENGTH) as u32,
        data.len() as u32,
        0,
        0,
        0,
        0,
    ] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(data);
    out
}

/// Writes `stack` as an FDG running the settings in `program`.
///
/// # Errors
///
/// [`WriteFailure::PreviewSize`] when a rendering is not the size this format
/// fixes, raised before any byte is written, [`WriteFailure::Unencodable`]
/// when the file would pass the 4 GB its offsets reach, and
/// [`WriteFailure::Io`] when the file cannot be written.
pub fn write_file(
    program: &PrintProgram,
    previews: Option<&PrintPreviews>,
    stack: &ContourStack,
    out: &mut dyn PrinterFileSink,
) -> Result<(), WriteFailure> {
    let (small, large) = previews_or_black(previews, PREVIEW_SIZES)?;
    let mut small_blob = Vec::new();
    rle15::encode_preview(&small, &mut small_blob);
    let mut large_blob = Vec::new();
    rle15::encode_preview(&large, &mut large_blob);
    let width = program.raster.panel_resolution().x();

    // Encode every layer first: the records ahead of the images say where
    // each image starts.
    let mut images: Vec<Vec<u8>> = Vec::new();
    let mut image_of_layer: Vec<usize> = Vec::with_capacity(program.layers.len());
    let mut seen: HashMap<Vec<u8>, usize> = HashMap::new();
    write_layer_images(
        stack,
        &program.raster,
        2,
        |_, rows| encode_layer(rows, width),
        |_, bytes: Vec<u8>| -> Result<(), WriteFailure> {
            let index = match seen.get(&bytes) {
                Some(&index) => index,
                None => {
                    images.push(bytes.clone());
                    seen.insert(bytes, images.len() - 1);
                    images.len() - 1
                }
            };
            image_of_layer.push(index);
            Ok(())
        },
    )?;
    drop(seen);

    let small_at = HEADER_LENGTH;
    let large_at = small_at + PREVIEW_TABLE_LENGTH + small_blob.len() as u64;
    let name_at = large_at + PREVIEW_TABLE_LENGTH + large_blob.len() as u64;
    let records_at = name_at + program.machine_name.as_str().len() as u64;
    let mut data_at = records_at + LAYER_RECORD_LENGTH * image_of_layer.len() as u64;
    if records_at > u64::from(u32::MAX) {
        return Err(WriteFailure::Unencodable(
            "the file passes the 4 GB its offsets reach".to_owned(),
        ));
    }
    let mut image_at = Vec::with_capacity(images.len());
    for image in &images {
        image_at.push(data_at);
        data_at += image.len() as u64;
    }

    let offsets = Offsets {
        small: small_at as u32,
        large: large_at as u32,
        machine_name: name_at as u32,
        layers: records_at as u32,
    };
    out.write_all(&header(program, &offsets))?;
    out.write_all(&preview_block(
        small.width(),
        small.height(),
        small_at,
        &small_blob,
    ))?;
    out.write_all(&preview_block(
        large.width(),
        large.height(),
        large_at,
        &large_blob,
    ))?;
    out.write_all(program.machine_name.as_str().as_bytes())?;
    for (layer, &image) in program.layers.iter().zip(&image_of_layer) {
        let stage = StageSettings::of(Some(layer));
        let at = image_at[image];
        let mut record = Vec::with_capacity(LAYER_RECORD_LENGTH as usize);
        record.extend_from_slice(&round_height(layer.slab_top_mm.value() as f32).to_le_bytes());
        record.extend_from_slice(&round_away(stage.exposure_sec, 2).to_le_bytes());
        record.extend_from_slice(&light_off_sec(&stage).to_le_bytes());
        for value in [
            (at % PAGE) as u32,
            images[image].len() as u32,
            (at / PAGE) as u32,
            84,
            0,
            0,
        ] {
            record.extend_from_slice(&value.to_le_bytes());
        }
        out.write_all(&record)?;
    }
    for image in &images {
        out.write_all(image)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests_fdg;
