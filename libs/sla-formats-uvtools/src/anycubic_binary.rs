/*
 * SPDX-FileCopyrightText: 2020-2026 PTRTECH, Tiago Conceição (sn4k3) and UVtools contributors
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 *
 * Ported to Rust from UVtools UVtools.Core/FileFormats/AnycubicFile.cs
 * (FileMark, Header, Preview, Preview2, LayerImageColorTable, Extra, Machine,
 * Software, Model, LayerDef, SubLayerDef, the extension table and
 * EncodeInternally), UVtools v7.1.1, commit
 * 8b5c91fd41b60bd6265ba1c1f2b2b64636b873e8, modified for Breadslicer: the
 * layers are encoded from the slicer's run-length rows on every core, the
 * per-layer figures and the model's bounds are measured on those rows, and the
 * print settings come from a Breadslicer print program.
 *
 * This program is free software: you can redistribute it and/or modify it
 * under the terms of the GNU Affero General Public License as published by the
 * Free Software Foundation, either version 3 of the License, or (at your
 * option) any later version. It is distributed WITHOUT ANY WARRANTY; see the
 * GNU Affero General Public License (LICENSE.md) for details.
 */

//! Anycubic's binary container, the one Photon Workshop wrote before its ZIP:
//! `.pwmx` (Photon Mono X), `.pwma` (Mono 4K), `.pm3` (M3), `.pws` (Photon S)
//! and the rest of [`AnycubicBinaryVariant`].
//!
//! The file is a run of named tables, little-endian, which the file mark at its
//! start points at. Which tables there are depends on the revision of the
//! layout, and each variant is written in the newest revision UVtools writes
//! for its extension:
//!
//! - 1 (`.pws`, `.pw0`, `.pwx`): the header, the preview and the layer table.
//! - 516 (`.pwmx`, `.pwma`, `.pm3`, `.dlp`, ...): plus the grey table, the
//!   2-stage motion (`EXTRA`) and the machine.
//! - 517 (`.pwmb`, `.pmx2`, `.pm3r`, `.pm5`, ...): plus the software and the
//!   bounds of the model.
//! - 518 (`.pm5s`, `.m5sp`, `.pm4n`): plus a second layer table and a second
//!   preview.
//!
//! The layer images are PW0 (16 greys), except in the `.pws`, which stores one
//! 1-bit plane per anti-aliasing level.

use std::io::SeekFrom;

use sc_sl1cer::contour::ContourStack;
use sc_sla_formats::layer_images::write_layer_images;
use sc_sla_formats::{
    AnycubicBinaryVariant, PreviewImage, PreviewSize, PreviewSizes, PrintPreviews, PrintProgram,
    PrinterFileSink, WriteFailure,
};

use crate::layer_stats::{LayerStats, PixelRect};
use crate::program::{
    ProgramSummary, StageSettings, mm_per_sec, round_away, round_display, round_even, round_height,
    slabs, uses_per_layer_settings,
};
use crate::{pw0, pws};

/// The first revision of the layout.
pub const VERSION_1: u32 = 1;
/// Adds the grey table.
pub const VERSION_515: u32 = 515;
/// Adds the 2-stage motion and the machine.
pub const VERSION_516: u32 = 516;
/// Adds the software and the bounds of the model.
pub const VERSION_517: u32 = 517;
/// Adds the second layer table and the second preview.
pub const VERSION_518: u32 = 518;

/// The 2 previews the slicer renders for this container. Revision 518 carries
/// both; the others carry the small one.
pub const PREVIEW_SIZES: PreviewSizes = PreviewSizes {
    small: PreviewSize {
        width: 224,
        height: 168,
    },
    large: PreviewSize {
        width: 330,
        height: 190,
    },
};

/// The file mark's magic.
pub(crate) const FILE_MARK: &str = "ANYCUBIC";

/// The width of a table's name.
const NAME_BYTES: usize = 12;

/// A table's name and length, which every named table starts with.
const TABLE_BASE_BYTES: u32 = NAME_BYTES as u32 + 4;

/// One record of the layer table.
pub(crate) const LAYER_DEF_BYTES: u32 = 32;

/// One record of the second layer table.
pub(crate) const SUB_LAYER_DEF_BYTES: u32 = 44;

/// The software table, which has no name.
const SOFTWARE_TABLE_BYTES: u32 = 164;

