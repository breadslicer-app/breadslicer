/*
 * SPDX-FileCopyrightText: 2020-2026 PTRTECH, Tiago Conceição (sn4k3) and UVtools contributors
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 *
 * Ported to Rust from UVtools UVtools.Core/FileFormats/JXSFile.cs, UVtools
 * v7.1.1, commit 8b5c91fd41b60bd6265ba1c1f2b2b64636b873e8, modified for
 * Breadslicer: the layers are encoded from the slicer's run-length rows,
 * `control.json` holds the action list its fields name (UVtools 7.1.1 writes
 * the `config.ini` fields there instead), and the settings come from a
 * Breadslicer print program.
 *
 * This program is free software: you can redistribute it and/or modify it
 * under the terms of the GNU Affero General Public License as published by the
 * Free Software Foundation, either version 3 of the License, or (at your
 * option) any later version. It is distributed WITHOUT ANY WARRANTY; see the
 * GNU Affero General Public License (LICENSE.md) for details.
 */

//! UniFormation's JXS: `.jxs` for the GKone (#778).
//!
//! A ZIP of the layers (`00000.png`, ... : each RGB pixel 3 grey sub-pixels,
//! left to right), `config.ini` with the print settings, and `control.json`
//! with the run as a list of actions: G-code lines, delays in milliseconds
//! and the layers to show.

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
use crate::program::{ProgramSummary, round_away, round_height};
use crate::sl1::{entry, zip_failure};

pub(crate) const CONFIG_FILE: &str = "config.ini";
pub(crate) const CONTROL_FILE: &str = "control.json";
const JOB_NAME: &str = "breadslicer";

pub(crate) fn layer_entry(index: usize) -> String {
    format!("{index:05}.png")
}

fn dialect() -> Dialect {
    let mut dialect = Dialect {
        positioning: Positioning::Relative,
        show_image: ShowImage::FilenamePng0,
        layer_move: MoveCommand::G0,
        end_move: MoveCommand::G0,
        ..Dialect::default()
    };
    dialect.wait_sync_delay.enabled = true;
    dialect
}

fn ms(seconds: f32) -> u32 {
    (seconds * 1000.0) as u32
}

fn config_ini(program: &PrintProgram, footer: &str) -> String {
    let summary = ProgramSummary::of(program);
    let (b, n) = (summary.bottom, summary.normal);
    let (lift2, speed2) = n.second_stage.map_or((0.0, 0.0), |(d, s, _)| (d, s));
    let lines: Vec<(&str, String)> = vec![
        ("Action", "print".to_owned()),
        ("FirstLayerTime", ms(b.exposure_sec).to_string()),
        ("LayerTime", ms(n.exposure_sec).to_string()),
        ("NumFade", "0".to_owned()),
        ("NumLayers", program.layers.len().to_string()),
        (
            "UsedMaterial",
            num(round_away((program.volume_mm3.value() / 1000.0) as f32, 3)),
        ),
        (
            "layerHeight",
            num(round_height(program.layer_height_mm.value() as f32)),
        ),
        ("nJobName", JOB_NAME.to_owned()),
        ("NumBottomLayers", program.bottom_layer_count.to_string()),
        ("LiftDistance1", num(round_away(n.lift_mm, 2))),
        ("LiftFeedrate1", num(round_away(n.lift_mm_min, 2))),
        ("LiftDistance2", num(round_away(lift2, 2))),
        ("LiftFeedrate2", num(round_away(speed2, 2))),
        ("BottomLiftFeedrate", num(round_away(b.lift_mm_min, 2))),
        ("RetractFeedrate", num(round_away(n.retract_mm_min, 2))),
        (
            "ZMoveTimeCompensation",
            ms(n.wait_before_cure_sec).to_string(),
        ),
        (
            "ZMoveTimeCompensationBottom",
            ms(b.wait_before_cure_sec).to_string(),
        ),
        ("OnLightCode", "M106 S255".to_owned()),
        ("OffLightCode", "M106 S0".to_owned()),
        ("GcodeHeader", "G21|G91|M17|M106 S0".to_owned()),
        ("GcodeFooter", footer.to_owned()),
        (
            "ApplicationName",
            format!("Breadslicer {}", env!("CARGO_PKG_VERSION")),
        ),
    ];
    let mut text = String::new();
    for (key, value) in lines {
        text.push_str(&format!("{key}={}\n", value.replace(['\r', '\n'], " ")));
    }
    text
}

