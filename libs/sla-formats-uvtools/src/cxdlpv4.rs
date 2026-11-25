/*
 * SPDX-FileCopyrightText: 2020-2026 PTRTECH, Tiago Conceição (sn4k3) and UVtools contributors
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 *
 * Ported to Rust from UVtools UVtools.Core/FileFormats/CrealityCXDLPv4File.cs
 * (with the layer codecs of UVtools.Core/FileFormats/CtbRleCodec.cs and
 * UVtools.Core/FileFormats/PixelPairRleCodec.cs), UVtools v7.1.1, commit
 * 8b5c91fd41b60bd6265ba1c1f2b2b64636b873e8, modified for Breadslicer: the
 * layers are encoded from the slicer's run-length rows on every core, the
 * largest-outline areas come from the sliced contours, the checksum is
 * carried along as the file is written, and the print settings come from a
 * Breadslicer print program.
 *
 * This program is free software: you can redistribute it and/or modify it
 * under the terms of the GNU Affero General Public License as published by the
 * Free Software Foundation, either version 3 of the License, or (at your
 * option) any later version. It is distributed WITHOUT ANY WARRANTY; see the
 * GNU Affero General Public License (LICENSE.md) for details.
 */

//! Creality's CXDLP version 4 (`.cxdlpv4`: Halot Mage, Mage Pro, Mage S,
//! Play, R6, and the Halot X1 family).
//!
//! The file has the CTB skeleton, little-endian, under the CXDLP magic:
//!
//! 1. A header: magic `CXSW3DV2` and version 4 (big-endian), the printer model
//!    as a big-endian length then the text with its terminating zero, then the
//!    panel, the plate, the offsets of every section, the light's PWMs.
//! 2. The small then the large preview, each a 32-byte record and its image in
//!    the CTB RGB15 run-length codec.
//! 3. The print parameters (68 bytes) and the slicer info (76 bytes).
//! 4. A 40-byte record per layer: its height, exposure, where its data is and
//!    its largest outline area (mm² x 1000).
//! 5. Each layer's data: a 44-byte record of its motion, then its image.
//! 6. A CRC-32 of everything before it (zero start, no final inversion),
//!    big-endian.
//!
//! The layer images use one of 2 codecs, which no header field names: the CTB
//! 7-bit run-length codec, or for the Halot X1 family a codec of 3-bit grey
//! pixel pairs, row by row.

use std::io::{self, SeekFrom, Write};

use sc_sl1cer::contour::ContourStack;
use sc_sl1cer::raster::rle::RleRun;
use sc_sla_formats::ctb::v5::preview_record::write_preview_record;
use sc_sla_formats::ctb::{rle7, rle15};
use sc_sla_formats::layer_images::write_layer_images;
use sc_sla_formats::{
    LayerProgram, PreviewImage, PreviewSize, PreviewSizes, PrintPreviews, PrintProgram,
    PrinterFileSink, WriteFailure,
};

use crate::cxdlp::{crc32_zero_start, printer_model as legacy_model};
use crate::layer_stats::ContourStats;
use crate::program::{
    ProgramSummary, StageSettings, round_away, round_display, round_height, slabs,
    uses_per_layer_settings,
};

/// The magic, written with its terminating zero.
const MAGIC: &[u8; 9] = b"CXSW3DV2\0";

const VERSION: u16 = 4;

/// The sizes of the 2 previews.
pub const PREVIEW_SIZES: PreviewSizes = PreviewSizes {
    small: PreviewSize {
        width: 120,
        height: 120,
    },
    large: PreviewSize {
        width: 300,
        height: 300,
    },
};

const PREVIEW_RECORD_BYTES: u32 = 32;
const PRINT_PARAMETERS_BYTES: u32 = 68;
const SLICER_INFO_BYTES: u32 = 76;
const LAYER_DEF_BYTES: u32 = 40;
const LAYER_DEF_EX_BYTES: u32 = 44;

/// The model the Halot X1 family's files name.
const HALOT_X1: &str = "HALOT-X1";

/// The model codes of the machines Breadslicer ships profiles for, by the
/// name the profile gives the machine, as UVtools' own profiles name them.
const PROFILE_MODELS: [(&str, &str); 2] = [("Halot Mage", "CL-103L"), ("Halot Mage Pro", "CL-103")];

/// The layer image codec.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerCodec {
    /// The CTB 7-bit grey run-length codec.
    Rle7,
    /// Runs of 3-bit grey pixel pairs, each row on its own.
    PixelPairs,
}

