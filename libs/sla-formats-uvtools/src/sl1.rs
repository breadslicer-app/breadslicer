/*
 * SPDX-FileCopyrightText: 2020-2026 PTRTECH, Tiago Conceição (sn4k3) and UVtools contributors
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 *
 * Ported to Rust from UVtools UVtools.Core/FileFormats/SL1File.cs (with the
 * archive helpers of UVtools.Core/FileFormats/FileFormat.cs), UVtools v7.1.1,
 * commit 8b5c91fd41b60bd6265ba1c1f2b2b64636b873e8, modified for Breadslicer:
 * the layers are encoded from the slicer's run-length rows on every core, the
 * slow and fast tilt counts are measured on those rows, the INI files carry
 * the settings a print program states rather than every setting PrusaSlicer
 * knows, and the print settings come from a Breadslicer print program.
 *
 * This program is free software: you can redistribute it and/or modify it
 * under the terms of the GNU Affero General Public License as published by the
 * Free Software Foundation, either version 3 of the License, or (at your
 * option) any later version. It is distributed WITHOUT ANY WARRANTY; see the
 * GNU Affero General Public License (LICENSE.md) for details.
 */

//! Prusa's SL1 archive: `.sl1` for the Original Prusa SL1 and `.sl1s` for the
//! SL1S SPEED (#174).
//!
//! The archive holds:
//!
//! - `thumbnail/thumbnail400x400.png` and `thumbnail/thumbnail800x480.png`.
//! - `breadslicer00000.png`, `breadslicer00001.png`, ...: each layer as an
//!   8-bit grey PNG, named after the job.
//! - `config.ini`: what the machine reads, as `key = value` lines: the job
//!   name, the first and the normal exposure, the layer height, how many
//!   layers take the first exposure (`numFade`), how many tilt slowly and how
//!   many fast, the print time and the resin used.
//! - `prusaslicer.ini`: the printer, resin and print settings the file was
//!   made with, as PrusaSlicer writes them.
//!
//! Readers find each entry by its name, so the layers go first and the
//! settings, which count them, last.
//!
//! The machine tilts the vat on every layer and lifts the platform by one
//! layer height, so the archive carries one layer height for the whole
//! print and no travel.

use std::io::{self, Write};

use sc_sl1cer::contour::ContourStack;
use sc_sl1cer::raster::rle::RleRun;
use sc_sla_formats::layer_images::write_layer_images;
use sc_sla_formats::{
    PreviewImage, PreviewSize, PreviewSizes, PrintPreviews, PrintProgram, PrinterFileSink,
    Sl1Variant, WriteFailure,
};
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::program::{ProgramSummary, round_away, round_height};
use crate::resample::png_bytes;

/// The 2 thumbnails PrusaSlicer writes for these machines.
pub const PREVIEW_SIZES: PreviewSizes = PreviewSizes {
    small: PreviewSize {
        width: 400,
        height: 400,
    },
    large: PreviewSize {
        width: 800,
        height: 480,
    },
};

pub(crate) const CONFIG_FILE: &str = "config.ini";
pub(crate) const PRUSASLICER_FILE: &str = "prusaslicer.ini";

/// The job name the layers are named after. The machine finds the layers by
/// the `jobDir` the config states, whatever the archive is called.
pub(crate) const JOB_NAME: &str = "breadslicer";

/// The PrusaSlicer build UVtools states in the files it writes, which the
/// machines accept.
const PRUSASLICER_VERSION: &str = "PrusaSlicer-2.5.0+win64-202209060714";

/// Where a layer image lives in the archive.
pub(crate) fn layer_entry(index: usize) -> String {
    format!("{JOB_NAME}{index:05}.png")
}

/// Where a thumbnail lives in the archive.
pub(crate) fn thumbnail_entry(size: PreviewSize) -> String {
    format!("thumbnail/thumbnail{}x{}.png", size.width, size.height)
}

/// What each machine's own printer profile states, as UVtools' PrusaSlicer
/// printer profiles for them carry it.
struct Machine {
    model: &'static str,
    /// Above this share of the panel lit, in percent, a layer tilts slowly.
    area_fill_pct: f64,
    fast_tilt_sec: f32,
    slow_tilt_sec: f32,
}

fn machine(variant: Sl1Variant) -> Machine {
    match variant {
        Sl1Variant::Sl1 => Machine {
            model: "SL1",
            area_fill_pct: 50.0,
            fast_tilt_sec: 5.0,
            slow_tilt_sec: 8.0,
        },
        Sl1Variant::Sl1s => Machine {
            model: "SL1S",
            area_fill_pct: 45.0,
            fast_tilt_sec: 2.5,
            slow_tilt_sec: 5.0,
        },
    }
}

/// A number as .NET's invariant culture writes a float: no trailing zeros, no
/// fraction for a whole value.
fn num(value: f32) -> String {
    let text = format!("{value}");
    text.strip_suffix(".0").map(str::to_owned).unwrap_or(text)
}

