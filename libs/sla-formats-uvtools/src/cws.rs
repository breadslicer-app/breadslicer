/*
 * SPDX-FileCopyrightText: 2020-2026 PTRTECH, Tiago Conceição (sn4k3) and UVtools contributors
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 *
 * Ported to Rust from UVtools UVtools.Core/FileFormats/CWSFile.cs (with the
 * archive helpers of UVtools.Core/FileFormats/FileFormat.cs), UVtools v7.1.1,
 * commit 8b5c91fd41b60bd6265ba1c1f2b2b64636b873e8, modified for Breadslicer:
 * the layers are encoded from the slicer's run-length rows, the settings come
 * from a Breadslicer print program (the bottom layer count included, which
 * UVtools leaves at its default in the G-code header), the files are named
 * after Breadslicer, and the end raise runs at the fastest speed the program
 * uses.
 *
 * This program is free software: you can redistribute it and/or modify it
 * under the terms of the GNU Affero General Public License as published by the
 * Free Software Foundation, either version 3 of the License, or (at your
 * option) any later version. It is distributed WITHOUT ANY WARRANTY; see the
 * GNU Affero General Public License (LICENSE.md) for details.
 */

//! Creation Workshop's CWS archive, in its 3 shapes (#778):
//!
//! - `.cws`, NovaMaker's (Bene4, Bene6, Elfin, Elfin2, Elfin3 Mini): a
//!   `slice.conf` of `key = value` lines and grey layers.
//! - `.rgb.cws`, for NovaMaker panels that light 3 sub-pixels per pixel (Bene4
//!   Mono, Bene5, Elfin2 Mono SE, Whale, Whale2): as `.cws`, but each layer
//!   packs 3 grey pixels into one RGB pixel, so the image is a third as wide.
//! - `.xml.cws`, Creation Workshop X's (Wanhao D7): a `manifest.xml` naming
//!   the files, the build profile as XML in a `.slicing` file, and the layers as
//!   RGBA images.
//!
//! All 3 carry the G-code that runs the print: relative moves, `;<Slice> n` to
//! show layer `n`, `;<Delay> ms` to wait.

use std::io;

use sc_sl1cer::contour::ContourStack;
use sc_sl1cer::raster::rle::RleRun;
use sc_sla_formats::layer_images::write_layer_images;
use sc_sla_formats::{PrintProgram, PrinterFileSink, WriteFailure};
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::gcode::{
    Builder, Dialect, Job, MoveCommand, NoHooks, Positioning, ShowImage, layers_of, num,
};
use crate::program::{ProgramSummary, round_away, round_display, round_height};
use crate::sl1::{entry, zip_failure};

/// Which CWS this is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CwsVariant {
    /// `.cws`: grey layers and `slice.conf`.
    Cws,
    /// `.rgb.cws`: 3 grey pixels per RGB pixel and `slice.conf`.
    Rgb,
    /// `.xml.cws`: RGBA layers, `manifest.xml` and the XML build profile.
    Xml,
}

/// What the archive's files are named after. It must not end with a digit:
/// the layers are named after it with their number.
pub(crate) const FILE_NAME: &str = "breadslicer";
pub(crate) const SLICE_CONF: &str = "slice.conf";
pub(crate) const MANIFEST: &str = "manifest.xml";

fn digits(count: usize) -> usize {
    count.max(1).to_string().len()
}

/// The name of layer `index` among `count`.
pub(crate) fn layer_entry(index: usize, count: usize) -> String {
    format!("{FILE_NAME}{index:0width$}.png", width = digits(count))
}

fn dialect(variant: CwsVariant) -> Dialect {
    let mut dialect = Dialect {
        positioning: Positioning::Relative,
        show_image: ShowImage::Index0,
        layer_move: MoveCommand::G1,
        end_move: MoveCommand::G1,
        ..Dialect::default()
    };
    dialect.sync_movements.enabled = true;
    dialect.wait_sync_delay.enabled = true;
    dialect.show_image_m6054.set(";<Slice>", Some("{0}"));
    let sync = if variant == CwsVariant::Xml {
        ";<Takes>"
    } else {
        ";<Delay>"
    };
    dialect.wait_sync_delay.set(sync, Some("0{0}"));
    dialect.wait_g4.set(";<Delay>", Some("{0}"));
    dialect
}

