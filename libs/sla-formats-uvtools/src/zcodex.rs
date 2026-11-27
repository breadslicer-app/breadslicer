/*
 * SPDX-FileCopyrightText: 2020-2026 PTRTECH, Tiago Conceição (sn4k3) and UVtools contributors
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 *
 * Ported to Rust from UVtools UVtools.Core/FileFormats/ZCodexFile.cs, UVtools
 * v7.1.1, commit 8b5c91fd41b60bd6265ba1c1f2b2b64636b873e8, modified for
 * Breadslicer: the layers are encoded from the slicer's run-length rows and
 * the settings come from a Breadslicer print program.
 *
 * This program is free software: you can redistribute it and/or modify it
 * under the terms of the GNU Affero General Public License as published by the
 * Free Software Foundation, either version 3 of the License, or (at your
 * option) any later version. It is distributed WITHOUT ANY WARRANTY; see the
 * GNU Affero General Public License (LICENSE.md) for details.
 */

//! Z-Suite's ZCodex: `.zcodex` for the Zortrax Inkspire (#778).
//!
//! A ZIP of 3 JSON files (`ResinMetadata`, `UserSettingsData`,
//! `ZCodeMetadata`), `Preview.png`, the layers as
//! `ResinSlicesData/Slice00000.png`, ... and `ResinGCodeData`: per layer
//! `<Slice> n`, the moves, and the light between `<Delay_blank>` and
//! `<Delay_model>`, which the machine times from the settings.

use std::io;

use sc_sl1cer::contour::ContourStack;
use sc_sl1cer::raster::rle::RleRun;
use sc_sla_formats::layer_images::write_layer_images;
use sc_sla_formats::{
    PreviewSize, PreviewSizes, PrintPreviews, PrintProgram, PrinterFileSink, WriteFailure,
};
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::chitubox_zip::previews_or_black;
use crate::gcode::{layers_of, num};
use crate::program::{ProgramSummary, round_away, round_height};
use crate::resample::png_bytes;
use crate::sl1::{entry, grey_png, zip_failure};

/// The one preview, 320 x 180; the small size is unused.
pub const PREVIEW_SIZES: PreviewSizes = PreviewSizes {
    small: PreviewSize {
        width: 320,
        height: 180,
    },
    large: PreviewSize {
        width: 320,
        height: 180,
    },
};

pub(crate) const GCODE_FILE: &str = "ResinGCodeData";

pub(crate) fn layer_entry(index: usize) -> String {
    format!("ResinSlicesData/Slice{index:05}.png")
}

fn ms(seconds: f32) -> u32 {
    (seconds * 1000.0) as u32
}

fn json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A JSON object, 2 spaces a level, as System.Text.Json indents.
fn object(fields: &[(&str, String)], indent: usize) -> String {
    let pad = " ".repeat(indent + 2);
    let body: Vec<String> = fields
        .iter()
        .map(|(key, value)| format!("{pad}\"{key}\": {value}"))
        .collect();
    format!("{{\n{}\n{}}}", body.join(",\n"), " ".repeat(indent))
}