/// The model table, its name and length included.
const MODEL_TABLE_BYTES: u32 = TABLE_BASE_BYTES + 32;

/// The levels the grey table maps.
const GREY_TABLE_LEVELS: u32 = 16;

/// The background colours UVtools writes for the machine and the second
/// preview.
const MACHINE_BACKGROUND: u32 = 6_506_241;
const PREVIEW2_BACKGROUND: u16 = 4293;

/// What the software table names as the renderer, as UVtools writes it.
const OPENGL_VERSION: &str = "3.3-CoreProfile";

/// Reports the revision of the layout a variant is written in: the newest one
/// UVtools lists for its extension (all of them for `.pm4n`, which it does not
/// restrict).
pub fn file_version(variant: AnycubicBinaryVariant) -> u32 {
    use AnycubicBinaryVariant as V;
    match variant {
        V::Pws | V::Pw0 | V::Pwx => VERSION_1,
        V::Pwmx | V::Pwmo | V::Pwms | V::Pmsq | V::Dlp | V::Pwma | V::Pm3 | V::Pm3m => VERSION_516,
        V::Pwmb | V::Dl2p | V::Pmx2 | V::Pm3r | V::Pm3n | V::Pm5 | V::Px6s => VERSION_517,
        V::Pm5s | V::M5sp | V::Pm4n => VERSION_518,
    }
}

/// Reports whether a variant stores its layers as PWS bit planes rather than
/// PW0.
pub fn uses_bit_planes(variant: AnycubicBinaryVariant) -> bool {
    variant == AnycubicBinaryVariant::Pws
}

/// The bytes the file mark takes in a revision.
pub(crate) fn file_mark_bytes(version: u32) -> u32 {
    // Header, software, preview, grey table, layer table, extra and layer
    // images, then the machine, the model, the second layer table and the
    // second preview as the revisions add them.
    let addresses = match version {
        VERSION_518.. => 11,
        VERSION_517 => 9,
        VERSION_516 => 8,
        _ => 7,
    };
    NAME_BYTES as u32 + 4 * (2 + addresses)
}

/// The tables a revision has, as its file mark counts them.
fn table_count(version: u32) -> u32 {
    match version {
        VERSION_518.. => 11,
        VERSION_517 => 9,
        VERSION_516 => 8,
        VERSION_515 => 5,
        _ => 4,
    }
}

/// The bytes the header's fields take in a revision, its name and length not
/// included.
pub(crate) fn header_bytes(version: u32) -> u32 {
    match version {
        VERSION_518.. => 96,
        VERSION_517 => 92,
        VERSION_516 => 84,
        _ => 80,
    }
}

/// The panel as these tables describe it.
#[derive(Clone, Copy, Debug)]
struct Panel {
    width_mm: f32,
    height_mm: f32,
    machine_z_mm: f32,
    resolution_x: u32,
    resolution_y: u32,
}

impl Panel {
    fn of(program: &PrintProgram) -> Self {
        let resolution = program.raster.panel_resolution();
        let (width_mm, height_mm) = program.raster.panel_size_mm();
        Self {
            width_mm: round_display(width_mm as f32),
            height_mm: round_display(height_mm as f32),
            machine_z_mm: round_away(program.raster.build_volume().z_mm() as f32, 2),
            resolution_x: resolution.x(),
            resolution_y: resolution.y(),
        }
    }

    /// A pixel's width in microns, 3 decimals.
    fn pixel_width_um(&self) -> f32 {
        round_even(self.width_mm / self.resolution_x as f32 * 1000.0, 3)
    }

    /// A pixel's height in microns, 3 decimals.
    fn pixel_height_um(&self) -> f32 {
        round_even(self.height_mm / self.resolution_y as f32 * 1000.0, 3)
    }

    /// A pixel's width and height in millimetres, 4 decimals.
    fn pixel_mm(&self) -> (f32, f32) {
        (
            round_even(self.width_mm / self.resolution_x as f32, 4),
            round_even(self.height_mm / self.resolution_y as f32, 4),
        )
    }
}

/// Where the file mark says each table is. A table a revision does not have
/// stays at 0.
#[derive(Clone, Copy, Debug, Default)]
struct Addresses {
    header: u32,
    software: u32,
    preview: u32,
    grey_table: u32,
    layer_definition: u32,
    extra: u32,
    machine: u32,
    layer_image: u32,
    model: u32,
    sub_layer_definition: u32,
    preview2: u32,
}

