/*
 * SPDX-FileCopyrightText: 2020-2026 PTRTECH, Tiago Conceição (sn4k3) and UVtools contributors
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 *
 * Ported to Rust from UVtools UVtools.Core/FileFormats/ChituboxFile.cs (Header,
 * PrintParameters, SlicerInfo, PrintParametersV4, LayerDef, LayerDefEx,
 * SanitizeMagicVersion, OnBeforeEncode and EncodeInternally) and
 * UVtools.Core/FileFormats/FileFormat.cs with
 * UVtools.Core/Operations/OperationCalculator.cs (the light-off delay of
 * formats without waits), UVtools v7.1.1, commit
 * 8b5c91fd41b60bd6265ba1c1f2b2b64636b873e8, modified for Breadslicer: the
 * layers are encoded from the slicer's run-length rows on every core with the
 * CTB codecs of sc-sla-formats, the layer key is derived from the print instead
 * of drawn at random, and the print settings come from a Breadslicer print
 * program.
 *
 * This program is free software: you can redistribute it and/or modify it
 * under the terms of the GNU Affero General Public License as published by the
 * Free Software Foundation, either version 3 of the License, or (at your
 * option) any later version. It is distributed WITHOUT ANY WARRANTY; see the
 * GNU Affero General Public License (LICENSE.md) for details.
 */

//! ChiTuBox CTB versions 2, 3 and 4, the revisions before the version 5 that
//! `sc-sla-formats` writes itself.
//!
//! The file is the same skeleton in every revision: a header, 2 previews, the
//! print parameters, the slicer info with the machine name, a layer table and
//! the layer images, all little-endian. The revisions differ in what they add:
//!
//! - 2: one motion for the whole file. The wait before each exposure goes into
//!   the light-off delay, which counts the platform's travel as well.
//! - 3: a 48-byte motion record before each layer image.
//! - 4: a new magic, the 2-stage motion and the waits in their own fields, and
//!   the vendor's disclaimer with a block of further print parameters.
//!
//! The layer images are 7-bit greys in the CTB run-length codec, XORed with a
//! keystream from the file's key and each layer's index, as UVtools writes
//! them.

use std::io::SeekFrom;
use std::num::NonZeroU32;

use sc_sl1cer::contour::ContourStack;
use sc_sla_formats::ctb::disclaimer::{DISCLAIMER_SIZE, write_disclaimer};
use sc_sla_formats::ctb::v5::preview_record::write_preview_record;
use sc_sla_formats::ctb::{PREVIEW_SIZES, layer_xor, rle7, rle15};
use sc_sla_formats::layer_images::write_layer_images;
use sc_sla_formats::{
    CtbVariant, LayerProgram, PreviewImage, PrintPreviews, PrintProgram, PrinterFileSink,
    WriteFailure,
};

use crate::program::{
    ProgramSummary, pwm_byte, round_away, round_display, round_even, round_height, slabs,
    uses_per_layer_settings,
};

/// The magic of versions 2 and 3.
pub(crate) const MAGIC_CTB: u32 = 0x12FD_0086;
/// The magic from version 4.
pub(crate) const MAGIC_CTB_V4: u32 = 0x12FD_0106;

pub(crate) const HEADER_BYTES: u32 = 112;
const PREVIEW_RECORD_BYTES: u32 = 32;
pub(crate) const PRINT_PARAMETERS_BYTES: u32 = 60;
pub(crate) const SLICER_INFO_BYTES: u32 = 76;
pub(crate) const PRINT_PARAMETERS_V4_BYTES: u32 = 464;
pub(crate) const TABLE_RECORD_BYTES: u32 = 36;
/// A layer's motion record: its table record again, then 48 bytes.
pub(crate) const LAYER_RECORD_BYTES: u32 = TABLE_RECORD_BYTES + 48;

/// What version 2 leaves in its last padding word, as UVtools writes it.
pub(crate) const V2_PADDING_MARK: u32 = 0x1234;

/// What the vendor's slicer 1.9.0 reports, which UVtools writes in every
/// revision.
const SOFTWARE_VERSION: u32 = 0x0109_0000;

const ANTI_ALIAS_OFF: u8 = 0x07;
const ANTI_ALIAS_ON: u8 = 0x0F;