fn flag(on: bool) -> &'static str {
    if on { "True" } else { "False" }
}

fn ms(seconds: f32) -> u32 {
    (seconds * 1000.0) as u32
}

/// The settings the G-code states first, as `;(name = value)` comments.
fn output_settings(program: &PrintProgram) -> String {
    let summary = ProgramSummary::of(program);
    let (b, n) = (summary.bottom, summary.normal);
    let raster = &program.raster;
    let panel = raster.panel_resolution();
    let (width_mm, height_mm) = raster.panel_size_mm();
    let mirror = raster.mirror();
    let aa = raster.aa().factor();
    let (xppm, yppm) = ppmm(program);
    let layers = program.layers.len();
    let entries: Vec<(&str, String)> = vec![
        ("Pix per mm X", num(xppm)),
        ("Pix per mm Y", num(yppm)),
        ("X Resolution", panel.x().to_string()),
        ("Y Resolution", panel.y().to_string()),
        (
            "Layer Thickness",
            num(round_height(program.layer_height_mm.value() as f32)),
        ),
        ("Layer Time", ms(n.exposure_sec).to_string()),
        ("Render Outlines", flag(false).to_owned()),
        ("Outline Width Inset", "2".to_owned()),
        ("Outline Width Outset", "0".to_owned()),
        ("Bottom Layers Time", ms(b.exposure_sec).to_string()),
        (
            "Number of Bottom Layers",
            program.bottom_layer_count.to_string(),
        ),
        ("Blanking Layer Time", "1000".to_owned()),
        ("Build Direction", "Bottom_Up".to_owned()),
        ("Lift Distance", num(round_away(n.lift_mm, 2))),
        ("Slide/Tilt Value", "0".to_owned()),
        ("Use Mainlift GCode Tab", flag(false).to_owned()),
        ("Settle Time", "1600".to_owned()),
        ("Anti Aliasing", flag(aa > 1).to_owned()),
        ("Anti Aliasing Value", aa.to_string()),
        ("Z Lift Feed Rate", num(round_away(n.lift_mm_min, 2))),
        ("Z Bottom Lift Feed Rate", num(round_away(b.lift_mm_min, 2))),
        ("Z Lift Retract Rate", num(round_away(n.retract_mm_min, 2))),
        ("Flip X", flag(mirror.x).to_owned()),
        ("Flip Y", flag(mirror.y).to_owned()),
        ("Number of Slices", layers.to_string()),
        ("Platform X Size", num(round_display(width_mm as f32))),
        ("Platform Y Size", num(round_display(height_mm as f32))),
        (
            "Platform Z Size",
            num(round_away(raster.build_volume().z().value() as f32, 2)),
        ),
        ("Max X Feedrate", "200".to_owned()),
        ("Max Y Feedrate", "200".to_owned()),
        ("Max Z Feedrate", "200".to_owned()),
        ("Machine Type", "UV_LCD".to_owned()),
        ("Bottom Layer Light PWM", b.pwm.to_string()),
        ("Layer Light PWM", n.pwm.to_string()),
    ];
    let mut text = String::from(";(**** Build and Slicing Parameters ****)\n");
    for (name, value) in entries {
        if name == "Number of Slices" {
            text.push_str(&format!(";{name} = {value}\n"));
        } else {
            text.push_str(&format!(";({name} = {value})\n"));
        }
    }
    text
}

/// Pixels per millimetre across and down the panel.
fn ppmm(program: &PrintProgram) -> (f32, f32) {
    let panel = program.raster.panel_resolution();
    let (width_mm, height_mm) = program.raster.panel_size_mm();
    let per = |pixels: u32, mm: f64| {
        if mm > 0.0 {
            pixels as f32 / round_display(mm as f32)
        } else {
            0.0
        }
    };
    (per(panel.x(), width_mm), per(panel.y(), height_mm))
}