/// Where a layer's image landed and how many of its pixels are lit.
#[derive(Clone, Copy, Debug)]
struct LayerRecord {
    address: u32,
    length: u32,
    lit_pixels: u32,
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

/// Writes `value` NUL-terminated in a field of `length` bytes, cut to fit.
fn put_fixed(out: &mut Vec<u8>, value: &str, length: usize) {
    let bytes = value.as_bytes();
    let kept = bytes.len().min(length - 1);
    out.extend_from_slice(&bytes[..kept]);
    out.resize(out.len() + length - kept, 0);
}

/// Starts a named table: its name, then its length.
fn put_table_base(out: &mut Vec<u8>, name: &str, length: u32) {
    put_fixed(out, name, NAME_BYTES);
    put_u32(out, length);
}

/// Packs a preview as RGB565, little-endian.
fn rgb565_le(image: &PreviewImage) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(image.rgb().len() / 3 * 2);
    for pixel in image.rgb().chunks_exact(3) {
        let (r, g, b) = (
            u16::from(pixel[0]),
            u16::from(pixel[1]),
            u16::from(pixel[2]),
        );
        let packed = ((r & 0b1111_1000) << 8) | ((g & 0b1111_1100) << 3) | (b >> 3);
        bytes.extend_from_slice(&packed.to_le_bytes());
    }
    bytes
}

/// The speed the platform returns at over the stretch farthest from the vat:
/// the second stage's when the travel has one.
fn far_retract_mm_min(stage: &StageSettings) -> f32 {
    stage
        .second_stage
        .map_or(stage.retract_mm_min, |(_, _, retract)| retract)
}

/// The whole rise of a stage, both stages together, as the header and the
/// layer table store it.
fn total_lift_mm(stage: &StageSettings) -> f32 {
    round_height(
        round_away(stage.lift_mm, 2)
            + stage
                .second_stage
                .map_or(0.0, |(height, _, _)| round_away(height, 2)),
    )
}

/// Reports whether either set of settings moves in 2 stages.
fn is_two_stage(summary: &ProgramSummary) -> bool {
    summary.bottom.second_stage.is_some() || summary.normal.second_stage.is_some()
}

fn header_table(
    version: u32,
    program: &PrintProgram,
    summary: &ProgramSummary,
    panel: &Panel,
) -> Vec<u8> {
    let (bottom, normal) = (&summary.bottom, &summary.normal);
    let length = header_bytes(version);
    let layer_count = program.layers.len() as u32;
    let most_transition = if program.bottom_layer_count == 0 {
        0
    } else {
        layer_count.saturating_sub(program.bottom_layer_count + 1)
    };

    let mut out = Vec::with_capacity((TABLE_BASE_BYTES + length) as usize);
    put_table_base(&mut out, "HEADER", length);
    put_f32(
        &mut out,
        panel.pixel_width_um().max(panel.pixel_height_um()),
    );
    put_f32(
        &mut out,
        round_height(program.layer_height_mm.value() as f32),
    );
    put_f32(&mut out, round_away(normal.exposure_sec, 2));
    put_f32(&mut out, round_away(normal.wait_before_cure_sec, 2));
    put_f32(&mut out, round_away(bottom.exposure_sec, 2));
    put_f32(&mut out, program.bottom_layer_count as f32);
    put_f32(&mut out, total_lift_mm(normal));
    put_f32(&mut out, mm_per_sec(round_away(normal.lift_mm_min, 2)));
    put_f32(
        &mut out,
        mm_per_sec(round_away(far_retract_mm_min(normal), 2)),
    );
    put_f32(
        &mut out,
        round_away((program.volume_mm3.value() / 1000.0) as f32, 3),
    );
    put_u32(&mut out, u32::from(program.raster.aa().factor()));
    put_u32(&mut out, panel.resolution_x);
    put_u32(&mut out, panel.resolution_y);
    put_f32(&mut out, round_away(program.mass_g.value() as f32, 3));
    put_f32(&mut out, 0.0); // price
    out.extend_from_slice(&[b'$', 0, 0, 0]); // its currency
    put_u32(
        &mut out,
        u32::from(uses_per_layer_settings(program, summary)),
    );
    put_u32(&mut out, program.print_time_sec.value() as u32);
    put_u32(
        &mut out,
        program.transition_layer_count.min(most_transition),
    );
    put_u32(&mut out, 0); // transition type
    if length >= 84 {
        put_u32(&mut out, u32::from(is_two_stage(summary))); // advanced mode
    }
    if length >= 92 {
        put_u16(&mut out, 0); // grey
        put_u16(&mut out, 0); // blur level
        put_u32(&mut out, 0); // resin type
    }
    if length >= 96 {
        put_u32(&mut out, 0); // intelligent mode off: the exposures are the file's
    }
    debug_assert_eq!(out.len() as u32, TABLE_BASE_BYTES + length);
    out
}