/// The 2 slots of the version 4 block that UVtools fills with 4, and the one it
/// fills with 5; nothing identifies them.
const V4_FOUR: f32 = 4.0;
const V4_FIVE: u32 = 5;

/// The reserved tail of the version 4 block.
const V4_RESERVED_BYTES: usize = 380;

/// Reports the revision number a variant writes.
pub(crate) fn version_of(variant: CtbVariant) -> Option<u32> {
    match variant {
        CtbVariant::V2 => Some(2),
        CtbVariant::V3 => Some(3),
        CtbVariant::V4 => Some(4),
        CtbVariant::PlainV5 | CtbVariant::EncryptedV5 => None,
    }
}

/// One layer's settings, named as UVtools names them for CTB, speeds in
/// millimetres per minute.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Motion {
    exposure_sec: f32,
    wait_before_cure_sec: f32,
    wait_after_cure_sec: f32,
    wait_after_lift_sec: f32,
    lift_mm: f32,
    lift_mm_min: f32,
    retract_mm_min: f32,
    second_lift_mm: f32,
    second_lift_mm_min: f32,
    second_retract_mm: f32,
    second_retract_mm_min: f32,
    pwm: u8,
}

impl Motion {
    fn of(layer: Option<&LayerProgram>) -> Self {
        let Some(layer) = layer else {
            return Self::default();
        };
        let (second_lift_mm, second_lift_mm_min) = layer.second_lift.map_or((0.0, 0.0), |stage| {
            (
                stage.distance_mm.value() as f32,
                stage.speed_mm_min.value() as f32,
            )
        });
        let (second_retract_mm, second_retract_mm_min) =
            layer.second_retract.map_or((0.0, 0.0), |stage| {
                (
                    stage.distance_mm.value() as f32,
                    stage.speed_mm_min.value() as f32,
                )
            });
        Self {
            exposure_sec: round_away(layer.exposure_sec.value() as f32, 2),
            wait_before_cure_sec: round_away(layer.wait_before_cure_sec.value() as f32, 2),
            wait_after_cure_sec: round_away(layer.wait_after_cure_sec.value() as f32, 2),
            wait_after_lift_sec: round_away(layer.wait_after_lift_sec.value() as f32, 2),
            lift_mm: round_away(layer.lift.distance_mm.value() as f32, 2),
            lift_mm_min: round_away(layer.lift.speed_mm_min.value() as f32, 2),
            retract_mm_min: round_away(layer.retract.speed_mm_min.value() as f32, 2),
            second_lift_mm: round_away(second_lift_mm, 2),
            second_lift_mm_min: round_away(second_lift_mm_min, 2),
            second_retract_mm: round_away(second_retract_mm, 2),
            second_retract_mm_min: round_away(second_retract_mm_min, 2),
            pwm: pwm_byte(layer.light_pwm),
        }
    }

    /// The whole rise, both stages together.
    fn total_lift_mm(&self) -> f32 {
        round_height(self.lift_mm + self.second_lift_mm)
    }

    /// The delay a revision without waits keeps the light off for: the time
    /// the platform takes to rise and return, plus the wait before the
    /// exposure. As in UVtools it is set only when there is such a wait, and
    /// from version 4 the wait has its own field and the delay stays 0.
    fn light_off_sec(&self, version: u32) -> f32 {
        if version >= 4 || self.wait_before_cure_sec <= 0.0 {
            return 0.0;
        }
        let lift = self.total_lift_mm();
        let mut seconds = round_even(self.wait_before_cure_sec, 2);
        if lift > 0.0 && self.lift_mm_min > 0.0 {
            seconds += lift / (self.lift_mm_min / 60.0);
        }
        if lift > 0.0 && self.retract_mm_min > 0.0 {
            seconds += lift / (self.retract_mm_min / 60.0);
        }
        round_even(seconds, 2)
    }
}

/// The settings the header and the parameter blocks summarise: the first
/// layer's, and the first layer's past the bottom and transition layers.
fn summary(program: &PrintProgram) -> (Motion, Motion) {
    let first_normal = program
        .bottom_layer_count
        .saturating_add(program.transition_layer_count) as usize;
    let normal = program
        .layers
        .get(first_normal)
        .or_else(|| program.layers.last());
    (Motion::of(program.layers.first()), Motion::of(normal))
}

