/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The fixed-size header every GOO V3.0 file opens with.
//!
//! The two previews account for 195,112 of its bytes and are written
//! black, which the machine's file browser shows as an empty
//! thumbnail.

use std::io::{self, Read, Write};

use sc_print_primitives::{AaLevel, LightPwm};

use crate::print_program::{LayerProgram, PrintProgram};

/// Header length, and so the offset the first layer record sits at.
pub const HEADER_SIZE: u32 = 195_477;

/// Bytes identifying the file, in the header and again in the footer.
pub const FILE_MAGIC: [u8; 8] = [0x07, 0x00, 0x00, 0x00, 0x44, 0x4C, 0x50, 0x00];

const FORMAT_VERSION: [u8; 4] = *b"V3.0";
const DELIMITER: [u8; 2] = [0x0D, 0x0A];

const SMALL_PREVIEW_BYTES: usize = 116 * 116 * 2;
const LARGE_PREVIEW_BYTES: usize = 290 * 290 * 2;

const SOFTWARE_NAME: &str = "SoulCrafted Slicer";

const GREY_LEVEL: u16 = 0;
const BLUR_LEVEL: u16 = 0;
/// Selects the 3 wait times over a single light-off time.
const DELAY_MODE_WAIT_TIMES: u8 = 1;
/// Read only under the light-off delay mode, which this writer does
/// not select.
const LIGHT_OFF_SEC: f32 = 0.0;
const ADVANCE_MODE: u8 = 0;
const PRINT_TIME_SEC: u32 = 0;
const TOTAL_VOLUME_MM3: f32 = 0.0;
const TOTAL_WEIGHT_G: f32 = 0.0;
const TOTAL_PRICE: f32 = 0.0;
const PRICE_UNIT: &str = "";
/// Pixel values span the whole byte, rather than the low nibble.
const GREY_SCALE_LEVEL: u8 = 1;

/// Write the whole header for `program`, previews included.
///
/// # Errors
///
/// Propagates `out`'s failures, and rejects a program holding more
/// layers than the header's count can express.
pub fn write_header(program: &PrintProgram, out: &mut impl Write) -> io::Result<()> {
    let layer_count = u32::try_from(program.layers.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "more layers than a header can count",
        )
    })?;
    let raster = &program.raster;
    let plate = raster.build_volume();
    let bottom = LayerSummary::of(program.layers.first());
    let normal = LayerSummary::of(program.layers.last());

    out.write_all(&FORMAT_VERSION)?;
    out.write_all(&FILE_MAGIC)?;
    write_text(out, SOFTWARE_NAME, 32)?;
    write_text(out, program.slicer_version, 24)?;
    write_text(out, program.created_at.as_str(), 24)?;
    write_text(out, program.machine_name.as_str(), 32)?;
    write_text(out, program.machine_name.as_str(), 32)?;
    write_text(out, program.material_name.as_str(), 32)?;
    write_u16(out, anti_aliasing_factor(raster.aa()))?;
    write_u16(out, GREY_LEVEL)?;
    write_u16(out, BLUR_LEVEL)?;
    write_zeros(out, SMALL_PREVIEW_BYTES)?;
    out.write_all(&DELIMITER)?;
    write_zeros(out, LARGE_PREVIEW_BYTES)?;
    out.write_all(&DELIMITER)?;

    write_u32(out, layer_count)?;
    write_u16(out, resolution_axis(raster.pixel_resolution().x()))?;
    write_u16(out, resolution_axis(raster.pixel_resolution().y()))?;
    write_flag(out, raster.mirror().x)?;
    write_flag(out, raster.mirror().y)?;
    write_f32(out, plate.x_mm() as f32)?;
    write_f32(out, plate.y_mm() as f32)?;
    write_f32(out, plate.z_mm() as f32)?;
    write_f32(out, program.layer_height_mm.value() as f32)?;
    write_f32(out, normal.exposure_sec)?;
    out.write_all(&[DELAY_MODE_WAIT_TIMES])?;
    write_f32(out, LIGHT_OFF_SEC)?;
    write_f32(out, bottom.wait_before_lift_sec)?;
    write_f32(out, bottom.wait_after_lift_sec)?;
    write_f32(out, bottom.wait_after_retract_sec)?;
    write_f32(out, normal.wait_before_lift_sec)?;
    write_f32(out, normal.wait_after_lift_sec)?;
    write_f32(out, normal.wait_after_retract_sec)?;
    write_f32(out, bottom.exposure_sec)?;
    write_u32(out, program.bottom_layer_count)?;
    write_f32(out, bottom.lift_distance_mm)?;
    write_f32(out, bottom.lift_speed_mm_min)?;
    write_f32(out, normal.lift_distance_mm)?;
    write_f32(out, normal.lift_speed_mm_min)?;
    write_f32(out, bottom.retract_distance_mm)?;
    write_f32(out, bottom.retract_speed_mm_min)?;
    write_f32(out, normal.retract_distance_mm)?;
    write_f32(out, normal.retract_speed_mm_min)?;
    write_f32(out, bottom.second_lift_distance_mm)?;
    write_f32(out, bottom.second_lift_speed_mm_min)?;
    write_f32(out, normal.second_lift_distance_mm)?;
    write_f32(out, normal.second_lift_speed_mm_min)?;
    write_f32(out, bottom.second_retract_distance_mm)?;
    write_f32(out, bottom.second_retract_speed_mm_min)?;
    write_f32(out, normal.second_retract_distance_mm)?;
    write_f32(out, normal.second_retract_speed_mm_min)?;
    write_u16(out, bottom.light_pwm)?;
    write_u16(out, normal.light_pwm)?;
    out.write_all(&[ADVANCE_MODE])?;
    write_u32(out, PRINT_TIME_SEC)?;
    write_f32(out, TOTAL_VOLUME_MM3)?;
    write_f32(out, TOTAL_WEIGHT_G)?;
    write_f32(out, TOTAL_PRICE)?;
    write_text(out, PRICE_UNIT, 8)?;
    write_u32(out, HEADER_SIZE)?;
    out.write_all(&[GREY_SCALE_LEVEL])?;
    write_u16(out, transition_count(program))
}