/// The civil date and time, in UTC, of a moment `unix_sec` after the epoch.
fn utc(unix_sec: u64) -> (i64, u32, u32, u64, u64, u64) {
    let days = (unix_sec / 86_400) as i64;
    let rest = unix_sec % 86_400;
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day, rest / 3600, rest / 60 % 60, rest % 60)
}

/// The creation time as PrusaSlicer writes it: `2024-02-08 at 19:52:50 UTC`.
fn creation_timestamp(unix_sec: u64) -> String {
    let (year, month, day, hour, minute, second) = utc(unix_sec);
    format!("{year}-{month:02}-{day:02} at {hour:02}:{minute:02}:{second:02} UTC")
}

/// How many layers tilt slowly and how many fast.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TiltCounts {
    pub slow: u32,
    pub fast: u32,
}

fn config_ini(variant: Sl1Variant, program: &PrintProgram, tilts: TiltCounts) -> String {
    let machine = machine(variant);
    let summary = ProgramSummary::of(program);
    let (first_exposure, normal_exposure) =
        (summary.bottom.exposure_sec, summary.normal.exposure_sec);
    let lines = [
        ("action", "print".to_owned()),
        ("jobDir", JOB_NAME.to_owned()),
        ("expTime", num(round_away(normal_exposure, 2))),
        ("expTimeFirst", num(round_away(first_exposure, 2))),
        ("expUserProfile", "0".to_owned()),
        (
            "fileCreationTimestamp",
            creation_timestamp(program.created_at.unix_sec),
        ),
        ("hollow", "0".to_owned()),
        (
            "layerHeight",
            num(round_height(program.layer_height_mm.value() as f32)),
        ),
        ("materialName", program.material_name.as_str().to_owned()),
        ("numFade", program.bottom_layer_count.to_string()),
        ("numFast", tilts.fast.to_string()),
        ("numSlow", tilts.slow.to_string()),
        ("printProfile", program.slicer_version.to_owned()),
        (
            "printTime",
            num(round_away(program.print_time_sec.value() as f32, 6)),
        ),
        ("printerModel", machine.model.to_owned()),
        ("printerProfile", program.machine_name.as_str().to_owned()),
        ("printerVariant", "default".to_owned()),
        ("prusaSlicerVersion", PRUSASLICER_VERSION.to_owned()),
        (
            "usedMaterial",
            num(round_away((program.volume_mm3.value() / 1000.0) as f32, 6)),
        ),
    ];
    ini(&lines)
}

fn prusaslicer_ini(variant: Sl1Variant, program: &PrintProgram) -> String {
    let machine = machine(variant);
    let raster = &program.raster;
    let panel = raster.panel_resolution();
    let (width_mm, height_mm) = raster.panel_size_mm();
    let mirror = raster.mirror();
    let summary = ProgramSummary::of(program);
    let (first_exposure, normal_exposure) =
        (summary.bottom.exposure_sec, summary.normal.exposure_sec);
    let layer_height = num(round_height(program.layer_height_mm.value() as f32));
    let flag = |on: bool| if on { "1" } else { "0" }.to_owned();
    let lines = [
        ("area_fill", num(machine.area_fill_pct as f32)),
        ("display_height", num(round_away(height_mm as f32, 3))),
        ("display_mirror_x", flag(mirror.x)),
        ("display_mirror_y", flag(mirror.y)),
        ("display_orientation", "landscape".to_owned()),
        ("display_pixels_x", panel.x().to_string()),
        ("display_pixels_y", panel.y().to_string()),
        ("display_width", num(round_away(width_mm as f32, 3))),
        ("exposure_time", num(round_away(normal_exposure, 2))),
        ("faded_layers", program.bottom_layer_count.to_string()),
        ("fast_tilt_time", num(machine.fast_tilt_sec)),
        ("gamma_correction", "1".to_owned()),
        ("hollowing_enable", "0".to_owned()),
        ("initial_exposure_time", num(round_away(first_exposure, 2))),
        ("initial_layer_height", layer_height.clone()),
        ("layer_height", layer_height),
        ("material_print_speed", "fast".to_owned()),
        (
            "max_print_height",
            num(round_away(raster.build_volume().z().value() as f32, 2)),
        ),
        ("pad_enable", "0".to_owned()),
        ("printer_model", machine.model.to_owned()),
        (
            "printer_settings_id",
            program.machine_name.as_str().to_owned(),
        ),
        ("printer_technology", "SLA".to_owned()),
        ("printer_variant", "default".to_owned()),
        ("printer_vendor", String::new()),
        ("sla_archive_format", "SL1".to_owned()),
        (
            "sla_material_settings_id",
            program.material_name.as_str().to_owned(),
        ),
        ("sla_print_settings_id", program.slicer_version.to_owned()),
        ("slow_tilt_time", num(machine.slow_tilt_sec)),
        ("supports_enable", "0".to_owned()),
        ("thumbnails", "400x400/PNG, 800x480/PNG".to_owned()),
    ];
    ini(&lines)
}