/// Finds the model the machine checks a file against: a Halot X1 of any kind
/// names `HALOT-X1`, a Breadslicer profile name gives its code, and otherwise
/// a code in the name or the name itself, as UVtools does.
pub(crate) fn printer_model(machine_name: &str) -> String {
    let name = machine_name.trim();
    if is_halot_x1(name) {
        return HALOT_X1.to_owned();
    }
    if let Some((_, code)) = PROFILE_MODELS
        .iter()
        .find(|(profile, _)| profile.eq_ignore_ascii_case(name))
    {
        return (*code).to_owned();
    }
    // The CXDLP version 2 and 3 lookup picks codes out of a name; it falls back
    // to the panel, which version 4 does not, so the resolution here matches
    // nothing.
    legacy_model(name, (0, 0)).unwrap_or_else(|| name.to_owned())
}

/// Whether a name is a Halot X1's, as `HALOT[- ]?X1` matches it.
fn is_halot_x1(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    ["HALOT-X1", "HALOT X1", "HALOTX1"]
        .iter()
        .any(|pattern| upper.contains(pattern))
}

/// The codec a machine's layers take: the pixel pairs for the Halot X1 family
/// (by its model, `R009`, or its panels), the CTB codec otherwise and on any
/// odd-width panel.
pub fn codec_for(model: &str, resolution: (u32, u32)) -> LayerCodec {
    if resolution.0 % 2 != 0 {
        return LayerCodec::Rle7;
    }
    if model.eq_ignore_ascii_case(HALOT_X1)
        || model.eq_ignore_ascii_case("R009")
        || is_pixel_pair_panel(resolution)
    {
        LayerCodec::PixelPairs
    } else {
        LayerCodec::Rle7
    }
}

/// The panels of the Halot X1 family: the X1 and X1 Max, and the X1 Neo.
pub(crate) fn is_pixel_pair_panel(resolution: (u32, u32)) -> bool {
    matches!(resolution, (15120, 6230) | (8520, 4320))
}

/// The grey a pixel of `grey` reads back as from a pixel-pair image: its top
/// 3 bits spread back over the byte.
pub(crate) fn pixel_pair_grey(grey: u8) -> u8 {
    let gray3 = grey >> 5;
    (gray3 << 5) | (gray3 << 2) | (gray3 >> 1)
}

/// The dimmest grey a pixel-pair image stores lit.
const PIXEL_PAIR_DIMMEST: u8 = 32;

/// Encodes a frame of `runs`, `width` pixels per row, as pixel-pair runs.
pub(crate) fn encode_pixel_pairs(runs: &[RleRun], width: u32, out: &mut Vec<u8>) {
    let width = width as usize;
    let units_per_row = width / 2;
    let mut row = vec![0_u8; width];
    let mut filled = 0_usize;
    let flush_row = |row: &[u8], out: &mut Vec<u8>| {
        let mut unit = 0;
        while unit < units_per_row {
            let value = pair_unit(row, unit);
            let mut count = 1;
            while unit + count < units_per_row && pair_unit(row, unit + count) == value {
                count += 1;
            }
            push_pair_unit(out, value, count);
            unit += count;
        }
    };
    for run in runs {
        let mut left = run.length as usize;
        while left > 0 {
            let take = left.min(width - filled);
            row[filled..filled + take].fill(run.value);
            filled += take;
            left -= take;
            if filled == width {
                flush_row(&row, out);
                filled = 0;
            }
        }
    }
}

fn pair_unit(row: &[u8], unit: usize) -> u8 {
    0x80 | (row[unit * 2] >> 5) | ((row[unit * 2 + 1] >> 5) << 3)
}