/// `slice.conf`: what the NovaMaker firmware reads.
fn slice_conf(program: &PrintProgram) -> String {
    let summary = ProgramSummary::of(program);
    let (b, n) = (summary.bottom, summary.normal);
    let panel = program.raster.panel_resolution();
    let (xppm, yppm) = ppmm(program);
    let entries: [(&str, String); 14] = [
        ("xppm", num(xppm)),
        ("yppm", num(yppm)),
        ("xres", panel.x().to_string()),
        ("yres", panel.y().to_string()),
        (
            "thickness",
            num(round_height(program.layer_height_mm.value() as f32)),
        ),
        ("layers_num", program.layers.len().to_string()),
        ("head_layers_num", program.bottom_layer_count.to_string()),
        ("layers_expo_ms", ms(n.exposure_sec).to_string()),
        ("head_layers_expo_ms", ms(b.exposure_sec).to_string()),
        (
            "wait_before_expo_ms",
            ms(n.wait_before_cure_sec).to_string(),
        ),
        ("lift_distance", num(round_away(n.lift_mm, 2))),
        ("lift_up_speed", num(round_away(n.lift_mm_min, 2))),
        ("lift_down_speed", num(round_away(n.retract_mm_min, 2))),
        ("lift_when_finished", "80".to_owned()),
    ];
    let mut text = format!(
        "# Breadslicer {}\n# conf version 1.0\n\n",
        env!("CARGO_PKG_VERSION")
    );
    for (key, value) in entries {
        text.push_str(&format!("{key:<24}= {value}\n"));
    }
    text
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// `manifest.xml`, naming the G-code, the profile and every layer.
fn manifest(count: usize) -> String {
    let mut text = String::from(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<manifest FileVersion=\"1\">\n  <Slices>\n",
    );
    for index in 0..count {
        text.push_str(&format!(
            "    <Slice>\n      <name>{}</name>\n    </Slice>\n",
            layer_entry(index, count)
        ));
    }
    text.push_str(&format!(
        "  </Slices>\n  <SliceProfile>\n    <name>{FILE_NAME}.slicing</name>\n  </SliceProfile>\n  <GCode>\n    <name>{FILE_NAME}.gcode</name>\n  </GCode>\n</manifest>"
    ));
    text
}

const GCODE_START: &str = ";********** Header Start ********\r\nG21 ; Set units to be mm\r\nG91 ; Relative Positioning\r\nM17 ; Enable motors\r\n; G28 Z ; Auto-home on print - commented by default";
const GCODE_END: &str = ";********** Footer ********\r\n@cmdCloseShutter ; UV off\r\nG4 P0            ; wait for last lift to complete\r\nG1 Z40.0 F150.0  ; lift model clear of resin\r\nG4 P0            ; sync\r\nM18              ; Disable Motors\r\n;<Completed>";
const GCODE_LIFT: &str = ";********** Lift Sequence %d$CURSLICE ********\r\nG1 Z($ZLiftDist * $ZDir) F{$CURSLICE < $NumFirstLayers?$ZBottomLiftRate:$ZLiftRate}\r\n;<Takes> {$CURSLICE<$NumFirstLayers?%d($ZLiftDist*1000*60/$ZBottomLiftRate):%d($ZLiftDist*1000*60/$ZLiftRate)}\r\nG4 P0            ; Wait for lift rise to complete\r\n;<Delay> %d$TopTime\r\nG1 Z(($LayerThickness-$ZLiftDist) * $ZDir) F$ZRetractRate\r\n;<Takes> %d(($ZLiftDist*1000-$LayerThickness*1000)*60/$ZRetractRate)";
const GCODE_LAYER: &str = ";********** Layer %d$CURSLICE ********\r\n;<Slice> %d$CURSLICE\r\n@cmdOpenShutter  ; UV on\r\n;{$CURSLICE<$NumFirstLayers?<Delay> %d$FirstLayerTime:<Delay> %d$LayerTime}\r\n@cmdCloseShutter ; UV off\r\n;<Slice> Blank\r\n;<Delay> %d$BlankTime";
const NOTES: &str = "Use these as a place to start; tune them for your specific printer and resins.\r\nAlways run a calibration on your resin to dial in for your machine.\r\nProfiles created by Earl Miller for CW v1.0.0.75.";

/// The `.slicing` build profile Creation Workshop X reads, as XML.
fn slice_build_config(program: &PrintProgram) -> String {
    let summary = ProgramSummary::of(program);
    let n = summary.normal;
    let b = summary.bottom;
    let panel = program.raster.panel_resolution();
    let mirror = program.raster.mirror();
    let aa = program.raster.aa().factor();
    let (xppm, yppm) = ppmm(program);
    let fields: Vec<(&str, String)> = vec![
        ("DotsPermmX", num(xppm)),
        ("DotsPermmY", num(yppm)),
        ("XResolution", panel.x().to_string()),
        ("YResolution", panel.y().to_string()),
        ("BlankTime", "0".to_owned()),
        ("TopTime", "0".to_owned()),
        ("PlatformTemp", "75".to_owned()),
        ("ExportSVG", "0".to_owned()),
        ("Export", "False".to_owned()),
        ("ExportPNG", "False".to_owned()),
        ("CalcSliceSize", "False".to_owned()),
        ("XOffset", "0".to_owned()),
        ("YOffset", "0".to_owned()),
        ("Direction", "Bottom_Up".to_owned()),
        ("LiftDistance", num(round_away(n.lift_mm, 2))),
        ("SlideTiltValue", "0".to_owned()),
        ("SettleTime", "1600".to_owned()),
        ("AntiAliasing", flag(aa > 1).to_owned()),
        ("UseMainLiftGCode", "False".to_owned()),
        ("AntiAliasingValue", aa.to_string()),
        ("LiftFeedRate", num(round_away(n.lift_mm_min, 2))),
        ("BottomLiftFeedRate", num(round_away(b.lift_mm_min, 2))),
        ("LiftRetractRate", num(round_away(n.retract_mm_min, 2))),
        ("ExportOption", "ZIP".to_owned()),
        ("RenderOutlines", "False".to_owned()),
        ("OutlineWidth_Inset", "2".to_owned()),
        ("OutlineWidth_Outset", "0".to_owned()),
        ("FlipX", flag(mirror.x).to_owned()),
        ("FlipY", flag(mirror.y).to_owned()),
        ("Notes", NOTES.to_owned()),
        ("GCodeHeader", GCODE_START.to_owned()),
        ("GCodeFooter", GCODE_END.to_owned()),
        ("GCodeLift", GCODE_LIFT.to_owned()),
        ("GCodeLayer", GCODE_LAYER.to_owned()),
    ];
    let mut text = String::from(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<SliceBuildConfig FileVersion=\"2\">\n",
    );
    for (name, value) in fields {
        text.push_str(&format!("  <{name}>{}</{name}>\n", xml_escape(&value)));
    }
    text.push_str(&format!(
        "  <InkConfig>\n    <Name>{}</Name>\n    <SliceHeight>{}</SliceHeight>\n    <LayerTime>{}</LayerTime>\n    <FirstLayerTime>{}</FirstLayerTime>\n    <NumberofBottomLayers>{}</NumberofBottomLayers>\n    <ResinPriceL>0</ResinPriceL>\n  </InkConfig>\n",
        xml_escape(program.material_name.as_str()),
        num(round_height(program.layer_height_mm.value() as f32)),
        ms(n.exposure_sec),
        ms(b.exposure_sec),
        program.bottom_layer_count,
    ));
    text.push_str(&format!(
        "  <SelectedInk>{}</SelectedInk>\n  <MinTestExposure>5000</MinTestExposure>\n  <TestExposureStep>100</TestExposureStep>\n  <ExportPreview>None</ExportPreview>\n  <UseMask>False</UseMask>\n  <MaskFilename>False</MaskFilename>\n</SliceBuildConfig>",
        xml_escape(program.material_name.as_str())
    ));
    text
}

/// The G-code for `program`.
pub(crate) fn gcode(variant: CwsVariant, program: &PrintProgram) -> String {
    let layers = layers_of(program);
    let job = Job::new(program, &layers);
    let mut builder = Builder::new(dialect(variant));
    builder.rebuild(&job, &output_settings(program), &mut NoHooks);
    builder.into_text()
}

/// Encodes one layer as the variant stores it.
fn layer_png(
    variant: CwsVariant,
    rows: &[RleRun],
    width: u32,
    height: u32,
) -> Result<Vec<u8>, png::EncodingError> {
    let mut grey = Vec::with_capacity(width as usize * height as usize);
    for run in rows {
        grey.extend(std::iter::repeat_n(run.value, run.length as usize));
    }
    let (color, out_width, pixels) = match variant {
        CwsVariant::Cws => (png::ColorType::Grayscale, width, grey),
        // OpenCV reads 3 greys as one BGR pixel and writes it as RGB: the
        // first grey is the blue sample.
        CwsVariant::Rgb => {
            let mut rgb = Vec::with_capacity(grey.len());
            for triple in grey.chunks_exact(3) {
                rgb.extend_from_slice(&[triple[2], triple[1], triple[0]]);
            }
            (png::ColorType::Rgb, width / 3, rgb)
        }
        CwsVariant::Xml => {
            let mut rgba = Vec::with_capacity(grey.len() * 4);
            for value in grey {
                rgba.extend_from_slice(&[value, value, value, 255]);
            }
            (png::ColorType::Rgba, width, rgba)
        }
    };
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, out_width, height);
        encoder.set_color(color);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Balanced);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(&pixels)?;
        writer.finish()?;
    }
    Ok(bytes)
}