/// The header's copy of one layer's record, named as the header names
/// its slots. A program with no layers summarises to zeros.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct LayerSummary {
    exposure_sec: f32,
    wait_before_lift_sec: f32,
    wait_after_lift_sec: f32,
    wait_after_retract_sec: f32,
    lift_distance_mm: f32,
    lift_speed_mm_min: f32,
    retract_distance_mm: f32,
    retract_speed_mm_min: f32,
    second_lift_distance_mm: f32,
    second_lift_speed_mm_min: f32,
    second_retract_distance_mm: f32,
    second_retract_speed_mm_min: f32,
    light_pwm: u16,
}

impl LayerSummary {
    fn of(layer: Option<&LayerProgram>) -> Self {
        let Some(layer) = layer else {
            return Self::default();
        };
        let (second_lift_distance_mm, second_lift_speed_mm_min) =
            layer.second_lift.map_or((0.0, 0.0), |stage| {
                (
                    stage.distance_mm.value() as f32,
                    stage.speed_mm_min.value() as f32,
                )
            });
        let (second_retract_distance_mm, second_retract_speed_mm_min) =
            layer.second_retract.map_or((0.0, 0.0), |stage| {
                (
                    stage.distance_mm.value() as f32,
                    stage.speed_mm_min.value() as f32,
                )
            });

        Self {
            exposure_sec: layer.exposure_sec.value() as f32,
            // The header names a wait by the motion it precedes or
            // follows; a layer record names it by the exposure.
            wait_before_lift_sec: layer.wait_after_cure_sec.value() as f32,
            wait_after_lift_sec: layer.wait_after_lift_sec.value() as f32,
            wait_after_retract_sec: layer.wait_before_cure_sec.value() as f32,
            lift_distance_mm: layer.lift.distance_mm.value() as f32,
            lift_speed_mm_min: layer.lift.speed_mm_min.value() as f32,
            retract_distance_mm: layer.retract.distance_mm.value() as f32,
            retract_speed_mm_min: layer.retract.speed_mm_min.value() as f32,
            second_lift_distance_mm,
            second_lift_speed_mm_min,
            second_retract_distance_mm,
            second_retract_speed_mm_min,
            light_pwm: pwm_byte(layer.light_pwm),
        }
    }
}

/// Report the super-sampling factor the raster was produced at.
fn anti_aliasing_factor(aa: AaLevel) -> u16 {
    match aa {
        AaLevel::Off => 1,
        AaLevel::X2 => 2,
        AaLevel::X4 => 4,
        AaLevel::X8 => 8,
    }
}

/// Convert a duty cycle to the 0..255 scale the header stores.
fn pwm_byte(pwm: LightPwm) -> u16 {
    (pwm.percent() / 100.0 * 255.0).round() as u16
}

/// Narrow a pixel count to the 16 bits the header gives an axis.
fn resolution_axis(pixels: u32) -> u16 {
    u16::try_from(pixels).unwrap_or(u16::MAX)
}

fn transition_count(program: &PrintProgram) -> u16 {
    u16::try_from(program.transition_layer_count).unwrap_or(u16::MAX)
}

fn write_u16(out: &mut impl Write, value: u16) -> io::Result<()> {
    out.write_all(&value.to_be_bytes())
}

fn write_u32(out: &mut impl Write, value: u32) -> io::Result<()> {
    out.write_all(&value.to_be_bytes())
}

fn write_f32(out: &mut impl Write, value: f32) -> io::Result<()> {
    out.write_all(&value.to_be_bytes())
}

fn write_flag(out: &mut impl Write, flag: bool) -> io::Result<()> {
    out.write_all(&[u8::from(flag)])
}

/// Write `text` into a fixed-width slot, cut at a character boundary
/// so at least one zero byte terminates it.
fn write_text(out: &mut impl Write, text: &str, width: usize) -> io::Result<()> {
    let mut end = text.len().min(width - 1);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    out.write_all(&text.as_bytes()[..end])?;
    write_zeros(out, width - end)
}

fn write_zeros(out: &mut impl Write, count: usize) -> io::Result<()> {
    io::copy(&mut io::repeat(0).take(count as u64), out)?;
    Ok(())
}