fn push_pair_unit(out: &mut Vec<u8>, unit: u8, count: usize) {
    out.push(unit);
    let repeat = count - 1;
    if repeat == 0 {
        return;
    }
    let mut groups = 1;
    let mut rest = repeat >> 7;
    while rest != 0 {
        groups += 1;
        rest >>= 7;
    }
    for group in (0..groups).rev() {
        out.push(((repeat >> (group * 7)) & 0x7f) as u8);
    }
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_f32(out: &mut Vec<u8>, value: f32) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// One layer's settings as this format stores them, speeds in millimetres per
/// minute, every value rounded to 2 decimals as UVtools rounds them.
#[derive(Clone, Copy, Debug, Default)]
struct Motion {
    exposure_sec: f32,
    total_lift_mm: f32,
    lift_mm_min: f32,
    second_lift_mm: f32,
    second_lift_mm_min: f32,
    retract_mm_min: f32,
    second_retract_mm: f32,
    second_retract_mm_min: f32,
    wait_before_cure_sec: f32,
    wait_after_cure_sec: f32,
    wait_after_lift_sec: f32,
    pwm: u8,
}

impl Motion {
    fn of(stage: &StageSettings, layer: Option<&LayerProgram>) -> Self {
        let second_retract = layer.and_then(|layer| layer.second_retract);
        let (second_lift_mm, second_lift_mm_min) = stage
            .second_stage
            .map_or((0.0, 0.0), |(distance, lift, _)| (distance, lift));
        Self {
            exposure_sec: round_away(stage.exposure_sec, 2),
            total_lift_mm: round_away(stage.total_lift_mm(), 2),
            lift_mm_min: round_away(stage.lift_mm_min, 2),
            second_lift_mm: round_away(second_lift_mm, 2),
            second_lift_mm_min: round_away(second_lift_mm_min, 2),
            retract_mm_min: round_away(stage.retract_mm_min, 2),
            second_retract_mm: round_away(
                second_retract.map_or(0.0, |stage| stage.distance_mm.value() as f32),
                2,
            ),
            second_retract_mm_min: round_away(
                second_retract.map_or(0.0, |stage| stage.speed_mm_min.value() as f32),
                2,
            ),
            wait_before_cure_sec: round_away(stage.wait_before_cure_sec, 2),
            wait_after_cure_sec: round_away(stage.wait_after_cure_sec, 2),
            wait_after_lift_sec: round_away(stage.wait_after_lift_sec, 2),
            pwm: stage.pwm,
        }
    }

    fn of_layer(layer: Option<&LayerProgram>) -> Self {
        Self::of(&StageSettings::of(layer), layer)
    }
}

/// Where each section starts.
#[derive(Clone, Copy, Debug)]
struct Layout {
    small_preview: u32,
    large_preview: u32,
    print_parameters: u32,
    slicer_info: u32,
    layer_table: u32,
    layer_data: u64,
}

/// Wraps the output, carrying the checksum of every byte written through it.
struct Checksummed<'w> {
    out: &'w mut dyn PrinterFileSink,
    crc: u32,
    written: u64,
}

impl Write for Checksummed<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.out.write(buf)?;
        self.crc = crc32_zero_start(self.crc, &buf[..written]);
        self.written += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

/// Moves a checksum `length` zero bytes on, which is what the bytes after a
/// span of zeros owe to the bytes before it: the CRC register is linear, so
/// `crc(A ‖ B) = shift(crc(A), |B|) ⊕ crc(B)`.
fn crc32_shift(crc: u32, mut length: u64) -> u32 {
    // The operator of 1 zero bit, then squared to 2, 4, ... bits.
    let mut odd = [0_u32; 32];
    odd[0] = 0xedb8_8320;
    let mut row = 1;
    for slot in odd.iter_mut().skip(1) {
        *slot = row;
        row <<= 1;
    }
    let mut even = [0_u32; 32];
    gf2_square(&mut even, &odd); // 2 zero bits
    gf2_square(&mut odd, &even); // 4 zero bits

    let mut crc = crc;
    if length == 0 {
        return crc;
    }
    loop {
        gf2_square(&mut even, &odd);
        if length & 1 != 0 {
            crc = gf2_times(&even, crc);
        }
        length >>= 1;
        if length == 0 {
            break;
        }
        gf2_square(&mut odd, &even);
        if length & 1 != 0 {
            crc = gf2_times(&odd, crc);
        }
        length >>= 1;
        if length == 0 {
            break;
        }
    }
    crc
}

fn gf2_times(matrix: &[u32; 32], mut vector: u32) -> u32 {
    let mut sum = 0;
    let mut index = 0;
    while vector != 0 {
        if vector & 1 != 0 {
            sum ^= matrix[index];
        }
        vector >>= 1;
        index += 1;
    }
    sum
}

fn gf2_square(square: &mut [u32; 32], matrix: &[u32; 32]) {
    for (index, slot) in square.iter_mut().enumerate() {
        *slot = gf2_times(matrix, matrix[index]);
    }
}

