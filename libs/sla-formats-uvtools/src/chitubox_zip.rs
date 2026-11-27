/*
 * SPDX-FileCopyrightText: 2020-2026 PTRTECH, Tiago Conceição (sn4k3) and UVtools contributors
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 *
 * Ported to Rust from UVtools UVtools.Core/FileFormats/ChituboxZipFile.cs
 * (with the archive helpers of UVtools.Core/FileFormats/FileFormat.cs),
 * UVtools v7.1.1, commit 8b5c91fd41b60bd6265ba1c1f2b2b64636b873e8, modified
 * for Breadslicer: the layers are encoded from the slicer's run-length rows,
 * the settings come from a Breadslicer print program, and the end raise runs
 * at the fastest speed the program uses.
 *
 * This program is free software: you can redistribute it and/or modify it
 * under the terms of the GNU Affero General Public License as published by the
 * Free Software Foundation, either version 3 of the License, or (at your
 * option) any later version. It is distributed WITHOUT ANY WARRANTY; see the
 * GNU Affero General Public License (LICENSE.md) for details.
 */

//! ChiTuBox's ZIP: `.zip` for the Phrozen Sonic and Shuffle family, the
//! Wanhao CGR and D8 and the Kelant S400 (#778).
//!
//! The archive holds `run.gcode` (the settings as `;key:value` comments, then
//! the G-code that runs the machine), `preview.png` and
//! `preview_cropping.png`, and the layers as `1.png`, `2.png`, ...: 8-bit grey
//! images the size of the panel.

use std::io;

use sc_sl1cer::contour::ContourStack;
use sc_sl1cer::raster::rle::RleRun;
use sc_sla_formats::layer_images::write_layer_images;
use sc_sla_formats::{
    PreviewImage, PreviewSize, PreviewSizes, PrintPreviews, PrintProgram, PrinterFileSink,
    WriteFailure,
};
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::gcode::{Builder, Dialect, Job, MoveCommand, NoHooks, layers_of, num};
use crate::program::{ProgramSummary, round_away, round_display, round_height};
use crate::resample::png_bytes;
use crate::sl1::{entry, grey_png, zip_failure};

/// `preview.png` and `preview_cropping.png`, as ChiTuBox writes them.
pub const PREVIEW_SIZES: PreviewSizes = PreviewSizes {
    small: PreviewSize {
        width: 168,
        height: 150,
    },
    large: PreviewSize {
        width: 954,
        height: 850,
    },
};

pub(crate) const GCODE_FILE: &str = "run.gcode";
pub(crate) const PREVIEW_FILE: &str = "preview.png";
pub(crate) const PREVIEW_CROPPING_FILE: &str = "preview_cropping.png";

/// The job name the archive states.
const FILE_NAME: &str = "breadslicer";

/// The G-code ChiTuBox's ZIP runs: absolute moves in mm/min, waits in
/// milliseconds, `M6054 "1.png"` for the first layer, layers lifted with `G0`
/// and the end raise with `G1`.
pub(crate) fn dialect() -> Dialect {
    Dialect {
        layer_move: MoveCommand::G0,
        end_move: MoveCommand::G1,
        ..Dialect::default()
    }
}