/// One entry of the action list.
enum Action {
    Gcode(String),
    Delay(u32),
    LayerNumber(usize),
    Slice(String),
}

/// The action list the G-code reads as, and the last move (the footer).
fn actions(gcode: &str, clear_image: &str) -> (Vec<Action>, String) {
    let mut out = Vec::new();
    let mut last_move = "G1 Z100 F150".to_owned();
    for line in gcode.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with(clear_image) {
            out.push(Action::Slice("<BLANK>".to_owned()));
            continue;
        }
        if line.starts_with(';') {
            continue;
        }
        let line = line.split(';').next().unwrap_or_default().trim_end();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("M6054") {
            let digits: String = rest.chars().filter(char::is_ascii_digit).collect();
            let index: usize = digits.parse().unwrap_or_default();
            out.push(Action::LayerNumber(index + 1));
            out.push(Action::Slice(format!("{{PWD}}/{index:05}.png")));
            continue;
        }
        if let Some(rest) = line.strip_prefix("G4 P") {
            let delay: u32 = rest.trim().parse().unwrap_or_default();
            if delay > 0 {
                out.push(Action::Delay(delay));
            }
            continue;
        }
        if line.starts_with("G0") || line.starts_with("G1") {
            last_move = line.to_owned();
        }
        out.push(Action::Gcode(line.to_owned()));
    }
    (out, last_move)
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

fn control_json(program: &PrintProgram, actions: &[Action]) -> String {
    let mut text = format!(
        "{{\n  \"total_time\": {},\n  \"total_slices\": {},\n  \"used_material\": {},\n  \"action_list\": [",
        num(program.print_time_sec.value() as f32),
        program.layers.len(),
        num(round_away((program.volume_mm3.value() / 1000.0) as f32, 3)),
    );
    for (index, action) in actions.iter().enumerate() {
        let (kind, value) = match action {
            Action::Gcode(line) => ("gcode", json_string(line)),
            Action::Delay(ms) => ("delay", ms.to_string()),
            Action::LayerNumber(number) => ("layerno", number.to_string()),
            Action::Slice(path) => ("slice", json_string(path)),
        };
        if index > 0 {
            text.push(',');
        }
        text.push_str(&format!("\n    [\n      \"{kind}\",\n      {value}\n    ]"));
    }
    text.push_str("\n  ]\n}");
    text
}

/// One layer as the GKone reads it: 3 greys a pixel, red first.
fn layer_png(rows: &[RleRun], width: u32, height: u32) -> Result<Vec<u8>, png::EncodingError> {
    let mut grey = Vec::with_capacity(width as usize * height as usize);
    for run in rows {
        grey.extend(std::iter::repeat_n(run.value, run.length as usize));
    }
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width / 3, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Balanced);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(&grey)?;
        writer.finish()?;
    }
    Ok(bytes)
}

/// Writes `stack` as a JXS running the settings in `program`.
///
/// # Errors
///
/// [`WriteFailure::Unencodable`] when the panel is not a whole number of RGB
/// pixels wide, and [`WriteFailure::Io`] when the archive cannot be written.
pub fn write_file(
    program: &PrintProgram,
    stack: &ContourStack,
    out: &mut dyn PrinterFileSink,
) -> Result<(), WriteFailure> {
    let panel = program.raster.panel_resolution();
    let (width, height) = (panel.x(), panel.y());
    if width % 3 != 0 {
        return Err(WriteFailure::Unencodable(format!(
            "a .jxs panel packs 3 pixels in one, and {width} pixels across is not a multiple of 3"
        )));
    }
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut zip = ZipWriter::new(out);
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
    let layers = layers_of(program);
    let job = Job::new(program, &layers);
    let dialect = dialect();
    let clear = dialect.clear_image.name.clone();
    let mut builder = Builder::new(dialect);
    builder.rebuild(&job, "", &mut NoHooks);
    let (actions, footer) = actions(builder.text(), &clear);
    entry(
        &mut zip,
        CONFIG_FILE,
        config_ini(program, &footer).as_bytes(),
        options,
    )?;
    entry(
        &mut zip,
        CONTROL_FILE,
        control_json(program, &actions).as_bytes(),
        options,
    )?;
    zip.finish().map_err(zip_failure)?;
    Ok(())
}

#[cfg(test)]
mod tests_jxs;