fn preview_table(image: &PreviewImage) -> Vec<u8> {
    let data = rgb565_le(image);
    let mut out = Vec::with_capacity(28 + data.len());
    put_table_base(&mut out, "PREVIEW", 28 + data.len() as u32);
    put_u32(&mut out, image.width());
    put_fixed(&mut out, "x", 4);
    put_u32(&mut out, image.height());
    out.extend_from_slice(&data);
    out
}

fn preview2_table(image: &PreviewImage) -> Vec<u8> {
    let data = rgb565_le(image);
    let mut out = Vec::with_capacity(28 + data.len());
    put_table_base(&mut out, "PREVIEW2", 28 + data.len() as u32);
    put_u32(&mut out, image.width());
    put_u16(&mut out, PREVIEW2_BACKGROUND);
    put_u16(&mut out, 0);
    put_u32(&mut out, image.height());
    out.extend_from_slice(&data);
    out
}

/// The grey each anti-aliasing level lights a pixel at, for 16 levels.
fn grey_table(anti_aliasing: u8) -> Vec<u8> {
    let increment = 255.0_f32 / f32::from(anti_aliasing.max(1));
    let mut out = Vec::with_capacity(28);
    put_u32(&mut out, 0); // not the full greyscale
    put_u32(&mut out, GREY_TABLE_LEVELS);
    for level in 1..=GREY_TABLE_LEVELS {
        out.push((level as f32 * increment).min(255.0) as u8);
    }
    put_u32(&mut out, 0);
    out
}

/// Writes one set of settings' 2-stage motion: the stretch next to the vat,
/// then the rest. A single-stage travel repeats its speeds with no second
/// height, which is what the machine reads when the advanced mode is off.
fn put_stages(out: &mut Vec<u8>, stage: &StageSettings) {
    let lift_speed = mm_per_sec(round_away(stage.lift_mm_min, 2));
    put_u32(out, 2);
    put_f32(out, round_away(stage.lift_mm, 2));
    put_f32(out, lift_speed);
    put_f32(out, mm_per_sec(round_away(stage.retract_mm_min, 2)));
    match stage.second_stage {
        Some((height, lift, _)) => {
            put_f32(out, round_away(height, 2));
            put_f32(out, mm_per_sec(round_away(lift, 2)));
        }
        None => {
            put_f32(out, 0.0);
            put_f32(out, lift_speed);
        }
    }
    put_f32(out, mm_per_sec(round_away(far_retract_mm_min(stage), 2)));
}

fn extra_table(summary: &ProgramSummary) -> Vec<u8> {
    let mut out = Vec::with_capacity(72);
    // UVtools writes 24 here, whatever the table holds.
    put_table_base(&mut out, "EXTRA", 24);
    put_stages(&mut out, &summary.bottom);
    put_stages(&mut out, &summary.normal);
    out
}

