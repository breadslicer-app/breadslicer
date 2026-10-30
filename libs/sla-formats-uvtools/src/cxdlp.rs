/*
 * SPDX-FileCopyrightText: 2020-2026 PTRTECH, Tiago Conceição (sn4k3) and UVtools contributors
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 *
 * Ported to Rust from UVtools UVtools.Core/FileFormats/CrealityCXDLPFile.cs
 * (with the column scan of UVtools.Core/FileFormats/ColumnLines.cs, the
 * RGB565 big-endian packing of UVtools.Core/FileFormats/FileFormat.cs and the
 * Creality models of UVtools.Core/Printer/Machine.cs), UVtools v7.1.1, commit
 * 8b5c91fd41b60bd6265ba1c1f2b2b64636b873e8, modified for Breadslicer: the
 * layers are encoded from the slicer's run-length rows on every core, the
 * largest-outline areas come from the sliced contours, and the print settings
 * come from a Breadslicer print program.
 *
 * This program is free software: you can redistribute it and/or modify it
 * under the terms of the GNU Affero General Public License as published by the
 * Free Software Foundation, either version 3 of the License, or (at your
 * option) any later version. It is distributed WITHOUT ANY WARRANTY; see the
 * GNU Affero General Public License (LICENSE.md) for details.
 */

//! Creality's CXDLP container, versions 2 and 3 (Halot printers).
//!
//! Everything multi-byte is big-endian, except the integers of the version 3
//! block, which UVtools writes little-endian and the machines accept. The file
//! is:
//!
//! 1. A header: magic `CXSW3DV2`, version, printer model code (such as `CL-60`
//!    for the Halot One), layer count and panel resolution.
//! 2. 3 previews in RGB565, 116 x 116 then 290 x 290 twice, each followed by
//!    `\r\n`.
//! 3. The print settings, once for the whole file: the panel size and layer
//!    height as UTF-16 text, then exposures, the bottom layer count, lifts,
//!    speeds in whole mm/s and light PWMs.
//! 4. Each layer's largest outline area (mm² x 1000), then `\r\n`.
//! 5. Version 3 only: the slicer and material names and the machine's
//!    compensation and anti-aliasing switches.
//! 6. One record per layer: its largest outline area, then one 6-byte line per
//!    run of equal non-black pixels down each column (start row, end row,
//!    column, grey), columns left to right, then `\r\n`.
//! 7. The magic again, then a checksum of everything before it: CRC-32 (with
//!    a zero start and no final inversion) for version 3, an XOR of every byte
//!    for version 2.
//!
//! The format keeps one set of settings for the bottom layers and one for the
//! rest, with no per-layer values: a transition between them, or anything else
//! that varies per layer, is not carried. The settings are stored as whole
//! numbers except the normal exposure, which keeps tenths of a second, and the
//! machines refuse a wait before cure under 1 second, so it is raised to 1.

use std::io::{self, Write};

use sc_sl1cer::contour::ContourStack;
use sc_sl1cer::raster::rle::RleRun;
use sc_sla_formats::layer_images::write_layer_images;
use sc_sla_formats::{
    CxdlpVersion, PreviewImage, PreviewSize, PreviewSizes, PrintPreviews, PrintProgram,
    WriteFailure,
};

use crate::layer_stats::{ContourStats, LayerStats};
use crate::program::{ProgramSummary, mm_per_sec};

/// The magic, written with its terminating zero, at the start and before the
/// checksum.
pub(crate) const MAGIC: &[u8; 9] = b"CXSW3DV2\0";

/// Ends every preview, the area table, the version 3 block and every layer.
pub(crate) const PAGE_BREAK: [u8; 2] = [0x0d, 0x0a];

/// The sizes of the 3 previews. The slicer renders the first 2; the third is
/// the second again.
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

/// The highest row a layer line can name: 13 bits.
const MAX_ROW: u32 = 0x1fff;

/// The highest column a layer line can name: 14 bits.
const MAX_COLUMN: u32 = 0x3fff;

/// Bytes per layer line.
pub(crate) const LINE_BYTES: usize = 6;

