/*
 * SPDX-FileCopyrightText: 2020-2026 PTRTECH, Tiago Conceição (sn4k3) and UVtools contributors
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 *
 * Ported to Rust from UVtools UVtools.Core/FileFormats/NanoDLPFile.cs,
 * UVtools v7.1.1, commit 8b5c91fd41b60bd6265ba1c1f2b2b64636b873e8, modified
 * for Breadslicer: the layers are encoded from the slicer's run-length rows,
 * each layer's areas in info.json are measured on the slicer's own contours,
 * and the settings come from a Breadslicer print program.
 *
 * This program is free software: you can redistribute it and/or modify it
 * under the terms of the GNU Affero General Public License as published by the
 * Free Software Foundation, either version 3 of the License, or (at your
 * option) any later version. It is distributed WITHOUT ANY WARRANTY; see the
 * GNU Affero General Public License (LICENSE.md) for details.
 */

//! NanoDLP's plate export: `.nanodlp` for the Concepts3D Athena 8K and 12K
//! (#778).
//!
//! A ZIP of the plate as NanoDLP keeps it: `meta.json`, `slicer.json`,
//! `plate.json`, `profile.json` (the resin profile the machine runs, its
//! G-code templates included), `info.json` (each layer's areas and bounds),
//! the `3d.png` preview and the layers as `1.png`, `2.png`, ...

use std::io;

use sc_sl1cer::contour::{ContourLayer, ContourStack};
use sc_sl1cer::raster::rle::RleRun;
use sc_sla_formats::layer_images::write_layer_images;
use sc_sla_formats::{
    PreviewSize, PreviewSizes, PrintPreviews, PrintProgram, PrinterFileSink, WriteFailure,
};
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::chitubox_zip::previews_or_black;
use crate::gcode::num;
use crate::layer_stats::LayerStats;
use crate::program::{ProgramSummary, round_away, round_height};
use crate::resample::png_bytes;
use crate::sl1::{entry, zip_failure};

/// The one preview, 607 x 487; the small size is unused.
pub const PREVIEW_SIZES: PreviewSizes = PreviewSizes {
    small: PreviewSize {
        width: 607,
        height: 487,
    },
    large: PreviewSize {
        width: 607,
        height: 487,
    },
};

/// The G-code the Athena runs before each layer, as UVtools states it.
const SHIELD_BEFORE_LAYER: &str = r#"G90\r\nSET_PROGRESS_BAR CL=[[LayerNumber]] TL=[[TotalNumberOfLayers]]\r\nMOVE_PLATE Z=[[LayerPosition]] F=600\r\n[[MoveWait 2]]\r\n[[PositionSet [[LayerPosition]]]]\r\n[JS]\r\nif([[_FssEnableDynamicwait]] == 0)output=\"[[DynamicWaitStart]]\";\r\n[/JS]\r\nfss_idle"#;
/// The G-code the Athena runs after each layer, as UVtools states it.
const SHIELD_AFTER_LAYER: &str = r#"[JS]if ([[LayerNumber]]==1||[[LayerNumber]]%3==0) output=\"TRIGGER\";[/JS]\r\n[JS]if ([[LayerNumber]]==1||[[LayerNumber]]%5==0) output=\"M105\";[/JS]\r\n[[MoveCounterSet 0]]\r\nUVLED_ON PWM=[[_UvPwmValue]]\r\nDWELL P={[[CureTime]]*1000}\r\n[[MoveWait 1]]\r\nUVLED_OFF \r\n[[GPIOHigh 10]]\r\n\r\n[JS]\r\nvar nano = nanodlpContext();\r\nvar resinlevel = nano[\"Status\"][\"ResinLevelMm\"];\r\nif( [[_FssEnablePeeldetection]]==0 \u0026\u0026 [[CurrentPosition]] \u003e resinlevel) output=\"[[SeparationDetectionStart]]\";\r\n[/JS]\r\n\r\n[[MoveCounterSet 0]]\r\nG91\r\nMOVE_PLATE_FSS Z=[[ZLiftDistance]] F={120-([[LayerNumber]]\u003c5)*70}\r\n[[PositionChange [[ZLiftDistance]]]]\r\n[[MoveWait 1]]\r\n[[SeparationDetectionStop]]\r\n[JS]if ([[LayerNumber]]==1||[[LayerNumber]]%100==0) output=\"G4 P2000\";[/JS]\r\n[JS]if ([[LayerNumber]]==1||[[LayerNumber]]%100==0) output=\"[[PressureWrite 1]]\";[/JS]"#;