/// Writes `stack` as a CXDLP version 4 file running the settings in
/// `program`.
///
/// Every layer image is the plate's image that [`write_layer_images`]
/// describes, as in the other formats; the raster setup's mirror flags turn it
/// around and its panel rotation turns it a quarter. Without `previews` the
/// machine shows black thumbnails.
///
/// # Errors
///
/// [`WriteFailure::PreviewSize`] when a rendering is not the size this format
/// fixes and [`WriteFailure::Unencodable`] for a panel or a file beyond what
/// the format can address, before any byte is written; [`WriteFailure::Io`]
/// when it cannot be written.
pub fn write_file(
    program: &PrintProgram,
    previews: Option<&PrintPreviews>,
    stack: &ContourStack,
    out: &mut dyn PrinterFileSink,
) -> Result<(), WriteFailure> {
    let resolution = program.raster.panel_resolution();
    let (res_x, res_y) = (resolution.x(), resolution.y());
    let unreachable = || {
        WriteFailure::Unencodable(format!(
            "the file passes the {} bytes its 32-bit offsets reach",
            u32::MAX
        ))
    };
    u16::try_from(res_x).map_err(|_| {
        WriteFailure::Unencodable(format!("a {res_x} pixel wide panel is beyond CXDLP v4"))
    })?;
    u16::try_from(res_y).map_err(|_| {
        WriteFailure::Unencodable(format!("a {res_y} pixel tall panel is beyond CXDLP v4"))
    })?;
    let model = printer_model(program.machine_name.as_str());
    let codec = codec_for(&model, (res_x, res_y));

    let (small, large) = match previews {
        Some(previews) => {
            previews.small.check_size(PREVIEW_SIZES.small)?;
            previews.large.check_size(PREVIEW_SIZES.large)?;
            (previews.small.clone(), previews.large.clone())
        }
        None => (
            PreviewImage::black(PREVIEW_SIZES.small),
            PreviewImage::black(PREVIEW_SIZES.large),
        ),
    };
    let mut small_blob = Vec::new();
    rle15::encode_preview(&small, &mut small_blob);
    let mut large_blob = Vec::new();
    rle15::encode_preview(&large, &mut large_blob);

    let layer_count = u32::try_from(program.layers.len()).map_err(|_| unreachable())?;
    let header_bytes = 100 + model.len() as u32;
    let small_preview = header_bytes;
    let large_preview = small_preview
        + PREVIEW_RECORD_BYTES
        + u32::try_from(small_blob.len()).map_err(|_| unreachable())?;
    let print_parameters = large_preview
        + PREVIEW_RECORD_BYTES
        + u32::try_from(large_blob.len()).map_err(|_| unreachable())?;
    let slicer_info = print_parameters + PRINT_PARAMETERS_BYTES;
    let layer_table = slicer_info + SLICER_INFO_BYTES;
    let layout = Layout {
        small_preview,
        large_preview,
        print_parameters,
        slicer_info,
        layer_table,
        layer_data: u64::from(layer_table) + u64::from(LAYER_DEF_BYTES) * u64::from(layer_count),
    };
    if layout.layer_data > u64::from(u32::MAX) {
        return Err(unreachable());
    }

    let summary = ProgramSummary::of(program);
    let bottom = Motion::of(&summary.bottom, program.layers.first());
    let first_normal = program
        .bottom_layer_count
        .saturating_add(program.transition_layer_count) as usize;
    let normal = Motion::of(
        &summary.normal,
        program
            .layers
            .get(first_normal)
            .or_else(|| program.layers.last()),
    );

    let mut sections = header(program, &model, &layout, (&bottom, &normal));
    debug_assert_eq!(sections.len() as u32, header_bytes);
    write_preview_record(
        &small,
        &small_blob,
        layout.small_preview + PREVIEW_RECORD_BYTES,
        &mut sections,
    )?;
    sections.extend_from_slice(&small_blob);
    write_preview_record(
        &large,
        &large_blob,
        layout.large_preview + PREVIEW_RECORD_BYTES,
        &mut sections,
    )?;
    sections.extend_from_slice(&large_blob);
    sections.extend(print_parameters_block(program, (&bottom, &normal)));
    sections.extend(slicer_info_block(program, (&bottom, &normal)));
    debug_assert_eq!(sections.len() as u32, layout.layer_table);

    let start = out.stream_position()?;
    let head_crc = crc32_zero_start(0, &sections);
    out.write_all(&sections)?;
    let table_bytes = LAYER_DEF_BYTES as usize * program.layers.len();
    out.write_all(&vec![0; table_bytes])?;

    let largest_outlines: Vec<u32> = stack
        .layers()
        .iter()
        .map(|layer| (ContourStats::of(layer).largest_outline_mm2 * 1000.0) as u32)
        .collect();
    let tops = slabs(program);
    let mut table = Vec::with_capacity(table_bytes);
    let mut data = Checksummed {
        out,
        crc: 0,
        written: 0,
    };
    let mut next = layout.layer_data;
    let dimmest = match codec {
        LayerCodec::Rle7 => rle7::DIMMEST_STORED,
        LayerCodec::PixelPairs => PIXEL_PAIR_DIMMEST,
    };
    write_layer_images(
        stack,
        &program.raster,
        dimmest,
        |_, runs| {
            let mut image = Vec::new();
            match codec {
                LayerCodec::Rle7 => rle7::encode_layer(runs, &mut image),
                LayerCodec::PixelPairs => encode_pixel_pairs(runs, res_x, &mut image),
            }
            image
        },
        |index, image: Vec<u8>| -> Result<(), WriteFailure> {
            let layer = program.layers.get(index);
            let motion = Motion::of_layer(layer);
            let length = u32::try_from(image.len()).map_err(|_| unreachable())?;
            let top = tops.get(index).map_or(0.0, |&(_, top)| top);
            let address = u32::try_from(next).map_err(|_| unreachable())?;
            put_f32(&mut table, round_height(top));
            put_f32(&mut table, motion.exposure_sec);
            put_f32(&mut table, 0.0); // the waits carry the pauses, not a light-off delay
            put_u32(&mut table, address);
            put_u32(&mut table, length + LAYER_DEF_EX_BYTES);
            put_u32(&mut table, 0); // the data type: an image
            put_u32(&mut table, 0); // the centroid distance, which UVtools leaves at 0
            put_u32(
                &mut table,
                largest_outlines.get(index).copied().unwrap_or_default(),
            );
            put_u32(&mut table, 0);
            put_u32(&mut table, 0);

            data.write_all(&layer_motion(&motion))?;
            data.write_all(&image)?;
            next += u64::from(LAYER_DEF_EX_BYTES) + u64::from(length);
            Ok(())
        },
    )?;
    let (data_crc, data_length) = (data.crc, data.written);
    if next > u64::from(u32::MAX) {
        return Err(unreachable());
    }

    // The table sits between the head and the layers: fold the 3 checksums
    // together in file order.
    let mut crc = crc32_shift(head_crc, table.len() as u64) ^ crc32_zero_start(0, &table);
    crc = crc32_shift(crc, data_length) ^ data_crc;

    let end = out.stream_position()?;
    out.seek(SeekFrom::Start(start + u64::from(layout.layer_table)))?;
    out.write_all(&table)?;
    out.seek(SeekFrom::Start(end))?;
    out.write_all(&crc.to_be_bytes())?;
    out.flush()?;
    Ok(())
}