/// The Creality models whose files this format carries, with the panel each
/// one has, in the order UVtools matches a panel against them.
const CREALITY_MODELS: [(&str, u32, u32); 12] = [
    ("CT-005", 3840, 2400),
    ("CT133PRO", 3840, 2160),
    ("CL-89L", 3840, 2400),
    ("CL-133", 3840, 2160),
    ("CL-60", 1620, 2560),
    ("CL-79", 4320, 2560),
    ("CL-70", 2560, 2400),
    ("CL-89", 3840, 2400),
    ("CL-92", 5760, 3600),
    ("CL925", 5760, 3600),
    ("CL-103L", 7680, 4320),
    ("CL-103", 7680, 4320),
];

/// The model codes of the CXDLP machines Breadslicer ships profiles for, by
/// the name the profile gives the machine: the code UVtools picks out of the
/// name its own printer profile gives the machine ("Creality Halot Sky CL-89",
/// "Creality CT-005 Pro"). Checked first, since 2 models can share a panel
/// and the profile names leave the code out.
const BREADSLICER_PROFILE_MODELS: [(&str, &str); 10] = [
    ("Halot One", "CL-60"),
    ("Halot One Pro", "CL-70"),
    ("Halot One Plus", "CL-79"),
    ("Halot Lite", "CL-89L"),
    ("Halot Sky", "CL-89"),
    ("Halot Sky Plus", "CL-92"),
    ("Halot Ray", "CL925"),
    ("Halot Max", "CL-133"),
    ("CT-005 Pro", "CT-005"),
    ("CT-133 Pro", "CT-133"),
];

/// Finds the model code the machine checks a file against.
///
/// A Breadslicer profile name gives its own, a name that is a code already
/// (`CL-...`, `CT-...`) is kept, a name holding one has it picked out, and
/// otherwise the panel resolution picks the first Creality model with that
/// panel.
pub(crate) fn printer_model(machine_name: &str, resolution: (u32, u32)) -> Option<String> {
    let name = machine_name.trim();
    if let Some((_, code)) = BREADSLICER_PROFILE_MODELS
        .iter()
        .find(|(profile_name, _)| profile_name.eq_ignore_ascii_case(name))
    {
        return Some((*code).to_owned());
    }
    if name.starts_with("CL") || name.starts_with("CT") {
        return Some(name.to_owned());
    }
    if let Some(code) = code_in(name) {
        return Some(code);
    }
    CREALITY_MODELS
        .iter()
        .find(|(_, x, y)| (*x, *y) == resolution)
        .map(|(code, _, _)| (*code).to_owned())
}

/// Picks a model code out of a name, as `(CL|CT)-?[0-9]+[a-zA-Z]?` matches it.
fn code_in(name: &str) -> Option<String> {
    let bytes = name.as_bytes();
    for start in 0..bytes.len().saturating_sub(2) {
        if !(bytes[start] == b'C' && (bytes[start + 1] == b'L' || bytes[start + 1] == b'T')) {
            continue;
        }
        let mut end = start + 2;
        if bytes.get(end) == Some(&b'-') {
            end += 1;
        }
        let digits_start = end;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
        if end == digits_start {
            continue;
        }
        if bytes.get(end).is_some_and(u8::is_ascii_alphabetic) {
            end += 1;
        }
        return Some(name[start..end].to_owned());
    }
    None
}

/// The checksum the file ends with, kept as the bytes go out.
struct ChecksumWriter<'w, W: Write + ?Sized> {
    inner: &'w mut W,
    version: CxdlpVersion,
    crc: u32,
    xor: u8,
}

impl<'w, W: Write + ?Sized> ChecksumWriter<'w, W> {
    fn new(inner: &'w mut W, version: CxdlpVersion) -> Self {
        Self {
            inner,
            version,
            crc: 0,
            xor: 0,
        }
    }

    fn checksum(&self) -> u32 {
        match self.version {
            CxdlpVersion::V2 => u32::from(self.xor),
            CxdlpVersion::V3 => self.crc,
        }
    }
}