/// A JSON value, written as System.Text.Json writes an indented object.
enum Json {
    Raw(String),
    Text(String),
    Object(Vec<(&'static str, Json)>),
}

fn n(value: f32) -> Json {
    Json::Raw(num(value))
}

fn int(value: impl ToString) -> Json {
    Json::Raw(value.to_string())
}

fn text(value: &str) -> Json {
    Json::Text(value.to_owned())
}

fn raw(value: &str) -> Json {
    Json::Raw(value.to_owned())
}

fn escape(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn write_json(value: &Json, indent: usize, out: &mut String) {
    match value {
        Json::Raw(raw) => out.push_str(raw),
        Json::Text(text) => out.push_str(&escape(text)),
        Json::Object(fields) => {
            if fields.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push_str("{\n");
            for (index, (key, field)) in fields.iter().enumerate() {
                out.push_str(&" ".repeat(indent + 2));
                out.push_str(&format!("\"{key}\": "));
                write_json(field, indent + 2, out);
                if index + 1 < fields.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&" ".repeat(indent));
            out.push('}');
        }
    }
}

fn json(value: &Json) -> String {
    let mut out = String::new();
    write_json(value, 0, &mut out);
    out
}

/// The fields the slicer and the resin profile share, as NanoDLP names them.
fn shared_fields(aa: u8) -> Vec<(&'static str, Json)> {
    vec![
        ("AdaptSlicing", int(0)),
        ("AdaptSlicingMin", int(0)),
        ("AdaptSlicingMax", int(0)),
        ("SupportOffset", int(0)),
        ("Offset", int(0)),
        ("FillColor", text("#ffffff")),
        ("BlankColor", text("#000000")),
        ("DimAmount", int(0)),
        ("DimWall", int(0)),
        ("DimSkip", int(0)),
        ("PixelDiming", int(0)),
        ("HatchingType", int(0)),
        ("ElephantMidExposure", int(1)),
        ("ElephantType", int(0)),
        ("ElephantAmount", int(0)),
        ("ElephantWall", int(0)),
        ("ElephantLayers", int(0)),
        ("HatchingWall", int(20)),
        ("HatchingGap", int(20)),
        ("HatchingOuterWall", int(20)),
        ("HatchingTopCap", int(10)),
        ("HatchingBottomCap", int(10)),
        ("MultiCureGap", int(0)),
        ("AntiAlias", int(aa)),
        ("AntiAlias3D", int(0)),
        ("AntiAlias3DDistance", int(0)),
        ("AntiAlias3DMin", int(0)),
        ("AntiAliasThreshold", int(0)),
        ("ImageRotate", int(0)),
        ("IgnoreMask", int(1)),
        ("XYRes", int(0)),
    ]
}

fn color(r: u8, g: u8, b: u8, a: u8) -> Json {
    Json::Object(vec![
        ("R", int(r)),
        ("G", int(g)),
        ("B", int(b)),
        ("A", int(a)),
    ])
}

fn mc() -> Json {
    Json::Object(vec![
        ("StartX", int(0)),
        ("StartY", int(0)),
        ("Width", int(0)),
        ("Height", int(0)),
        ("X", raw("null")),
        ("Y", raw("null")),
        ("MultiCureGap", int(0)),
        ("Count", int(0)),
    ])
}

struct Machine {
    pixel_mm: (f32, f32),
    thickness_um: f32,
}

fn machine(program: &PrintProgram) -> Machine {
    let panel = program.raster.panel_resolution();
    let (width_mm, height_mm) = program.raster.panel_size_mm();
    Machine {
        pixel_mm: (
            round_away(width_mm as f32 / panel.x() as f32, 4),
            round_away(height_mm as f32 / panel.y() as f32, 4),
        ),
        thickness_um: round_height(round_height(program.layer_height_mm.value() as f32) * 1000.0),
    }
}

fn meta() -> String {
    json(&Json::Object(vec![
        ("FormatVersion", int(2)),
        ("Distro", text("generic")),
        ("Program", text("Breadslicer")),
        ("Version", text(env!("CARGO_PKG_VERSION"))),
        ("OS", text(std::env::consts::OS)),
        ("Arch", text(std::env::consts::ARCH)),
        ("Profile", raw("false")),
    ]))
}

/// Where the model lies, in millimetres from the panel's centre, and how high.
#[derive(Clone, Copy, Debug, Default)]
struct Boundary {
    x_min: f32,
    x_max: f32,
    y_min: f32,
    y_max: f32,
    z_max: f32,
}

fn boundary(program: &PrintProgram, bounds: &[Option<[u32; 4]>]) -> Boundary {
    let (width_mm, height_mm) = program.raster.panel_size_mm();
    let panel = program.raster.panel_resolution();
    let pixel = (
        width_mm as f32 / panel.x() as f32,
        height_mm as f32 / panel.y() as f32,
    );
    let mut rect: Option<[u32; 4]> = None;
    for b in bounds.iter().flatten() {
        rect = Some(match rect {
            None => *b,
            Some(r) => [
                r[0].min(b[0]),
                r[1].min(b[1]),
                r[2].max(b[2]),
                r[3].max(b[3]),
            ],
        });
    }
    let z_max = program
        .layers
        .last()
        .map_or(0.0, |l| round_height(l.slab_top_mm.value() as f32));
    let Some([x0, y0, x1, y1]) = rect else {
        return Boundary {
            z_max,
            ..Boundary::default()
        };
    };
    let r2 = |value: f32| round_away(value, 2);
    let (x, y) = (r2(x0 as f32 * pixel.0), r2(y0 as f32 * pixel.1));
    let (w, h) = (
        r2((x1 - x0) as f32 * pixel.0),
        r2((y1 - y0) as f32 * pixel.1),
    );
    let (half_w, half_h) = (width_mm as f32 / 2.0, height_mm as f32 / 2.0);
    let r4 = |value: f32| round_away(value, 4);
    Boundary {
        x_min: r4(x - half_w),
        x_max: r4(x + w - half_w),
        y_min: r4(y - half_h),
        y_max: r4(y + h - half_h),
        z_max,
    }
}

fn boundary_fields(b: &Boundary) -> Vec<(&'static str, Json)> {
    vec![
        ("XMin", n(b.x_min)),
        ("XMax", n(b.x_max)),
        ("YMin", n(b.y_min)),
        ("YMax", n(b.y_max)),
        ("ZMin", int(0)),
        ("ZMax", n(b.z_max)),
    ]
}

fn slicer(program: &PrintProgram, machine: &Machine, bounds: &Boundary) -> String {
    let panel = program.raster.panel_resolution();
    let aa = program.raster.aa().factor();
    let mirror = program.raster.mirror();
    let mut fields = vec![
        ("Type", text("stl")),
        ("URL", text("")),
        ("PWidth", int(panel.x())),
        ("PHeight", int(panel.y())),
        ("ScaleFactor", int(0)),
        ("StartLayer", int(0)),
        ("SupportDepth", n(machine.thickness_um)),
        ("SupportLayerNumber", int(program.bottom_layer_count)),
        ("Thickness", n(machine.thickness_um)),
        ("XOffset", n(panel.x() as f32 / 2.0)),
        ("YOffset", n(panel.y() as f32 / 2.0)),
        ("ZOffset", int(0)),
        ("XPixelSize", n(machine.pixel_mm.0)),
        ("YPixelSize", n(machine.pixel_mm.1)),
        ("Mask", raw("null")),
        ("SliceFromZero", raw("false")),
        ("DisableValidator", raw("false")),
        ("AutoCenter", int(0)),
        ("PreviewGenerate", raw("false")),
        ("Running", raw("false")),
        ("Debug", raw("false")),
        ("IsFaulty", raw("false")),
        ("Corrupted", raw("false")),
        ("MultiMaterial", raw("false")),
        ("AdaptExport", text("")),
        ("PreviewColor", text("")),
        ("FaultyLayers", raw("[]")),
        ("OverhangLayers", raw("[]")),
        ("LayerStatus", raw("[]")),
        ("Boundary", Json::Object(boundary_fields(bounds))),
        ("MC", mc()),
        ("MultiThickness", text("")),
        ("ExportPath", text("")),
        ("NetworkSave", text("")),
        ("File", text("/plate.stl")),
        ("FileSize", int(0)),
    ];
    fields.extend(shared_fields(aa));
    fields.extend([
        ("XRes", int(0)),
        ("YRes", int(0)),
        ("ZResPerc", int(0)),
        ("PreviewWidth", int(PREVIEW_SIZES.large.width)),
        ("PreviewHeight", int(PREVIEW_SIZES.large.height)),
        ("BarrelFactor", int(0)),
        ("BarrelX", int(0)),
        ("BarrelY", int(0)),
        ("ImageMirror", int(u8::from(mirror.x || mirror.y))),
        ("DisplayController", int(1)),
        ("LightOutputFormula", text("")),
        ("PlateID", int(0)),
        ("LayerID", int(0)),
        ("LayerCount", int(program.layers.len())),
        ("UUID", text("")),
        ("DynamicThickness", raw("[]")),
        ("FillColorRGB", color(255, 255, 255, 255)),
        ("BlankColorRGB", color(0, 0, 0, 255)),
        ("ExportType", int(1)),
        ("OutputPath", text("")),
        ("Suffix", text("")),
        ("SkipEmpty", int(0)),
    ]);
    json(&Json::Object(fields))
}

fn plate(program: &PrintProgram, bounds: &Boundary) -> String {
    let seconds = program.print_time_sec.value() as u32;
    let mut fields = vec![
        ("PlateID", int(0)),
        ("ProfileID", int(0)),
        ("Profile", raw("null")),
        ("CreatedDate", int(0)),
        ("StopLayers", text("")),
        ("Path", text("breadslicer")),
        ("LowQualityLayerNumber", int(0)),
        ("AutoCenter", int(0)),
        ("Updated", int(program.created_at.unix_sec)),
        ("LastPrint", int(0)),
        ("PrintTime", int(seconds)),
        ("PrintEst", int(seconds)),
        ("ImageRotate", int(0)),
        ("MaskEffect", int(0)),
        ("XRes", int(0)),
        ("YRes", int(0)),
        ("ZRes", int(0)),
        ("MultiCure", text("")),
        ("MultiThickness", text("")),
        ("CureTimes", raw("[]")),
        ("DynamicThickness", raw("[]")),
        ("Offset", int(0)),
        ("OverHangs", raw("[]")),
        ("Risky", raw("false")),
        ("IsFaulty", raw("false")),
        ("Repaired", raw("false")),
        ("FaultyLayers", raw("[]")),
        ("Corrupted", raw("false")),
        (
            "TotalSolidArea",
            n(round_away((program.volume_mm3.value() / 1000.0) as f32, 3)),
        ),
        ("BlackoutData", raw("null")),
        ("LayersCount", int(program.layers.len())),
        ("Feedback", raw("false")),
        ("ReSliceNeeded", raw("false")),
        ("MultiMaterial", raw("false")),
        ("PrintID", int(0)),
        ("MC", mc()),
    ];
    fields.extend(boundary_fields(bounds));
    json(&Json::Object(fields))
}

fn profile(program: &PrintProgram, machine: &Machine) -> String {
    let summary = ProgramSummary::of(program);
    let (b, nl) = (summary.bottom, summary.normal);
    let r2 = |value: f32| n(round_away(value, 2));
    let pwm = format!(
        "{}",
        (f64::from(nl.pwm) / 255.0 * 100.0).round_ties_even() / 100.0
    );
    let custom = Json::Object(vec![
        ("CdThreshold", text("2000")),
        ("DwFlowEndSlope", text("10")),
        ("FssEnableCrashdetection", text("0")),
        ("FssEnableDynamicwait", text("0")),
        ("FssEnablePeeldetection", text("0")),
        ("FssEnableResinleveldetection", text("0")),
        ("PdPeelEndSlope", text("0")),
        ("PdPeelStartSlope", text("0")),
        ("ResinPreheatTemperature", text("25")),
        ("RlThreshold", text("-5")),
        ("UvPwmValue", text(&pwm)),
    ]);
    let mut fields = vec![
        ("ResinID", int(0)),
        ("ProfileID", int(0)),
        ("Title", text(program.machine_name.as_str())),
        ("Desc", text("")),
        ("Color", text("")),
        ("ResinPrice", int(0)),
        ("OptimumTemperature", int(0)),
        ("Depth", n(machine.thickness_um)),
        ("SupportTopWait", r2(b.wait_after_lift_sec)),
        ("SupportWaitHeight", r2(b.total_lift_mm())),
        ("SupportDepth", n(machine.thickness_um)),
        ("SupportWaitBeforePrint", r2(b.wait_before_cure_sec)),
        ("SupportWaitAfterPrint", r2(b.wait_after_cure_sec)),
        ("TransitionalLayer", int(program.transition_layer_count)),
        ("Updated", int(program.created_at.unix_sec)),
        ("CustomValues", custom),
        ("Type", int(0)),
        ("ZStepWait", int(5000)),
        ("TopWait", r2(nl.wait_after_lift_sec)),
        ("WaitHeight", r2(nl.total_lift_mm())),
        ("CureTime", r2(nl.exposure_sec)),
        ("WaitBeforePrint", r2(nl.wait_before_cure_sec)),
        ("WaitAfterPrint", r2(nl.wait_after_cure_sec)),
        ("SupportCureTime", r2(b.exposure_sec)),
        ("SupportLayerNumber", int(program.bottom_layer_count)),
    ];
    fields.extend(shared_fields(program.raster.aa().factor()));
    fields.extend([
        ("YRes", int(0)),
        ("ZResPerc", int(0)),
        ("DynamicCureTime", text("")),
        ("DynamicSpeed", text("")),
        ("ShieldBeforeLayer", text(SHIELD_BEFORE_LAYER)),
        ("ShieldAfterLayer", text(SHIELD_AFTER_LAYER)),
        ("ShieldDuringCure", text("")),
        ("ShieldStart", text("")),
        ("ShieldResume", text("")),
        ("ShieldFinish", text("")),
        ("LaserCode", text("")),
        ("ShutterOpenGcode", text("")),
        ("ShutterCloseGcode", text("")),
        ("SeparationDetection", text("")),
        ("ResinLevelDetection", text("")),
        ("CrashDetection", text("")),
        ("DynamicWait", text("")),
        ("SlowSectionHeight", int(7)),
        ("SlowSectionStepWait", int(40)),
        ("JumpPerLayer", int(0)),
        ("DynamicWaitAfterLift", text("")),
        ("DynamicLift", text("")),
        ("JumpHeight", int(0)),
        ("LowQualityCureTime", int(0)),
        ("LowQualitySkipPerLayer", int(0)),
        ("XYResPerc", int(0)),
    ]);
    json(&Json::Object(fields))
}

/// A layer's areas from its contours, in square millimetres: the whole solid,
/// its smallest and largest parts, and how many parts.
fn areas(layer: &ContourLayer) -> (f32, f32, f32, usize) {
    let parts: Vec<f64> = layer
        .polygons()
        .iter()
        .map(|polygon| {
            polygon.outer().signed_area().abs()
                - polygon
                    .holes()
                    .iter()
                    .map(|hole| hole.signed_area().abs())
                    .sum::<f64>()
        })
        .collect();
    let total: f64 = parts.iter().sum();
    let smallest = parts.iter().copied().fold(f64::INFINITY, f64::min);
    let largest = parts.iter().copied().fold(0.0_f64, f64::max);
    let r4 = |value: f64| round_away(value as f32, 4);
    (
        r4(total),
        if parts.is_empty() { 0.0 } else { r4(smallest) },
        r4(largest),
        parts.len(),
    )
}

/// `info.json`: each layer's areas and lit bounds, in pixels.
fn info(stack: &ContourStack, bounds: &[Option<[u32; 4]>]) -> String {
    let mut out = String::from("[");
    for (index, layer) in stack.layers().iter().take(bounds.len()).enumerate() {
        let (total, smallest, largest, count) = areas(layer);
        let rect = bounds[index].unwrap_or([0; 4]);
        let item = Json::Object(vec![
            ("TotalSolidArea", n(total)),
            ("LargestArea", n(largest)),
            ("SmallestArea", n(smallest)),
            ("MinX", int(rect[0])),
            ("MinY", int(rect[1])),
            ("MaxX", int(rect[2])),
            ("MaxY", int(rect[3])),
            ("AreaCount", int(count)),
        ]);
        out.push_str(if index == 0 { "\n  " } else { ",\n  " });
        write_json(&item, 2, &mut out);
    }
    out.push_str(if bounds.is_empty() { "]" } else { "\n]" });
    out
}

struct Encoded {
    png: Result<Vec<u8>, png::EncodingError>,
    bounds: Option<[u32; 4]>,
}

/// One layer as a mono NanoDLP panel reads it: 3 greys a pixel, red first.
fn layer_png(pixels: &[u8], width: u32, height: u32) -> Result<Vec<u8>, png::EncodingError> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width / 3, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Balanced);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(pixels)?;
        writer.finish()?;
    }
    Ok(bytes)
}