/// Writes `stack` as a CWS archive of `variant` running the settings in
/// `program`.
///
/// # Errors
///
/// [`WriteFailure::Unencodable`] when an `.rgb.cws` panel is not a whole
/// number of RGB pixels wide, and [`WriteFailure::Io`] when the archive cannot
/// be written.
pub fn write_file(
    variant: CwsVariant,
    program: &PrintProgram,
    stack: &ContourStack,
    out: &mut dyn PrinterFileSink,
) -> Result<(), WriteFailure> {
    let panel = program.raster.panel_resolution();
    let (width, height) = (panel.x(), panel.y());
    if variant == CwsVariant::Rgb && width % 3 != 0 {
        return Err(WriteFailure::Unencodable(format!(
            "an .rgb.cws panel packs 3 pixels in one, and {width} pixels across is not a multiple of 3"
        )));
    }
    let count = program.layers.len();
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut zip = ZipWriter::new(out);
    match variant {
        CwsVariant::Xml => {
            entry(&mut zip, MANIFEST, manifest(count).as_bytes(), options)?;
            entry(
                &mut zip,
                &format!("{FILE_NAME}.slicing"),
                slice_build_config(program).as_bytes(),
                options,
            )?;
        }
        CwsVariant::Cws | CwsVariant::Rgb => {
            entry(
                &mut zip,
                SLICE_CONF,
                slice_conf(program).as_bytes(),
                options,
            )?;
        }
    }
    write_layer_images(
        stack,
        &program.raster,
        1,
        |_, rows| layer_png(variant, rows, width, height),
        |index, png: Result<Vec<u8>, png::EncodingError>| -> Result<(), WriteFailure> {
            let png = png.map_err(|err| WriteFailure::Io(io::Error::other(err)))?;
            entry(&mut zip, &layer_entry(index, count), &png, options)
        },
    )?;
    entry(
        &mut zip,
        &format!("{FILE_NAME}.gcode"),
        gcode(variant, program).as_bytes(),
        options,
    )?;
    zip.finish().map_err(zip_failure)?;
    Ok(())
}

#[cfg(test)]
mod tests_cws;