impl<W: Write + ?Sized> Write for ChecksumWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buf)?;
        let buf = &buf[..written];
        match self.version {
            CxdlpVersion::V2 => self.xor = buf.iter().fold(self.xor, |acc, byte| acc ^ byte),
            CxdlpVersion::V3 => self.crc = crc32_zero_start(self.crc, buf),
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// The table of the reflected CRC-32 polynomial 0xEDB88320.
const CRC_TABLE: [u32; 256] = {
    let mut table = [0_u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut value = i as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 == 0 {
                value >> 1
            } else {
                (value >> 1) ^ 0xedb8_8320
            };
            bit += 1;
        }
        table[i] = value;
        i += 1;
    }
    table
};

/// Continues the version 3 checksum over `bytes`: CRC-32's table walk, started
/// from zero and never inverted, as the machine computes it.
pub(crate) fn crc32_zero_start(mut crc: u32, bytes: &[u8]) -> u32 {
    for &byte in bytes {
        crc = CRC_TABLE[((crc as u8) ^ byte) as usize] ^ (crc >> 8);
    }
    crc
}

/// Rounds to the nearest whole number, halves away from zero, clamped into a
/// `u16`: the format stores most settings this way.
fn round_u16(value: f32) -> u16 {
    value.round().clamp(0.0, f32::from(u16::MAX)) as u16
}

/// Rounds to `digits` decimals, halves to even, as UVtools rounds the panel
/// size.
fn round_even(value: f32, digits: i32) -> f32 {
    let scale = 10_f64.powi(digits);
    ((f64::from(value) * scale).round_ties_even() / scale) as f32
}

/// Rounds to 3 decimals, halves away from zero, as UVtools rounds heights.
fn round_height(value: f32) -> f32 {
    ((f64::from(value) * 1000.0).round() / 1000.0) as f32
}

fn write_u16_be(out: &mut impl Write, value: u16) -> io::Result<()> {
    out.write_all(&value.to_be_bytes())
}

fn write_u32_be(out: &mut impl Write, value: u32) -> io::Result<()> {
    out.write_all(&value.to_be_bytes())
}

/// A string as `u32` big-endian length then its bytes, the length counting a
/// terminating zero; an empty string is a zero length alone.
fn write_terminated(out: &mut impl Write, value: &str) -> io::Result<()> {
    if value.is_empty() {
        return write_u32_be(out, 0);
    }
    write_u32_be(out, value.len() as u32 + 1)?;
    out.write_all(value.as_bytes())?;
    out.write_all(&[0])
}

/// A number as UTF-16 big-endian text, after its byte length.
fn write_utf16_number(out: &mut impl Write, value: f32) -> io::Result<()> {
    let text = value.to_string();
    let units: Vec<u16> = text.encode_utf16().collect();
    write_u32_be(out, (units.len() * 2) as u32)?;
    for unit in units {
        write_u16_be(out, unit)?;
    }
    Ok(())
}

/// Packs a preview as RGB565, big-endian.
pub(crate) fn rgb565_be(image: &PreviewImage) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(image.rgb().len() / 3 * 2);
    for pixel in image.rgb().chunks_exact(3) {
        let (r, g, b) = (
            u16::from(pixel[0]),
            u16::from(pixel[1]),
            u16::from(pixel[2]),
        );
        let packed = ((r & 0b1111_1000) << 8) | ((g & 0b1111_1100) << 3) | (b >> 3);
        bytes.extend_from_slice(&packed.to_be_bytes());
    }
    bytes
}

/// Writes one layer line.
fn push_line(out: &mut Vec<u8>, start_row: u32, end_row: u32, column: u32, grey: u8) {
    out.extend_from_slice(&[
        (start_row >> 5) as u8,
        ((start_row << 3) + (end_row >> 10)) as u8,
        (end_row >> 2) as u8,
        ((end_row << 6) + (column >> 8)) as u8,
        column as u8,
        grey,
    ]);
}