fn ini(lines: &[(&str, String)]) -> String {
    let mut text = String::new();
    for (key, value) in lines {
        // One line per key: a value never breaks a line.
        let value = value.replace(['\r', '\n'], " ");
        text.push_str(&format!("{key} = {value}\n"));
    }
    text
}

/// One layer's image, as a PNG, and whether it tilts slowly.
struct Encoded {
    png: Result<Vec<u8>, png::EncodingError>,
    slow: bool,
}

/// Encodes a frame's rows as an 8-bit grey PNG, `width` pixels per row, and
/// reports whether more than `area_fill_pct` of it is lit, every pixel counted
/// by its grey.
fn encode(rows: &[RleRun], width: u32, height: u32, area_fill_pct: f64) -> Encoded {
    let mut pixels = Vec::with_capacity(width as usize * height as usize);
    let mut lit = 0_u64;
    for run in rows {
        pixels.extend(std::iter::repeat_n(run.value, run.length as usize));
        lit += u64::from(run.value) * u64::from(run.length);
    }
    let total = u64::from(width) * u64::from(height) * 255;
    let slow = total > 0 && (lit as f64) * 100.0 > area_fill_pct * total as f64;
    Encoded {
        png: grey_png(&pixels, width, height),
        slow,
    }
}

pub(crate) fn grey_png(
    pixels: &[u8],
    width: u32,
    height: u32,
) -> Result<Vec<u8>, png::EncodingError> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Balanced);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(pixels)?;
        writer.finish()?;
    }
    Ok(bytes)
}

pub(crate) fn entry<W: Write + io::Seek>(
    zip: &mut ZipWriter<W>,
    name: &str,
    bytes: &[u8],
    options: SimpleFileOptions,
) -> Result<(), WriteFailure> {
    zip.start_file(name, options).map_err(zip_failure)?;
    zip.write_all(bytes)?;
    Ok(())
}

pub(crate) fn zip_failure(err: zip::result::ZipError) -> WriteFailure {
    match err {
        zip::result::ZipError::Io(err) => WriteFailure::Io(err),
        other => WriteFailure::Io(io::Error::other(other)),
    }
}

/// Writes `stack` as an SL1 archive of `variant` running the settings in
/// `program`.
///
/// Every layer image is the plate's image that [`write_layer_images`]
/// describes, as in the other formats. The archive states one layer height,
/// the program's: a program whose layers are not that far apart is not one
/// this format can carry, and the caller slices such a print at a fixed
/// height for it. Without `previews` the machine shows black thumbnails.
///
/// # Errors
///
/// [`WriteFailure::PreviewSize`] when a rendering is not the size this format
/// fixes, raised before any byte is written, and [`WriteFailure::Io`] when the
/// archive cannot be written.
pub fn write_file(
    variant: Sl1Variant,
    program: &PrintProgram,
    previews: Option<&PrintPreviews>,
    stack: &ContourStack,
    out: &mut dyn PrinterFileSink,
) -> Result<(), WriteFailure> {
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
    let panel = program.raster.panel_resolution();
    let (width, height) = (panel.x(), panel.y());
    let area_fill_pct = machine(variant).area_fill_pct;

    let text = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    // A PNG is compressed already.
    let image = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut zip = ZipWriter::new(out);
    for preview in [&small, &large] {
        let png = png_bytes(preview).map_err(|err| WriteFailure::Io(io::Error::other(err)))?;
        let size = PreviewSize {
            width: preview.width(),
            height: preview.height(),
        };
        entry(&mut zip, &thumbnail_entry(size), &png, image)?;
    }
    // The layers go before the config, which counts the slow and the fast
    // ones: no layer waits in memory for it.
    let mut tilts = TiltCounts::default();
    write_layer_images(
        stack,
        &program.raster,
        1,
        |_, rows| encode(rows, width, height, area_fill_pct),
        |index, encoded: Encoded| -> Result<(), WriteFailure> {
            let png = encoded
                .png
                .map_err(|err| WriteFailure::Io(io::Error::other(err)))?;
            if encoded.slow {
                tilts.slow += 1;
            } else {
                tilts.fast += 1;
            }
            entry(&mut zip, &layer_entry(index), &png, image)
        },
    )?;
    entry(
        &mut zip,
        CONFIG_FILE,
        config_ini(variant, program, tilts).as_bytes(),
        text,
    )?;
    entry(
        &mut zip,
        PRUSASLICER_FILE,
        prusaslicer_ini(variant, program).as_bytes(),
        text,
    )?;
    zip.finish().map_err(zip_failure)?;
    Ok(())
}

#[cfg(test)]
mod tests_sl1;