fn machine_table(
    version: u32,
    program: &PrintProgram,
    panel: &Panel,
    bit_planes: bool,
) -> Result<Vec<u8>, WriteFailure> {
    let length = if version >= VERSION_518 { 224 } else { 156 };
    let mut out = Vec::with_capacity(length as usize);
    put_table_base(&mut out, "MACHINE", length);
    put_fixed(&mut out, program.machine_name.as_str(), 96);
    put_fixed(&mut out, if bit_planes { "pwsImg" } else { "pw0Img" }, 16);
    put_u32(&mut out, GREY_TABLE_LEVELS); // the most anti-aliasing levels
    put_u32(
        &mut out,
        match version {
            VERSION_518.. => 15,
            VERSION_517 => 7,
            _ => 1,
        },
    );
    put_f32(&mut out, panel.width_mm);
    put_f32(&mut out, panel.height_mm);
    put_f32(&mut out, panel.machine_z_mm);
    put_u32(&mut out, version); // the newest revision the machine reads
    put_u32(&mut out, MACHINE_BACKGROUND);
    if version >= VERSION_518 {
        let (Ok(resolution_x), Ok(resolution_y)) = (
            u16::try_from(panel.resolution_x),
            u16::try_from(panel.resolution_y),
        ) else {
            return Err(WriteFailure::Unencodable(format!(
                "a {} x {} panel is beyond the {} pixels a side this table counts",
                panel.resolution_x,
                panel.resolution_y,
                u16::MAX,
            )));
        };
        put_f32(&mut out, panel.pixel_width_um());
        put_f32(&mut out, panel.pixel_height_um());
        out.resize(out.len() + 8 * 4, 0);
        put_u32(&mut out, 1); // displays
        put_u32(&mut out, 0);
        put_u16(&mut out, resolution_x);
        put_u16(&mut out, resolution_y);
        out.resize(out.len() + 4 * 4, 0);
    }
    debug_assert_eq!(out.len() as u32, length);
    Ok(out)
}

/// Names the operating system and processor the way .NET does, as UVtools
/// writes it.
fn runtime_identifier() -> String {
    let os = match std::env::consts::OS {
        "windows" => "win",
        "macos" => "osx",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "x86",
        other => other,
    };
    format!("{os}-{arch}")
}

fn software_table(program: &PrintProgram) -> Vec<u8> {
    let mut out = Vec::with_capacity(SOFTWARE_TABLE_BYTES as usize);
    put_fixed(&mut out, "Breadslicer", 32);
    put_u32(&mut out, SOFTWARE_TABLE_BYTES);
    put_fixed(&mut out, program.slicer_version, 32);
    put_fixed(&mut out, &runtime_identifier(), 64);
    put_fixed(&mut out, OPENGL_VERSION, 32);
    debug_assert_eq!(out.len() as u32, SOFTWARE_TABLE_BYTES);
    out
}

/// The model's bounds on the panel, in millimetres from its centre, and its
/// height.
fn model_table(panel: &Panel, bounds: Option<PixelRect>, print_height_mm: f32) -> Vec<u8> {
    let rect = bounds.unwrap_or(PixelRect {
        x0: 0,
        y0: 0,
        x1: 0,
        y1: 0,
    });
    let (pixel_width, pixel_height) = panel.pixel_mm();
    let x = round_even(rect.x0 as f32 * pixel_width, 2) - panel.width_mm / 2.0;
    let y = round_even(rect.y0 as f32 * pixel_height, 2) - panel.height_mm / 2.0;
    let width = round_even(rect.width() as f32 * pixel_width, 2);
    let height = round_even((rect.y1 - rect.y0) as f32 * pixel_height, 2);

    let mut out = Vec::with_capacity(MODEL_TABLE_BYTES as usize);
    put_table_base(&mut out, "MODEL", MODEL_TABLE_BYTES);
    put_f32(&mut out, round_away(x, 4));
    put_f32(&mut out, round_away(y, 4));
    put_f32(&mut out, 0.0);
    put_f32(&mut out, round_away(x + width, 4));
    put_f32(&mut out, round_away(y + height, 4));
    put_f32(&mut out, print_height_mm);
    put_u32(&mut out, 0); // no supports of the slicer's own
    put_f32(&mut out, 0.0);
    out
}

/// The layer table: where each image is, and the layer's own settings.
fn layer_definition_table(program: &PrintProgram, records: &[LayerRecord]) -> Vec<u8> {
    let count = records.len() as u32;
    let mut out = Vec::with_capacity((TABLE_BASE_BYTES + 4 + LAYER_DEF_BYTES * count) as usize);
    put_table_base(&mut out, "LAYERDEF", 4 + LAYER_DEF_BYTES * count);
    put_u32(&mut out, count);
    let heights = slabs(program);
    for ((layer, record), (thickness, _)) in program.layers.iter().zip(records).zip(heights) {
        let stage = StageSettings::of(Some(layer));
        put_u32(&mut out, record.address);
        put_u32(&mut out, record.length);
        put_f32(&mut out, total_lift_mm(&stage));
        put_f32(&mut out, mm_per_sec(round_away(stage.lift_mm_min, 2)));
        put_f32(&mut out, round_away(stage.exposure_sec, 2));
        put_f32(&mut out, round_height(thickness));
        put_u32(&mut out, record.lit_pixels);
        put_u32(&mut out, 0);
    }
    out
}