/// The key the layer images are XORed with. UVtools draws it at random, from
/// 255 up to the largest 32-bit signed integer; this writer derives it from
/// when the print was made, so the same print always gives the same bytes.
pub(crate) fn layer_key(program: &PrintProgram) -> NonZeroU32 {
    const LOWEST: u32 = 0xFF;
    const SPAN: u32 = i32::MAX as u32 - LOWEST;
    let mixed = (program.created_at.unix_sec as u32).wrapping_mul(0x9E37_79B9);
    NonZeroU32::new(LOWEST + mixed % SPAN).expect("the key is at least 255")
}

/// Where each section lands, from the sizes of the variable ones.
#[derive(Clone, Copy, Debug)]
struct Layout {
    large_preview: u32,
    small_preview: u32,
    print_parameters: u32,
    slicer_info: u32,
    machine_name: u32,
    disclaimer: u32,
    print_parameters_v4: u32,
    layer_table: u32,
    layer_data: u64,
}

impl Layout {
    fn plan(
        version: u32,
        large_blob: u32,
        small_blob: u32,
        machine_name: u32,
        layers: u32,
    ) -> Self {
        let large_preview = HEADER_BYTES;
        let small_preview = large_preview + PREVIEW_RECORD_BYTES + large_blob;
        let print_parameters = small_preview + PREVIEW_RECORD_BYTES + small_blob;
        let slicer_info = print_parameters + PRINT_PARAMETERS_BYTES;
        let name = slicer_info + SLICER_INFO_BYTES;
        let after_name = name + machine_name;
        let (disclaimer, print_parameters_v4, layer_table) = if version >= 4 {
            let block = after_name + DISCLAIMER_SIZE;
            (after_name, block, block + PRINT_PARAMETERS_V4_BYTES)
        } else {
            (0, 0, after_name)
        };
        Self {
            large_preview,
            small_preview,
            print_parameters,
            slicer_info,
            machine_name: name,
            disclaimer,
            print_parameters_v4,
            layer_table,
            layer_data: u64::from(layer_table) + u64::from(TABLE_RECORD_BYTES) * u64::from(layers),
        }
    }
}