/// Encodes one layer record from a frame of `runs`, `width` pixels per row:
/// the largest outline area, the line count, the lines down each column of the
/// lit area, and the page break.
pub(crate) fn encode_layer(runs: &[RleRun], width: u32, largest_outline: u32) -> Vec<u8> {
    let stats = LayerStats::of(runs, width);
    let mut lines = Vec::new();
    let mut line_count = 0_u32;

    if let Some(rect) = stats.bounds {
        let columns = rect.width() as usize;
        // Per column of the lit area: the open run's first row and grey.
        let mut open: Vec<Option<(u32, u8)>> = vec![None; columns];
        let mut finished: Vec<Vec<(u32, u32, u8)>> = vec![Vec::new(); columns];

        let width = u64::from(width);
        let mut position = 0_u64;
        for run in runs {
            let mut left = u64::from(run.length);
            while left > 0 {
                let row = (position / width) as u32;
                let column = position % width;
                let take = left.min(width - column);
                if row >= rect.y1 {
                    break;
                }
                if row >= rect.y0 {
                    let first = (column as u32).max(rect.x0);
                    let last = ((column + take) as u32).min(rect.x1);
                    for x in first..last {
                        let slot = (x - rect.x0) as usize;
                        match open[slot] {
                            Some((_, grey)) if grey == run.value => {}
                            Some((start, grey)) => {
                                finished[slot].push((start, row - 1, grey));
                                open[slot] = (run.value != 0).then_some((row, run.value));
                            }
                            None => {
                                if run.value != 0 {
                                    open[slot] = Some((row, run.value));
                                }
                            }
                        }
                    }
                }
                position += take;
                left -= take;
            }
        }
        for (slot, run) in open.iter().enumerate() {
            if let Some((start, grey)) = *run {
                finished[slot].push((start, rect.y1 - 1, grey));
            }
        }

        for (slot, column_lines) in finished.iter().enumerate() {
            let column = rect.x0 + slot as u32;
            for &(start, end, grey) in column_lines {
                push_line(&mut lines, start, end, column, grey);
                line_count += 1;
            }
        }
    }

    let mut record = Vec::with_capacity(8 + lines.len() + PAGE_BREAK.len());
    record.extend_from_slice(&largest_outline.to_be_bytes());
    record.extend_from_slice(&line_count.to_be_bytes());
    record.extend_from_slice(&lines);
    record.extend_from_slice(&PAGE_BREAK);
    record
}

/// Writes `stack` as a CXDLP file of `version` running the settings in
/// `program`.
///
/// Every layer image is the plate's image that [`write_layer_images`]
/// describes, as in the GOO and CTB files; the raster setup's mirror flags turn
/// it around and its panel rotation turns it a quarter.
/// Without `previews` the machine shows black thumbnails.
///
/// # Errors
///
/// [`WriteFailure::PreviewSize`] when a rendering is not the size this format
/// fixes, and [`WriteFailure::Unencodable`] when the panel, the layer count or
/// the machine is beyond what the format can name. Both are raised before any
/// byte is written.
pub fn write_file(
    version: CxdlpVersion,
    program: &PrintProgram,
    previews: Option<&PrintPreviews>,
    stack: &ContourStack,
    out: &mut (impl Write + ?Sized),
) -> Result<(), WriteFailure> {
    let resolution = program.raster.panel_resolution();
    let (res_x, res_y) = (resolution.x(), resolution.y());
    if res_x > MAX_COLUMN + 1 || res_y > MAX_ROW + 1 {
        return Err(WriteFailure::Unencodable(format!(
            "a {res_x} x {res_y} panel is beyond the {} x {} pixels CXDLP lines can address",
            MAX_COLUMN + 1,
            MAX_ROW + 1,
        )));
    }
    let layer_count = u16::try_from(program.layers.len()).map_err(|_| {
        WriteFailure::Unencodable(format!(
            "{} layers is more than the {} CXDLP counts",
            program.layers.len(),
            u16::MAX,
        ))
    })?;
    let model =
        printer_model(program.machine_name.as_str(), (res_x, res_y)).ok_or_else(|| {
            WriteFailure::Unencodable(format!(
                "no Creality model has a {res_x} x {res_y} panel, and the machine name \"{}\" names none",
                program.machine_name.as_str(),
            ))
        })?;
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

    let largest_outlines: Vec<u32> = stack
        .layers()
        .iter()
        .map(|layer| (ContourStats::of(layer).largest_outline_mm2 * 1000.0) as u32)
        .collect();

    let mut out = ChecksumWriter::new(out, version);
    write_header(&mut out, version, &model, layer_count, res_x, res_y)?;
    for preview in [&small, &large, &large] {
        out.write_all(&rgb565_be(preview))?;
        out.write_all(&PAGE_BREAK)?;
    }
    write_settings(&mut out, program)?;
    for area in &largest_outlines {
        write_u32_be(&mut out, *area)?;
    }
    out.write_all(&PAGE_BREAK)?;
    if version == CxdlpVersion::V3 {
        write_v3_block(&mut out, program)?;
    }

    write_layer_images(
        stack,
        &program.raster,
        |index, runs| encode_layer(runs, res_x, largest_outlines[index]),
        |_, record| out.write_all(&record),
    )?;

    write_u32_be(&mut out, MAGIC.len() as u32)?;
    out.write_all(MAGIC)?;
    let checksum = out.checksum();
    out.inner.write_all(&checksum.to_be_bytes())?;
    Ok(())
}