/// The bytes the second layer table takes for `count` layers, its name and
/// length included.
fn sub_layer_definition_bytes(count: u32) -> u32 {
    TABLE_BASE_BYTES + 8 + SUB_LAYER_DEF_BYTES * count
}

fn sub_layer_definition_table(records: &[LayerRecord]) -> Vec<u8> {
    let count = records.len() as u32;
    let length = sub_layer_definition_bytes(count);
    let mut out = Vec::with_capacity(length as usize);
    put_table_base(&mut out, "SUBIMGS", length);
    put_u32(&mut out, count);
    put_u32(&mut out, 1); // index
    for record in records {
        put_u32(&mut out, record.address);
        put_u32(&mut out, record.length);
        put_u32(&mut out, record.lit_pixels);
        out.resize(out.len() + 8 * 4, 0);
    }
    out
}

fn file_mark_table(version: u32, addresses: &Addresses) -> Vec<u8> {
    let mut out = Vec::with_capacity(file_mark_bytes(version) as usize);
    put_fixed(&mut out, FILE_MARK, NAME_BYTES);
    put_u32(&mut out, version);
    put_u32(&mut out, table_count(version));
    put_u32(&mut out, addresses.header);
    put_u32(&mut out, addresses.software);
    put_u32(&mut out, addresses.preview);
    put_u32(&mut out, addresses.grey_table);
    put_u32(&mut out, addresses.layer_definition);
    put_u32(&mut out, addresses.extra);
    if version >= VERSION_516 {
        put_u32(&mut out, addresses.machine);
    }
    put_u32(&mut out, addresses.layer_image);
    if version >= VERSION_517 {
        put_u32(&mut out, addresses.model);
    }
    if version >= VERSION_518 {
        put_u32(&mut out, addresses.sub_layer_definition);
        put_u32(&mut out, addresses.preview2);
    }
    debug_assert_eq!(out.len() as u32, file_mark_bytes(version));
    out
}

/// The position `out` writes at next, as the 32-bit address the tables store.
fn position(out: &mut dyn PrinterFileSink) -> Result<u32, WriteFailure> {
    let position = out.stream_position()?;
    u32::try_from(position).map_err(|_| {
        WriteFailure::Unencodable(format!(
            "the file passes the {} bytes its 32-bit addresses reach",
            u32::MAX
        ))
    })
}