fn resin_metadata(program: &PrintProgram) -> String {
    let summary = ProgramSummary::of(program);
    let (b, n) = (summary.bottom, summary.normal);
    let count = program.layers.len();
    let ml = round_away((program.volume_mm3.value() / 1000.0) as f32, 3);
    let per_layer = if count == 0 { 0.0 } else { ml / count as f32 };
    let layers: Vec<String> = (0..count)
        .map(|index| {
            object(
                &[
                    ("Layer", index.to_string()),
                    ("UsedMaterialVolume", num(per_layer)),
                ],
                4,
            )
        })
        .collect();
    let layers = if layers.is_empty() {
        "[]".to_owned()
    } else {
        format!("[\n    {}\n  ]", layers.join(",\n    "))
    };
    object(
        &[
            ("Guid", json_string("07452AC2-7494-4576-BA60-BFEA8815F917")),
            ("Material", json_string(program.material_name.as_str())),
            ("MaterialId", "null".to_owned()),
            (
                "LayerThickness",
                num(round_height(program.layer_height_mm.value() as f32)),
            ),
            (
                "PrintTime",
                (program.print_time_sec.value() as u32).to_string(),
            ),
            ("LayerTime", ms(n.exposure_sec).to_string()),
            ("BottomLayersTime", ms(b.exposure_sec).to_string()),
            ("AdditionalSupportLayerTime", "0".to_owned()),
            ("BottomLayersNumber", program.bottom_layer_count.to_string()),
            ("BlankingLayerTime", ms(n.wait_before_cure_sec).to_string()),
            ("TotalMaterialVolumeUsed", num(ml)),
            (
                "TotalMaterialWeightUsed",
                num(round_away(program.mass_g.value() as f32, 3)),
            ),
            ("TotalLayersCount", count.to_string()),
            ("DisableSettingsChanges", "false".to_owned()),
            ("Layers", layers),
        ],
        0,
    )
}

fn user_settings(program: &PrintProgram) -> String {
    let summary = ProgramSummary::of(program);
    let (b, n) = (summary.bottom, summary.normal);
    let seconds = program.print_time_sec.value() as u64;
    let thickness = num(round_height(program.layer_height_mm.value() as f32));
    object(
        &[
            (
                "MaxLayer",
                program.layers.len().saturating_sub(1).to_string(),
            ),
            (
                "PrintTime",
                json_string(&format!("{}h {}m", seconds / 3600 % 24, seconds / 60 % 60)),
            ),
            (
                "MaterialVolume",
                num(round_away((program.volume_mm3.value() / 1000.0) as f32, 3)),
            ),
            ("IsAdvanced", "0".to_owned()),
            ("Printer", json_string("Zortrax Inkspire")),
            ("MaterialType", "null".to_owned()),
            ("MaterialId", "0".to_owned()),
            ("LayerThickness", json_string(&format!("{thickness} mm"))),
            ("RaftEnabled", "0".to_owned()),
            ("RaftHeight", "0".to_owned()),
            ("RaftOffset", "0".to_owned()),
            ("ModelLiftEnabled", "0".to_owned()),
            ("ModelLiftHeight", "0".to_owned()),
            ("CrossSupportEnabled", "0".to_owned()),
            ("LayerExposureTime", ms(n.exposure_sec).to_string()),
            ("ExposureOffTime", ms(n.wait_before_cure_sec).to_string()),
            ("BottomLayerExposureTime", ms(b.exposure_sec).to_string()),
            ("BottomLayersCount", program.bottom_layer_count.to_string()),
            ("SupportAdditionalExposureEnabled", "0".to_owned()),
            ("SupportAdditionalExposureTime", "0".to_owned()),
            ("ZLiftDistance", num(round_away(n.lift_mm, 2))),
            ("ZLiftRetractRate", num(round_away(n.retract_mm_min, 2))),
            ("ZLiftFeedRate", num(round_away(n.lift_mm_min, 2))),
            ("AntiAliasing", program.raster.aa().factor().to_string()),
            ("XCorrection", "0".to_owned()),
            ("YCorrection", "0".to_owned()),
            ("HollowEnabled", "0".to_owned()),
            ("HollowThickness", "0".to_owned()),
            ("InfillDensity", "0".to_owned()),
        ],
        0,
    )
}