/// The settings block at the head of `run.gcode`, in the order ChiTuBox
/// writes it.
fn header(program: &PrintProgram) -> String {
    let summary = ProgramSummary::of(program);
    let (bottom, normal) = (summary.bottom, summary.normal);
    let raster = &program.raster;
    let panel = raster.panel_resolution();
    let (width_mm, height_mm) = raster.panel_size_mm();
    let mirror = raster.mirror();
    let mirrored = mirror.x || mirror.y;
    let r2 = |value: f32| num(round_away(value, 2));
    let lines: [(&str, String); 32] = [
        ("fileName", FILE_NAME.to_owned()),
        ("machineType", program.machine_name.as_str().to_owned()),
        (
            "estimatedPrintTime",
            num(round_away(program.print_time_sec.value() as f32, 3)),
        ),
        (
            "volume",
            num(round_away((program.volume_mm3.value() / 1000.0) as f32, 3)),
        ),
        ("resin", program.material_name.as_str().to_owned()),
        ("weight", num(round_away(program.mass_g.value() as f32, 3))),
        ("price", "0".to_owned()),
        (
            "layerHeight",
            num(round_height(program.layer_height_mm.value() as f32)),
        ),
        ("resolutionX", panel.x().to_string()),
        ("resolutionY", panel.y().to_string()),
        ("machineX", num(round_display(width_mm as f32))),
        ("machineY", num(round_display(height_mm as f32))),
        (
            "machineZ",
            num(round_away(raster.build_volume().z().value() as f32, 2)),
        ),
        (
            "projectType",
            if mirrored { "LCD_mirror" } else { "Normal" }.to_owned(),
        ),
        ("normalExposureTime", r2(normal.exposure_sec)),
        ("bottomLayExposureTime", r2(bottom.exposure_sec)),
        ("bottomLayerExposureTime", r2(bottom.exposure_sec)),
        ("normalDropSpeed", r2(normal.retract_mm_min)),
        ("normalLayerLiftSpeed", r2(normal.lift_mm_min)),
        ("normalLayerLiftHeight", r2(normal.lift_mm)),
        ("zSlowUpDistance", "0".to_owned()),
        ("bottomLayCount", program.bottom_layer_count.to_string()),
        ("bottomLayerCount", program.bottom_layer_count.to_string()),
        ("mirror", u8::from(mirrored).to_string()),
        ("totalLayer", program.layers.len().to_string()),
        ("bottomLayerLiftHeight", r2(bottom.lift_mm)),
        ("bottomLayerLiftSpeed", r2(bottom.lift_mm_min)),
        ("bottomLightOffTime", r2(bottom.wait_before_cure_sec)),
        ("lightOffTime", r2(normal.wait_before_cure_sec)),
        ("bottomPWMLight", bottom.pwm.to_string()),
        ("PWMLight", normal.pwm.to_string()),
        ("antiAliasLevel", raster.aa().factor().to_string()),
    ];
    let mut text = String::new();
    for (key, value) in lines {
        let value = value.replace(['\r', '\n'], " ");
        text.push_str(&format!(";{key}:{value}\n"));
    }
    text
}

/// The whole `run.gcode` for `program`.
pub(crate) fn gcode(program: &PrintProgram) -> String {
    let layers = layers_of(program);
    let job = Job::new(program, &layers);
    let mut builder = Builder::new(dialect());
    builder.rebuild(&job, &header(program), &mut NoHooks);
    builder.into_text()
}

fn layer_png(rows: &[RleRun], width: u32, height: u32) -> Result<Vec<u8>, png::EncodingError> {
    let mut pixels = Vec::with_capacity(width as usize * height as usize);
    for run in rows {
        pixels.extend(std::iter::repeat_n(run.value, run.length as usize));
    }
    grey_png(&pixels, width, height)
}

/// The previews, black when there are none.
pub(crate) fn previews_or_black(
    previews: Option<&PrintPreviews>,
    sizes: PreviewSizes,
) -> Result<(PreviewImage, PreviewImage), WriteFailure> {
    Ok(match previews {
        Some(previews) => {
            previews.small.check_size(sizes.small)?;
            previews.large.check_size(sizes.large)?;
            (previews.small.clone(), previews.large.clone())
        }
        None => (
            PreviewImage::black(sizes.small),
            PreviewImage::black(sizes.large),
        ),
    })
}

/// Writes `stack` as a ChiTuBox ZIP running the settings in `program`.
///
/// Every layer image is the plate's image that [`write_layer_images`]
/// describes, as in the other formats. Without `previews` the machine shows
/// black previews.
///
/// # Errors
///
/// [`WriteFailure::PreviewSize`] when a rendering is not the size this format
/// fixes, raised before any byte is written, and [`WriteFailure::Io`] when the
/// archive cannot be written.
pub fn write_file(
    program: &PrintProgram,
    previews: Option<&PrintPreviews>,
    stack: &ContourStack,
    out: &mut dyn PrinterFileSink,
) -> Result<(), WriteFailure> {
    let (small, large) = previews_or_black(previews, PREVIEW_SIZES)?;
    let panel = program.raster.panel_resolution();
    let (width, height) = (panel.x(), panel.y());

    let text = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let image = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut zip = ZipWriter::new(out);
    entry(&mut zip, GCODE_FILE, gcode(program).as_bytes(), text)?;
    for (name, preview) in [(PREVIEW_FILE, &large), (PREVIEW_CROPPING_FILE, &small)] {
        let png = png_bytes(preview).map_err(|err| WriteFailure::Io(io::Error::other(err)))?;
        entry(&mut zip, name, &png, image)?;
    }
    write_layer_images(
        stack,
        &program.raster,
        1,
        |_, rows| layer_png(rows, width, height),
        |index, png: Result<Vec<u8>, png::EncodingError>| -> Result<(), WriteFailure> {
            let png = png.map_err(|err| WriteFailure::Io(io::Error::other(err)))?;
            entry(&mut zip, &format!("{}.png", index + 1), &png, image)
        },
    )?;
    zip.finish().map_err(zip_failure)?;
    Ok(())
}

#[cfg(test)]
mod tests_chitubox_zip;