fn write_header(
    out: &mut impl Write,
    version: CxdlpVersion,
    model: &str,
    layer_count: u16,
    res_x: u32,
    res_y: u32,
) -> io::Result<()> {
    write_u32_be(out, MAGIC.len() as u32)?;
    out.write_all(MAGIC)?;
    write_u16_be(
        out,
        match version {
            CxdlpVersion::V2 => 2,
            CxdlpVersion::V3 => 3,
        },
    )?;
    write_terminated(out, model)?;
    write_u16_be(out, layer_count)?;
    write_u16_be(out, res_x as u16)?;
    write_u16_be(out, res_y as u16)?;
    out.write_all(&[0; 64])
}

fn write_settings(out: &mut impl Write, program: &PrintProgram) -> io::Result<()> {
    let (panel_x_mm, panel_y_mm) = program.raster.panel_size_mm();
    let summary = ProgramSummary::of(program);
    let (bottom, normal) = (summary.bottom, summary.normal);

    write_utf16_number(out, round_even(panel_x_mm as f32, 2))?;
    write_utf16_number(out, round_even(panel_y_mm as f32, 2))?;
    write_utf16_number(out, round_height(program.layer_height_mm.value() as f32))?;

    // The normal exposure keeps tenths of a second; everything else is whole.
    write_u16_be(out, round_u16(round_even(normal.exposure_sec, 1) * 10.0))?;
    // The machines refuse a file whose wait before cure is 0.
    write_u16_be(out, round_u16(normal.wait_before_cure_sec.max(1.0)))?;
    write_u16_be(out, round_u16(bottom.exposure_sec))?;
    write_u16_be(
        out,
        u16::try_from(program.bottom_layer_count).unwrap_or(u16::MAX),
    )?;
    write_u16_be(out, round_u16(bottom.total_lift_mm()))?;
    write_u16_be(out, round_u16(mm_per_sec(bottom.lift_mm_min)))?;
    write_u16_be(out, round_u16(normal.total_lift_mm()))?;
    write_u16_be(out, round_u16(mm_per_sec(normal.lift_mm_min)))?;
    write_u16_be(out, round_u16(mm_per_sec(normal.retract_mm_min)))?;
    write_u16_be(out, u16::from(bottom.pwm))?;
    write_u16_be(out, u16::from(normal.pwm))
}

fn write_v3_block(out: &mut impl Write, program: &PrintProgram) -> io::Result<()> {
    write_terminated(out, &format!("Breadslicer v{}", program.slicer_version))?;
    write_terminated(out, program.material_name.as_str())?;
    // UVtools writes these integers little-endian.
    out.write_all(&[0])?; // distortion compensation off
    out.write_all(&600_u32.to_le_bytes())?; // its thickness
    out.write_all(&300_000_u32.to_le_bytes())?; // its focal length
    out.write_all(&[1])?; // XY profile compensation on
    out.write_all(&0_u16.to_le_bytes())?; // by 0
    out.write_all(&[0])?; // Z penetration compensation off
    out.write_all(&1000_u16.to_le_bytes())?; // its level
    // Anti-aliasing on, greys 1 to 255: the layers carry their own greys.
    out.write_all(&[1, 1, 255])?;
    out.write_all(&[0, 2])?; // image blur off, level 2
    out.write_all(&PAGE_BREAK)
}

#[cfg(test)]
mod tests_cxdlp;