fn zcode_metadata(program: &PrintProgram) -> String {
    let material = object(
        &[
            ("ExtruderType", json_string("MAIN")),
            ("Id", "0".to_owned()),
            ("Name", json_string("")),
            ("Usage", "0".to_owned()),
            ("Temperature", "0".to_owned()),
        ],
        4,
    );
    object(
        &[
            ("ZCodexVersion", json_string("2.0.0.0")),
            ("SoftwareVersion", json_string("2.12.2.0")),
            ("MinFirmwareVersion", json_string("20013")),
            ("PrinterModelEnumId", "40".to_owned()),
            ("PrinterName", json_string(program.machine_name.as_str())),
            ("Materials", format!("[\n    {material}\n  ]")),
            ("HeatbedTemperature", "0".to_owned()),
            ("ChamberTemperature", "0".to_owned()),
            ("CommandCount", "0".to_owned()),
            (
                "PrintTime",
                (program.print_time_sec.value() as u32).to_string(),
            ),
            ("NozzleDiameter", "0".to_owned()),
            ("PrintBoundingBox", "null".to_owned()),
            ("Pauses", "null".to_owned()),
            ("MaterialUsages", "null".to_owned()),
        ],
        0,
    )
}

/// The run: per layer its slice, its moves and the light between the
/// delays the machine fills in.
pub(crate) fn gcode(program: &PrintProgram) -> String {
    let summary = ProgramSummary::of(program);
    let mut text = String::new();
    let mut last_z = 0.0_f32;
    for (index, layer) in layers_of(program).iter().enumerate() {
        text.push_str(&format!("<Slice> {index}\n"));
        if last_z != layer.position_z {
            if layer.lift_mm > 0.0 {
                text.push_str(&format!(
                    "G1 Z{} F{}\n",
                    num(layer.lift_mm),
                    num(layer.lift_mm_min)
                ));
                text.push_str(&format!(
                    "G1 Z-{} F{}\n",
                    num(round_height(layer.lift_mm - layer.position_z + last_z)),
                    num(layer.retract_mm_min)
                ));
            } else {
                text.push_str(&format!(
                    "G1 Z{} F{}\n",
                    num(round_height(layer.position_z - last_z)),
                    num(layer.lift_mm_min)
                ));
            }
        }
        text.push_str("<Delay_blank>\nM106 S255\n<Delay_model>\nM106 S0\n");
        last_z = layer.position_z;
    }
    text.push_str(&format!(
        "G1 Z40.0 F{}\nM18\n",
        num(round_away(summary.normal.lift_mm_min, 2))
    ));
    text
}

fn layer_png(rows: &[RleRun], width: u32, height: u32) -> Result<Vec<u8>, png::EncodingError> {
    let mut pixels = Vec::with_capacity(width as usize * height as usize);
    for run in rows {
        pixels.extend(std::iter::repeat_n(run.value, run.length as usize));
    }
    grey_png(&pixels, width, height)
}

/// Writes `stack` as a ZCodex running the settings in `program`.
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
    let (preview, _) = previews_or_black(previews, PREVIEW_SIZES)?;
    let panel = program.raster.panel_resolution();
    let (width, height) = (panel.x(), panel.y());
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut zip = ZipWriter::new(out);
    entry(
        &mut zip,
        "ResinMetadata",
        resin_metadata(program).as_bytes(),
        options,
    )?;
    entry(
        &mut zip,
        "UserSettingsData",
        user_settings(program).as_bytes(),
        options,
    )?;
    entry(
        &mut zip,
        "ZCodeMetadata",
        zcode_metadata(program).as_bytes(),
        options,
    )?;
    let png = png_bytes(&preview).map_err(|err| WriteFailure::Io(io::Error::other(err)))?;
    entry(&mut zip, "Preview.png", &png, options)?;
    write_layer_images(
        stack,
        &program.raster,
        1,
        |_, rows| layer_png(rows, width, height),
        |index, png: Result<Vec<u8>, png::EncodingError>| -> Result<(), WriteFailure> {
            let png = png.map_err(|err| WriteFailure::Io(io::Error::other(err)))?;
            entry(&mut zip, &layer_entry(index), &png, options)
        },
    )?;
    entry(&mut zip, GCODE_FILE, gcode(program).as_bytes(), options)?;
    zip.finish().map_err(zip_failure)?;
    Ok(())
}

#[cfg(test)]
mod tests_zcodex;