/// Writes `stack` as an Anycubic binary file of `variant` running the settings
/// in `program`.
///
/// Every layer image is the plate's image that [`write_layer_images`]
/// describes, as in the GOO and CTB files; the raster setup's mirror flags turn
/// it around and its panel rotation turns it a quarter. Without
/// `previews` the machine shows black thumbnails.
///
/// The layers are written as they are made, and the tables that point at them
/// are filled in once they are all in place, as UVtools does.
///
/// # Errors
///
/// [`WriteFailure::PreviewSize`] when a rendering is not the size this format
/// fixes and [`WriteFailure::Unencodable`] when the panel or the layer count is
/// beyond what the tables count, both raised before any byte is written;
/// [`WriteFailure::Unencodable`] also when the file outgrows its 32-bit
/// addresses, and [`WriteFailure::Io`] when it cannot be written.
pub fn write_file(
    variant: AnycubicBinaryVariant,
    program: &PrintProgram,
    previews: Option<&PrintPreviews>,
    stack: &ContourStack,
    out: &mut dyn PrinterFileSink,
) -> Result<(), WriteFailure> {
    let version = file_version(variant);
    let bit_planes = uses_bit_planes(variant);
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
    // Both layer tables have to fit the 32-bit addresses.
    let layer_count = u32::try_from(program.layers.len())
        .ok()
        .filter(|&count| {
            u64::from(SUB_LAYER_DEF_BYTES) * u64::from(count) < u64::from(u32::MAX / 4)
        })
        .ok_or_else(|| {
            WriteFailure::Unencodable(format!(
                "{} layers is more than this container's tables count",
                program.layers.len()
            ))
        })?;
    let panel = Panel::of(program);
    let summary = ProgramSummary::of(program);
    let anti_aliasing = program.raster.aa().factor();
    let machine = if version >= VERSION_516 {
        Some(machine_table(version, program, &panel, bit_planes)?)
    } else {
        None
    };

    let start = out.stream_position()?;
    let mut addresses = Addresses::default();
    out.write_all(&vec![0; file_mark_bytes(version) as usize])?;
    addresses.header = position(out)?;
    out.write_all(&header_table(version, program, &summary, &panel))?;
    addresses.preview = position(out)?;
    out.write_all(&preview_table(&small))?;
    if version >= VERSION_515 {
        addresses.grey_table = position(out)?;
        out.write_all(&grey_table(anti_aliasing))?;
    }

    // The layer table is filled once the images are placed.
    addresses.layer_definition = position(out)?;
    let layer_definition_bytes = TABLE_BASE_BYTES + 4 + LAYER_DEF_BYTES * layer_count;
    out.write_all(&vec![0; layer_definition_bytes as usize])?;
    addresses.layer_image = position(out)?;
    if let Some(machine) = machine {
        addresses.extra = addresses.layer_image;
        out.write_all(&extra_table(&summary))?;
        addresses.machine = position(out)?;
        out.write_all(&machine)?;
        if version >= VERSION_517 {
            addresses.software = position(out)?;
            out.write_all(&software_table(program))?;
            // The model's bounds are measured on the layers.
            addresses.model = position(out)?;
            out.write_all(&vec![0; MODEL_TABLE_BYTES as usize])?;
            if version >= VERSION_518 {
                addresses.sub_layer_definition = position(out)?;
                out.write_all(&vec![0; sub_layer_definition_bytes(layer_count) as usize])?;
                addresses.preview2 = position(out)?;
                out.write_all(&preview2_table(&large))?;
            }
        }
        addresses.layer_image = position(out)?;
    } else if version == VERSION_515 {
        addresses.extra = addresses.layer_image;
    }

    let width = program.raster.panel_resolution().x();
    let dimmest_stored = if bit_planes {
        pws::threshold(1, anti_aliasing)
    } else {
        pw0::DIMMEST_STORED
    };
    let mut records = Vec::with_capacity(program.layers.len());
    let mut bounds: Option<PixelRect> = None;
    write_layer_images(
        stack,
        &program.raster,
        |_, runs| {
            let image = if bit_planes {
                pws::encode(runs, anti_aliasing)
            } else {
                pw0::encode(runs)
            };
            // Counted on the stored image: a grey too dim for the codec is
            // stored dark.
            let lit_pixels: u64 = runs
                .iter()
                .filter(|run| run.value >= dimmest_stored)
                .map(|run| u64::from(run.length))
                .sum();
            let bounds = LayerStats::of_greys_from(runs, width, dimmest_stored).bounds;
            (image, bounds, lit_pixels)
        },
        |_, (image, rect, lit_pixels): (Vec<u8>, Option<PixelRect>, u64)| {
            let address = position(out)?;
            out.write_all(&image)?;
            records.push(LayerRecord {
                address,
                length: image.len() as u32,
                lit_pixels: u32::try_from(lit_pixels).unwrap_or(u32::MAX),
            });
            bounds = match (bounds, rect) {
                (Some(b), Some(r)) => Some(PixelRect {
                    x0: b.x0.min(r.x0),
                    y0: b.y0.min(r.y0),
                    x1: b.x1.max(r.x1),
                    y1: b.y1.max(r.y1),
                }),
                (b, r) => b.or(r),
            };
            Ok::<(), WriteFailure>(())
        },
    )?;
    let end = position(out)?;

    out.seek(SeekFrom::Start(u64::from(addresses.layer_definition)))?;
    out.write_all(&layer_definition_table(program, &records))?;
    if version >= VERSION_517 {
        let print_height = slabs(program)
            .last()
            .map_or(0.0, |&(_, top)| round_height(top));
        out.seek(SeekFrom::Start(u64::from(addresses.model)))?;
        out.write_all(&model_table(&panel, bounds, print_height))?;
    }
    if version >= VERSION_518 {
        out.seek(SeekFrom::Start(u64::from(addresses.sub_layer_definition)))?;
        out.write_all(&sub_layer_definition_table(&records))?;
    }
    out.seek(SeekFrom::Start(start))?;
    out.write_all(&file_mark_table(version, &addresses))?;
    out.seek(SeekFrom::Start(u64::from(end)))?;
    out.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests_anycubic_binary;