fn put_u8(out: &mut Vec<u8>, value: u8) {
    out.push(value);
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

fn header(
    version: u32,
    program: &PrintProgram,
    layout: &Layout,
    key: NonZeroU32,
    (bottom, normal): (&Motion, &Motion),
) -> Vec<u8> {
    let raster = &program.raster;
    let resolution = raster.panel_resolution();
    let (width_mm, height_mm) = raster.panel_size_mm();
    let print_height = slabs(program)
        .last()
        .map_or(0.0, |&(_, top)| round_height(top));

    let mut out = Vec::with_capacity(HEADER_BYTES as usize);
    put_u32(
        &mut out,
        if version >= 4 {
            MAGIC_CTB_V4
        } else {
            MAGIC_CTB
        },
    );
    put_u32(&mut out, version);
    put_f32(&mut out, round_display(width_mm as f32));
    put_f32(&mut out, round_display(height_mm as f32));
    put_f32(&mut out, round_away(raster.build_volume().z_mm() as f32, 2));
    put_u32(&mut out, 0);
    put_u32(&mut out, 0);
    put_f32(&mut out, print_height);
    put_f32(
        &mut out,
        round_height(program.layer_height_mm.value() as f32),
    );
    put_f32(&mut out, normal.exposure_sec);
    put_f32(&mut out, bottom.exposure_sec);
    put_f32(&mut out, round_away(normal.light_off_sec(version), 2));
    put_u32(&mut out, program.bottom_layer_count);
    put_u32(&mut out, resolution.x());
    put_u32(&mut out, resolution.y());
    put_u32(&mut out, layout.large_preview);
    put_u32(&mut out, layout.layer_table);
    put_u32(&mut out, program.layers.len() as u32);
    put_u32(&mut out, layout.small_preview);
    put_u32(&mut out, program.print_time_sec.value() as u32);
    put_u32(&mut out, u32::from(raster.mirror().x)); // the projector is mirrored
    put_u32(&mut out, layout.print_parameters);
    put_u32(&mut out, PRINT_PARAMETERS_BYTES);
    put_u32(&mut out, 1); // CTB stores its greys once, not one pass per level
    put_u16(&mut out, u16::from(normal.pwm));
    put_u16(&mut out, u16::from(bottom.pwm));
    put_u32(&mut out, key.get());
    put_u32(&mut out, layout.slicer_info);
    put_u32(&mut out, SLICER_INFO_BYTES);
    debug_assert_eq!(out.len() as u32, HEADER_BYTES);
    out
}

fn print_parameters(
    version: u32,
    program: &PrintProgram,
    (bottom, normal): (&Motion, &Motion),
) -> Vec<u8> {
    let mut out = Vec::with_capacity(PRINT_PARAMETERS_BYTES as usize);
    put_f32(&mut out, bottom.total_lift_mm());
    put_f32(&mut out, bottom.lift_mm_min);
    put_f32(&mut out, normal.total_lift_mm());
    put_f32(&mut out, normal.lift_mm_min);
    put_f32(&mut out, normal.retract_mm_min);
    put_f32(
        &mut out,
        round_away((program.volume_mm3.value() / 1000.0) as f32, 3),
    );
    put_f32(&mut out, round_away(program.mass_g.value() as f32, 3));
    put_f32(&mut out, 0.0); // nothing here prices a print
    put_f32(&mut out, round_even(bottom.light_off_sec(version), 2));
    put_f32(&mut out, round_away(normal.light_off_sec(version), 2));
    put_u32(&mut out, program.bottom_layer_count);
    put_u32(&mut out, 0);
    put_u32(&mut out, 0);
    put_u32(&mut out, 0);
    put_u32(&mut out, if version <= 2 { V2_PADDING_MARK } else { 0 });
    debug_assert_eq!(out.len() as u32, PRINT_PARAMETERS_BYTES);
    out
}

fn slicer_info(
    version: u32,
    program: &PrintProgram,
    layout: &Layout,
    (bottom, normal): (&Motion, &Motion),
) -> Vec<u8> {
    let aa = program.raster.aa().factor();
    let layer_count = program.layers.len() as u32;
    let most_transition = if program.bottom_layer_count == 0 {
        0
    } else {
        layer_count.saturating_sub(program.bottom_layer_count + 1)
    };
    // Version 2 offers no per-layer settings; later ones flag them with the
    // revision in the high nibble when a layer departs from its group.
    let per_layer = version >= 3 && uses_per_layer_settings(program, &ProgramSummary::of(program));
    let staged = version >= 4;

    let mut out = Vec::with_capacity(SLICER_INFO_BYTES as usize);
    let second = |value: f32| if staged { value } else { 0.0 };
    put_f32(&mut out, second(bottom.second_lift_mm));
    put_f32(&mut out, second(bottom.second_lift_mm_min));
    put_f32(&mut out, second(normal.second_lift_mm));
    put_f32(&mut out, second(normal.second_lift_mm_min));
    put_f32(&mut out, second(normal.second_retract_mm));
    put_f32(&mut out, second(normal.second_retract_mm_min));
    put_f32(&mut out, second(normal.wait_after_lift_sec));
    put_u32(&mut out, layout.machine_name);
    put_u32(&mut out, program.machine_name.as_str().len() as u32);
    put_u8(
        &mut out,
        if aa > 1 {
            ANTI_ALIAS_ON
        } else {
            ANTI_ALIAS_OFF
        },
    );
    put_u16(&mut out, 0);
    put_u8(&mut out, if per_layer { (version * 0x10) as u8 } else { 0 });
    put_u32(
        &mut out,
        u32::try_from(program.created_at.unix_sec / 60).unwrap_or_default(),
    );
    put_u32(&mut out, u32::from(aa));
    put_u32(&mut out, SOFTWARE_VERSION);
    put_f32(&mut out, second(normal.wait_before_cure_sec));
    put_f32(&mut out, second(normal.wait_after_lift_sec));
    put_u32(
        &mut out,
        program.transition_layer_count.min(most_transition),
    );
    put_u32(&mut out, layout.print_parameters_v4);
    put_u32(&mut out, 0);
    put_u32(&mut out, 0);
    debug_assert_eq!(out.len() as u32, SLICER_INFO_BYTES);
    out
}

fn print_parameters_v4(
    program: &PrintProgram,
    layout: &Layout,
    (bottom, normal): (&Motion, &Motion),
) -> Vec<u8> {
    let mut out = Vec::with_capacity(PRINT_PARAMETERS_V4_BYTES as usize);
    put_f32(&mut out, bottom.retract_mm_min);
    put_f32(&mut out, bottom.second_retract_mm_min);
    put_u32(&mut out, 0);
    put_f32(&mut out, V4_FOUR);
    put_u32(&mut out, 0);
    put_f32(&mut out, V4_FOUR);
    put_f32(&mut out, normal.wait_before_cure_sec);
    put_f32(&mut out, normal.wait_after_lift_sec);
    put_f32(&mut out, normal.wait_after_cure_sec);
    put_f32(&mut out, bottom.second_retract_mm);
    put_f32(&mut out, 0.0);
    put_u32(&mut out, 0);
    put_u32(&mut out, V4_FIVE);
    put_u32(&mut out, (program.layers.len() as u32).saturating_sub(1)); // the last layer's index
    out.resize(out.len() + 4 * 4, 0);
    put_u32(&mut out, layout.disclaimer);
    put_u32(&mut out, DISCLAIMER_SIZE);
    put_u32(&mut out, 0); // the resin block arrives in version 5
    out.resize(out.len() + V4_RESERVED_BYTES, 0);
    debug_assert_eq!(out.len() as u32, PRINT_PARAMETERS_V4_BYTES);
    out
}

/// Where a layer's image is: its offset in a 4 GiB page, its length and the
/// page.
#[derive(Clone, Copy, Debug)]
struct ImageLocation {
    address: u64,
    length: u32,
}

fn table_record(
    version: u32,
    slab_top_mm: f32,
    motion: &Motion,
    image: ImageLocation,
    out: &mut Vec<u8>,
) {
    put_f32(out, round_height(slab_top_mm));
    put_f32(out, motion.exposure_sec);
    put_f32(out, round_away(motion.light_off_sec(version), 2));
    put_u32(out, image.address as u32);
    put_u32(out, image.length);
    put_u32(out, (image.address >> 32) as u32);
    put_u32(
        out,
        if version >= 3 {
            LAYER_RECORD_BYTES
        } else {
            TABLE_RECORD_BYTES
        },
    );
    put_u32(out, 0);
    put_u32(out, 0);
}

/// The motion record before a layer image: its table record, then the motion.
/// Version 3 carries the single-stage motion and the light; version 4 adds the
/// second stage and the waits.
fn layer_record(version: u32, slab_top_mm: f32, motion: &Motion, image: ImageLocation) -> Vec<u8> {
    let staged = version >= 4;
    let second = |value: f32| if staged { value } else { 0.0 };
    let mut out = Vec::with_capacity(LAYER_RECORD_BYTES as usize);
    table_record(version, slab_top_mm, motion, image, &mut out);
    put_u32(
        &mut out,
        if image.length > 0 {
            LAYER_RECORD_BYTES + image.length
        } else {
            0
        },
    );
    put_f32(&mut out, motion.total_lift_mm());
    put_f32(&mut out, motion.lift_mm_min);
    put_f32(&mut out, second(motion.second_lift_mm));
    put_f32(&mut out, second(motion.second_lift_mm_min));
    put_f32(&mut out, motion.retract_mm_min);
    put_f32(&mut out, second(motion.second_retract_mm));
    put_f32(&mut out, second(motion.second_retract_mm_min));
    put_f32(&mut out, second(motion.wait_after_cure_sec));
    put_f32(&mut out, second(motion.wait_after_lift_sec));
    put_f32(&mut out, second(motion.wait_before_cure_sec));
    put_f32(&mut out, f32::from(motion.pwm));
    debug_assert_eq!(out.len() as u32, LAYER_RECORD_BYTES);
    out
}

/// Writes `stack` as a CTB file of `variant` (versions 2 to 4) running the
/// settings in `program`.
///
/// Every layer image is the plate's image that [`write_layer_images`]
/// describes, as in the other CTB revisions; the raster setup's mirror flags
/// turn it around and its panel rotation turns it a quarter. Without
/// `previews` the machine shows black thumbnails.
///
/// # Errors
///
/// [`WriteFailure::PreviewSize`] when a rendering is not the size this format
/// renders at and [`WriteFailure::Unencodable`] for a variant this writer does
/// not cover or a file its 32-bit offsets cannot reach, the first 2 before any
/// byte is written; [`WriteFailure::Io`] when it cannot be written.
pub fn write_file(
    variant: CtbVariant,
    program: &PrintProgram,
    previews: Option<&PrintPreviews>,
    stack: &ContourStack,
    out: &mut dyn PrinterFileSink,
) -> Result<(), WriteFailure> {
    let version = version_of(variant).ok_or_else(|| {
        WriteFailure::Unencodable(format!(
            "CTB {variant:?} is written by sc-sla-formats, not ported from UVtools"
        ))
    })?;
    let (large, small) = match previews {
        Some(previews) => {
            previews.large.check_size(PREVIEW_SIZES.large)?;
            previews.small.check_size(PREVIEW_SIZES.small)?;
            (previews.large.clone(), previews.small.clone())
        }
        None => (
            PreviewImage::black(PREVIEW_SIZES.large),
            PreviewImage::black(PREVIEW_SIZES.small),
        ),
    };
    let mut large_blob = Vec::new();
    rle15::encode_preview(&large, &mut large_blob);
    let mut small_blob = Vec::new();
    rle15::encode_preview(&small, &mut small_blob);

    let unreachable = || {
        WriteFailure::Unencodable(format!(
            "the file passes the {} bytes its 32-bit offsets reach",
            u32::MAX
        ))
    };
    let machine_name = program.machine_name.as_str().as_bytes();
    let layer_count = u32::try_from(program.layers.len()).map_err(|_| unreachable())?;
    let layout = Layout::plan(
        version,
        u32::try_from(large_blob.len()).map_err(|_| unreachable())?,
        u32::try_from(small_blob.len()).map_err(|_| unreachable())?,
        u32::try_from(machine_name.len()).map_err(|_| unreachable())?,
        layer_count,
    );
    if layout.layer_data > u64::from(u32::MAX) {
        return Err(unreachable());
    }

    let key = layer_key(program);
    let (bottom, normal) = summary(program);
    let mut sections = header(version, program, &layout, key, (&bottom, &normal));
    write_preview_record(
        &large,
        &large_blob,
        layout.large_preview + PREVIEW_RECORD_BYTES,
        &mut sections,
    )?;
    sections.extend_from_slice(&large_blob);
    write_preview_record(
        &small,
        &small_blob,
        layout.small_preview + PREVIEW_RECORD_BYTES,
        &mut sections,
    )?;
    sections.extend_from_slice(&small_blob);
    sections.extend(print_parameters(version, program, (&bottom, &normal)));
    sections.extend(slicer_info(version, program, &layout, (&bottom, &normal)));
    sections.extend_from_slice(machine_name);
    if version >= 4 {
        write_disclaimer(&mut sections)?;
        sections.extend(print_parameters_v4(program, &layout, (&bottom, &normal)));
    }
    debug_assert_eq!(sections.len() as u32, layout.layer_table);

    let start = out.stream_position()?;
    out.write_all(&sections)?;

    // The layer table is filled once the images are placed.
    let table_bytes = TABLE_RECORD_BYTES as usize * program.layers.len();
    out.write_all(&vec![0; table_bytes])?;
    let mut table = Vec::with_capacity(table_bytes);
    let tops = slabs(program);
    let mut next = layout.layer_data;
    write_layer_images(
        stack,
        &program.raster,
        rle7::DIMMEST_STORED,
        |index, runs| {
            let mut image = Vec::new();
            rle7::encode_layer(runs, &mut image);
            layer_xor::apply(key, index as u32, &mut image);
            image
        },
        |index, image: Vec<u8>| -> Result<(), WriteFailure> {
            let motion = Motion::of(program.layers.get(index));
            let top = tops.get(index).map_or(0.0, |&(_, top)| top);
            let length = u32::try_from(image.len()).map_err(|_| unreachable())?;
            if version >= 3 {
                let image_at = ImageLocation {
                    address: next + u64::from(LAYER_RECORD_BYTES),
                    length,
                };
                out.write_all(&layer_record(version, top, &motion, image_at))?;
                next += u64::from(LAYER_RECORD_BYTES);
            }
            let image_at = ImageLocation {
                address: next,
                length,
            };
            table_record(version, top, &motion, image_at, &mut table);
            out.write_all(&image)?;
            next += u64::from(length);
            Ok(())
        },
    )?;

    let end = out.stream_position()?;
    out.seek(SeekFrom::Start(start + u64::from(layout.layer_table)))?;
    out.write_all(&table)?;
    out.seek(SeekFrom::Start(end))?;
    out.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests_ctb_legacy;