fn header(
    program: &PrintProgram,
    model: &str,
    layout: &Layout,
    (bottom, normal): (&Motion, &Motion),
) -> Vec<u8> {
    let raster = &program.raster;
    let resolution = raster.panel_resolution();
    let (width_mm, height_mm) = raster.panel_size_mm();
    let print_height = slabs(program)
        .last()
        .map_or(0.0, |&(_, top)| round_height(top));
    let mut out = Vec::new();
    out.extend_from_slice(&(MAGIC.len() as u32).to_be_bytes());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_be_bytes());
    out.extend_from_slice(&(model.len() as u32 + 1).to_be_bytes());
    out.extend_from_slice(model.as_bytes());
    out.push(0);
    put_u16(&mut out, resolution.x() as u16);
    put_u16(&mut out, resolution.y() as u16);
    put_f32(&mut out, round_display(width_mm as f32));
    put_f32(&mut out, round_display(height_mm as f32));
    put_f32(&mut out, round_away(raster.build_volume().z_mm() as f32, 2));
    put_f32(&mut out, print_height);
    put_f32(
        &mut out,
        round_height(program.layer_height_mm.value() as f32),
    );
    put_u32(&mut out, program.bottom_layer_count);
    put_u32(&mut out, layout.small_preview);
    put_u32(&mut out, layout.layer_table);
    put_u32(&mut out, program.layers.len() as u32);
    put_u32(&mut out, layout.large_preview);
    put_u32(&mut out, program.print_time_sec.value() as u32);
    put_u32(&mut out, u32::from(raster.mirror().x)); // the projector is mirrored
    put_u32(&mut out, layout.print_parameters);
    put_u32(&mut out, PRINT_PARAMETERS_BYTES);
    put_u32(&mut out, 1); // the header's own level, which UVtools leaves at 1
    put_u16(&mut out, u16::from(normal.pwm));
    put_u16(&mut out, u16::from(bottom.pwm));
    put_u32(&mut out, 0); // the layers are not encrypted
    put_u32(&mut out, layout.slicer_info);
    put_u32(&mut out, SLICER_INFO_BYTES);
    out
}

