/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The fixed-size header every GOO V3.0 file opens with.
//!
//! The 2 previews account for 195,112 of its bytes. A caller with
//! nothing rendered leaves them black, which the machine's file
//! browser shows as an empty thumbnail.

use std::io::{self, Write};

use sc_print_primitives::AaLevel;

use super::record::LayerValues;
use super::wire::{write_f32, write_flag, write_text, write_u16, write_u32, write_zeros};
use crate::preview::{PreviewImage, PreviewSize, PreviewSizeMismatch, PreviewSizes, PrintPreviews};
use crate::print_program::PrintProgram;

/// Header length, and so the offset the first layer record sits at.
pub const HEADER_SIZE: u32 = 195_477;

/// Bytes identifying the file, in the header and again in the footer.
pub const FILE_MAGIC: [u8; 8] = [0x07, 0x00, 0x00, 0x00, 0x44, 0x4C, 0x50, 0x00];

const FORMAT_VERSION: [u8; 4] = *b"V3.0";
const DELIMITER: [u8; 2] = [0x0D, 0x0A];

/// Sizes this container fixes for the preview its file browser
/// lists and the one that entry opens.
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

const RGB565_BYTES: usize = 2;
const SMALL_PREVIEW_BYTES: usize =
    (PREVIEW_SIZES.small.width * PREVIEW_SIZES.small.height) as usize * RGB565_BYTES;
const LARGE_PREVIEW_BYTES: usize =
    (PREVIEW_SIZES.large.width * PREVIEW_SIZES.large.height) as usize * RGB565_BYTES;

const SOFTWARE_NAME: &str = "SoulCrafted Slicer";

const GREY_LEVEL: u16 = 0;
const BLUR_LEVEL: u16 = 0;
/// Selects the 3 wait times over a single light-off time.
const DELAY_MODE_WAIT_TIMES: u8 = 1;
/// Read only under the light-off delay mode, which this writer does
/// not select.
const LIGHT_OFF_SEC: f32 = 0.0;
const ADVANCE_MODE: u8 = 0;
const TOTAL_PRICE: f32 = 0.0;
const PRICE_UNIT: &str = "";
/// Pixel values span the whole byte, rather than the low nibble.
const GREY_SCALE_LEVEL: u8 = 1;

/// The renderings this container will accept, checked once so that
/// writing them cannot fail on their size.
#[derive(Clone, Copy, Debug)]
pub struct SizedPreviews<'a> {
    small: &'a PreviewImage,
    large: &'a PreviewImage,
}

impl<'a> SizedPreviews<'a> {
    /// Accept a rendered pair for this container.
    ///
    /// # Errors
    ///
    /// [`PreviewSizeMismatch`] for the first rendering whose size is
    /// not the one the container fixes. Resampling belongs to
    /// whoever rendered the image, so a near miss is refused.
    pub fn check(previews: &'a PrintPreviews) -> Result<Self, PreviewSizeMismatch> {
        check_size(&previews.small, PREVIEW_SIZES.small)?;
        check_size(&previews.large, PREVIEW_SIZES.large)?;
        Ok(Self {
            small: &previews.small,
            large: &previews.large,
        })
    }
}

fn check_size(image: &PreviewImage, size: PreviewSize) -> Result<(), PreviewSizeMismatch> {
    if image.width() == size.width && image.height() == size.height {
        return Ok(());
    }
    Err(PreviewSizeMismatch {
        expected_width: size.width,
        expected_height: size.height,
        actual_width: image.width(),
        actual_height: image.height(),
    })
}

/// Write the whole header for `program`, previews included. Without
/// `previews` both images are written black, which the machine shows
/// as an empty thumbnail.
///
/// # Errors
///
/// Propagates `out`'s failures, and rejects a program holding more
/// layers than the header's count can express.
pub fn write_header(
    program: &PrintProgram,
    previews: Option<SizedPreviews<'_>>,
    out: &mut impl Write,
) -> io::Result<()> {
    let layer_count = u32::try_from(program.layers.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "more layers than a header can count",
        )
    })?;
    let raster = &program.raster;
    let plate = raster.build_volume();
    let bottom = LayerValues::of(program.layers.first());
    let normal = LayerValues::of(program.layers.last());

    out.write_all(&FORMAT_VERSION)?;
    out.write_all(&FILE_MAGIC)?;
    write_text(out, SOFTWARE_NAME, 32)?;
    write_text(out, program.slicer_version, 24)?;
    write_text(out, program.created_at.rendered.as_str(), 24)?;
    write_text(out, program.machine_name.as_str(), 32)?;
    write_text(out, program.machine_name.as_str(), 32)?;
    write_text(out, program.material_name.as_str(), 32)?;
    write_u16(out, anti_aliasing_factor(raster.aa()))?;
    write_u16(out, GREY_LEVEL)?;
    write_u16(out, BLUR_LEVEL)?;
    match previews {
        Some(previews) => write_preview(out, previews.small)?,
        None => write_zeros(out, SMALL_PREVIEW_BYTES)?,
    }
    out.write_all(&DELIMITER)?;
    match previews {
        Some(previews) => write_preview(out, previews.large)?,
        None => write_zeros(out, LARGE_PREVIEW_BYTES)?,
    }
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
    write_u32(out, program.print_time_sec.value() as u32)?;
    write_f32(out, program.volume_mm3.value() as f32)?;
    write_f32(out, program.mass_g.value() as f32)?;
    write_f32(out, TOTAL_PRICE)?;
    write_text(out, PRICE_UNIT, 8)?;
    write_u32(out, HEADER_SIZE)?;
    out.write_all(&[GREY_SCALE_LEVEL])?;
    write_u16(out, transition_count(program))
}

/// Pack one rendering as RGB565, dropping the low bits of each
/// channel.
fn write_preview(out: &mut impl Write, image: &PreviewImage) -> io::Result<()> {
    for [red, green, blue] in image.rgb().as_chunks::<3>().0 {
        let red = u16::from(red >> 3);
        let green = u16::from(green >> 2);
        let blue = u16::from(blue >> 3);
        write_u16(out, red << 11 | green << 5 | blue)?;
    }
    Ok(())
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

/// Narrow a pixel count to the 16 bits the header gives an axis.
fn resolution_axis(pixels: u32) -> u16 {
    u16::try_from(pixels).unwrap_or(u16::MAX)
}

fn transition_count(program: &PrintProgram) -> u16 {
    u16::try_from(program.transition_layer_count).unwrap_or(u16::MAX)
}