fn encode(rows: &[RleRun], width: u32, height: u32) -> Encoded {
    let mut pixels = Vec::with_capacity(width as usize * height as usize);
    for run in rows {
        pixels.extend(std::iter::repeat_n(run.value, run.length as usize));
    }
    Encoded {
        png: layer_png(&pixels, width, height),
        bounds: LayerStats::of(rows, width)
            .bounds
            .map(|b| [b.x0, b.y0, b.x1, b.y1]),
    }
}

/// Writes `stack` as a NanoDLP plate running the settings in `program`.
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
    if width % 3 != 0 {
        return Err(WriteFailure::Unencodable(format!(
            "a mono NanoDLP panel packs 3 pixels in one, and {width} pixels across is not a multiple of 3"
        )));
    }
    let machine = machine(program);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut zip = ZipWriter::new(out);
    entry(&mut zip, "meta.json", meta().as_bytes(), options)?;
    entry(
        &mut zip,
        "profile.json",
        profile(program, &machine).as_bytes(),
        options,
    )?;
    let png = png_bytes(&preview).map_err(|err| WriteFailure::Io(io::Error::other(err)))?;
    let mut bounds = Vec::with_capacity(program.layers.len());
    // The layers before the manifests that state where they are lit.
    write_layer_images(
        stack,
        &program.raster,
        1,
        |_, rows| encode(rows, width, height),
        |index, encoded: Encoded| -> Result<(), WriteFailure> {
            let layer_png = encoded
                .png
                .map_err(|err| WriteFailure::Io(io::Error::other(err)))?;
            bounds.push(encoded.bounds);
            entry(&mut zip, &format!("{}.png", index + 1), &layer_png, options)
        },
    )?;
    let model = boundary(program, &bounds);
    entry(
        &mut zip,
        "slicer.json",
        slicer(program, &machine, &model).as_bytes(),
        options,
    )?;
    entry(
        &mut zip,
        "plate.json",
        plate(program, &model).as_bytes(),
        options,
    )?;
    entry(
        &mut zip,
        "info.json",
        info(stack, &bounds).as_bytes(),
        options,
    )?;
    entry(&mut zip, "3d.png", &png, options)?;
    zip.finish().map_err(zip_failure)?;
    Ok(())
}

#[cfg(test)]
mod tests_nanodlp;