fn print_parameters_block(program: &PrintProgram, (bottom, normal): (&Motion, &Motion)) -> Vec<u8> {
    let mut out = Vec::with_capacity(PRINT_PARAMETERS_BYTES as usize);
    put_f32(&mut out, bottom.total_lift_mm);
    put_f32(&mut out, bottom.lift_mm_min);
    put_f32(&mut out, normal.total_lift_mm);
    put_f32(&mut out, normal.lift_mm_min);
    put_f32(&mut out, normal.retract_mm_min);
    put_f32(
        &mut out,
        round_away((program.volume_mm3.value() / 1000.0) as f32, 3),
    );
    put_f32(&mut out, round_away(program.mass_g.value() as f32, 3));
    put_f32(&mut out, 0.0); // nothing here prices a print
    put_f32(&mut out, 0.0); // the bottom light-off delay: the waits carry it
    put_f32(&mut out, 0.0); // the light-off delay
    put_u32(&mut out, program.bottom_layer_count);
    put_f32(&mut out, normal.exposure_sec);
    put_f32(&mut out, bottom.exposure_sec);
    out.resize(out.len() + 16, 0);
    debug_assert_eq!(out.len() as u32, PRINT_PARAMETERS_BYTES);
    out
}

fn slicer_info_block(program: &PrintProgram, (bottom, normal): (&Motion, &Motion)) -> Vec<u8> {
    let aa = u32::from(program.raster.aa().factor()).clamp(1, 16);
    let per_layer = uses_per_layer_settings(program, &ProgramSummary::of(program));
    let layer_count = program.layers.len() as u32;
    let most_transition = if program.bottom_layer_count == 0 {
        0
    } else {
        layer_count.saturating_sub(program.bottom_layer_count + 1)
    };
    let mut out = Vec::with_capacity(SLICER_INFO_BYTES as usize);
    put_f32(&mut out, bottom.second_lift_mm);
    put_f32(&mut out, bottom.second_lift_mm_min);
    put_f32(&mut out, normal.second_lift_mm);
    put_f32(&mut out, normal.second_lift_mm_min);
    put_f32(&mut out, normal.second_retract_mm);
    put_f32(&mut out, normal.second_retract_mm_min);
    put_f32(&mut out, normal.wait_after_lift_sec);
    put_u32(&mut out, u32::from(per_layer));
    put_u32(
        &mut out,
        u32::try_from(program.created_at.unix_sec / 60).unwrap_or_default(),
    );
    put_u32(&mut out, aa);
    put_u32(&mut out, 0); // the software version, which UVtools leaves at 0
    put_f32(&mut out, normal.wait_before_cure_sec);
    put_f32(&mut out, normal.wait_after_cure_sec);
    put_f32(&mut out, normal.exposure_sec);
    put_f32(&mut out, bottom.exposure_sec);
    put_f32(&mut out, normal.wait_after_lift_sec);
    put_u32(
        &mut out,
        program.transition_layer_count.min(most_transition),
    );
    put_u32(&mut out, 0);
    put_u32(&mut out, 0);
    debug_assert_eq!(out.len() as u32, SLICER_INFO_BYTES);
    out
}

/// The 44 bytes before a layer's image: its motion.
fn layer_motion(motion: &Motion) -> Vec<u8> {
    let mut out = Vec::with_capacity(LAYER_DEF_EX_BYTES as usize);
    put_f32(&mut out, motion.total_lift_mm);
    put_f32(&mut out, motion.lift_mm_min);
    put_f32(&mut out, motion.second_lift_mm);
    put_f32(&mut out, motion.second_lift_mm_min);
    put_f32(&mut out, motion.retract_mm_min);
    put_f32(&mut out, motion.second_retract_mm);
    put_f32(&mut out, motion.second_retract_mm_min);
    put_f32(&mut out, motion.wait_after_cure_sec);
    put_f32(&mut out, motion.wait_after_lift_sec);
    put_f32(&mut out, motion.wait_before_cure_sec);
    put_f32(&mut out, f32::from(motion.pwm));
    debug_assert_eq!(out.len() as u32, LAYER_DEF_EX_BYTES);
    out
}

#[cfg(test)]
mod tests_cxdlpv4;
