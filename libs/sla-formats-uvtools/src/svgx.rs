/*
 * SPDX-FileCopyrightText: 2020-2026 PTRTECH, Tiago Conceição (sn4k3) and UVtools contributors
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 *
 * Ported to Rust from UVtools UVtools.Core/FileFormats/FlashForgeSVGXFile.cs,
 * UVtools v7.1.1, commit 8b5c91fd41b60bd6265ba1c1f2b2b64636b873e8, modified
 * for Breadslicer: each layer's paths are the slicer's own contours, projected
 * to the panel, rather than outlines traced around the pixels of its image,
 * and the settings come from a Breadslicer print program.
 *
 * This program is free software: you can redistribute it and/or modify it
 * under the terms of the GNU Affero General Public License as published by the
 * Free Software Foundation, either version 3 of the License, or (at your
 * option) any later version. It is distributed WITHOUT ANY WARRANTY; see the
 * GNU Affero General Public License (LICENSE.md) for details.
 */

//! FlashForge's SVGX: `.svgx` for the Foto, Focus, Hunter and Explorer Max,
//! and the Voxelab Proxima 6 (#778).
//!
//! A vector format: the machine draws each layer from its outline. The file
//! is a 28-byte header (`DLP-II 1.1`, then where the 2 previews and the
//! document start), the 2 previews as bottom-up BGR bitmaps behind a BMP-like
//! table, and an SVG document: the print settings in a `printparams` element,
//! then one `g` per layer whose `path`s fill, even-odd, what the layer cures.
//! Coordinates are millimetres from the panel's centre, `y` growing down the
//! image.


use sc_print_primitives::PanelRotation;
use sc_sl1cer::contour::{ClosedRing, ContourStack};
use sc_sla_formats::layer_images::plate_pixel_map;
use sc_sla_formats::{
    PreviewImage, PreviewSize, PreviewSizes, PrintPreviews, PrintProgram, PrinterFileSink,
    WriteFailure,
};

use crate::chitubox_zip::previews_or_black;
use crate::gcode::num;
use crate::program::{ProgramSummary, round_away, round_display, round_height};

/// The 2 previews: 128 x 128 and 200 x 240.
pub const PREVIEW_SIZES: PreviewSizes = PreviewSizes {
    small: PreviewSize {
        width: 128,
        height: 128,
    },
    large: PreviewSize {
        width: 200,
        height: 240,
    },
};

pub(crate) const IDENTIFIER: &[u8] = b"DLP-II 1.1\n";
const HEADER_LENGTH: u32 = 28;
const PREVIEW_TABLE_LENGTH: u32 = 54;

/// A preview as the file stores it: the BMP-like table, then its pixels as
/// BGR, bottom row first.
fn preview_bytes(image: &PreviewImage) -> Vec<u8> {
    let (width, height) = (image.width(), image.height());
    let rgb = image.rgb();
    let mut bgr = Vec::with_capacity(rgb.len());
    for row in (0..height as usize).rev() {
        let start = row * width as usize * 3;
        for pixel in rgb[start..start + width as usize * 3].chunks_exact(3) {
            bgr.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
        }
    }
    let data = bgr.len() as u32;
    let mut out = Vec::with_capacity(PREVIEW_TABLE_LENGTH as usize + bgr.len());
    out.extend_from_slice(b"BM");
    for value in [
        PREVIEW_TABLE_LENGTH + data,
        0,
        54,
        40,
        width,
        height,
        0x18_0001,
        0,
        data,
        3780,
        3780,
        0,
        0,
    ] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&bgr);
    out
}

/// A layer's paths and the figures its group states.
struct Group {
    paths: Vec<String>,
    area: f32,
    perimeter: f32,
    range: Option<[f32; 4]>,
}

/// The SVG document's text for `program`.
fn document(program: &PrintProgram, stack: &ContourStack) -> Result<String, WriteFailure> {
    let raster = &program.raster;
    if raster.panel_rotation() != PanelRotation::None {
        return Err(WriteFailure::Unencodable(
            "an .svgx panel is never turned".to_owned(),
        ));
    }
    let map = plate_pixel_map(raster);
    let panel = raster.panel_resolution();
    let (width_mm, height_mm) = raster.panel_size_mm();
    let display = (
        round_display(width_mm as f32),
        round_display(height_mm as f32),
    );
    let ppmm = (panel.x() as f32 / display.0, panel.y() as f32 / display.1);
    // The larger side of a pixel, in micrometres, as UVtools scales the
    // figures it states per layer.
    let pixel_um = round_away(
        (display.0 / panel.x() as f32 * 1000.0).max(display.1 / panel.y() as f32 * 1000.0),
        3,
    );
    let point = |p: &sc_sl1cer::geom2d::Point2D| {
        let (px, py) = map.mm_to_pixel(*p);
        (
            round_away(px as f32 / ppmm.0 - display.0 / 2.0, 3),
            round_away(py as f32 / ppmm.1 - display.1 / 2.0, 3),
            px,
            py,
        )
    };
    let ring_path = |ring: &ClosedRing, path: &mut String, range: &mut Option<[f32; 4]>| {
        for (i, vertex) in ring.vertices().iter().enumerate() {
            let (x, y, _, _) = point(vertex);
            *range = Some(match *range {
                None => [x, y, x, y],
                Some(r) => [r[0].min(x), r[1].min(y), r[2].max(x), r[3].max(y)],
            });
            match i {
                0 => path.push_str(&format!("M {} {} L", num(x), num(y))),
                _ => path.push_str(&format!(" {} {}", num(x), num(y))),
            }
        }
        path.push_str(" Z");
    };
    let in_pixels = |ring: &ClosedRing| -> (f64, f64) {
        let points: Vec<(f64, f64)> = ring
            .vertices()
            .iter()
            .map(|v| {
                let (_, _, px, py) = point(v);
                (px, py)
            })
            .collect();
        let mut area = 0.0;
        let mut length = 0.0;
        for (i, a) in points.iter().enumerate() {
            let b = points[(i + 1) % points.len()];
            area += a.0 * b.1 - b.0 * a.1;
            length += ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        }
        ((area / 2.0).abs(), length)
    };

    let layer_count = program.layers.len();
    let groups: Vec<Group> = stack
        .layers()
        .iter()
        .take(layer_count)
        .map(|layer| {
            let mut group = Group {
                paths: Vec::new(),
                area: 0.0,
                perimeter: 0.0,
                range: None,
            };
            for polygon in layer.polygons() {
                if polygon.outer().vertices().len() < 3 {
                    continue;
                }
                let mut path = String::new();
                ring_path(polygon.outer(), &mut path, &mut group.range);
                for hole in polygon.holes() {
                    if hole.vertices().len() < 3 {
                        continue;
                    }
                    path.push(' ');
                    ring_path(hole, &mut path, &mut group.range);
                }
                // As UVtools: the figures of the last outline drawn.
                let (area, length) = in_pixels(polygon.outer());
                group.area = (area / f64::from(pixel_um)).cbrt() as f32;
                group.area = round_away(group.area, 3);
                group.perimeter = round_away((length / f64::from(pixel_um)) as f32, 3);
                group.paths.push(path);
            }
            group
        })
        .collect();

    let mut range = [0.0_f32; 4];
    for group in &groups {
        if let Some(r) = group.range {
            range = [
                range[0].min(r[0]),
                range[1].min(r[1]),
                range[2].max(r[2]),
                range[3].max(r[3]),
            ];
        }
    }
    let summary = ProgramSummary::of(program);
    let max_z = program
        .layers
        .last()
        .map_or(0.0, |l| round_height(l.slab_top_mm.value() as f32));
    let attr = |text: &str| {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    };
    let mut text = String::from(
        "<?xml version=\"1.0\" encoding=\"utf-16\" standalone=\"no\"?>\n<svg xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xmlns:xsd=\"http://www.w3.org/2001/XMLSchema\" version=\"1.1\" xmlns=\"http://www.w3.org/2000/svg\">\n",
    );
    text.push_str(&format!(
        "<printparams machinename=\"{}\" materialname=\"{}\" layerheight=\"{}\" volume=\"{}\" layercount=\"{layer_count}\" lightintensity=\"1\" resolutionx=\"{}\" resolutiony=\"{}\" displaywidth=\"{}\" displayheight=\"{}\" machinez=\"{}\">\n",
        attr(program.machine_name.as_str()),
        attr(program.material_name.as_str()),
        num(round_height(program.layer_height_mm.value() as f32)),
        num(round_away((program.volume_mm3.value() / 1000.0) as f32, 3)),
        panel.x(),
        panel.y(),
        num(display.0),
        num(display.1),
        num(round_away(raster.build_volume().z().value() as f32, 2)),
    ));
    text.push_str(&format!(
        "<projectiontime attachlayer=\"{}\" buildinlayer=\"{}\" attachtime=\"{}\" basetime=\"{}\" />\n",
        program.bottom_layer_count,
        program.transition_layer_count,
        num(round_away(summary.bottom.exposure_sec, 2)),
        num(round_away(summary.normal.exposure_sec, 2)),
    ));
    text.push_str("<projectionadjust x=\"100\" y=\"100\" />\n");
    text.push_str(&format!(
        "<printrange minx=\"{}\" miny=\"{}\" minz=\"0\" maxx=\"{}\" maxy=\"{}\" maxz=\"{}\" />\n</printparams>\n<g id=\"background\">\n</g>\n",
        num(range[0]),
        num(range[1]),
        num(range[2]),
        num(range[3]),
        num(max_z),
    ));
    for (index, group) in groups.iter().enumerate() {
        if group.paths.is_empty() {
            text.push_str(&format!(
                "<g id=\"layer-{index}\" area=\"0\" perimeter=\"0\" />\n"
            ));
            continue;
        }
        text.push_str(&format!(
            "<g id=\"layer-{index}\" area=\"{}\" perimeter=\"{}\">\n",
            num(group.area),
            num(group.perimeter)
        ));
        for path in &group.paths {
            text.push_str(&format!(
                "<path d=\"{path}\" style=\"fill:white\" fill-rule=\"evenodd\" />\n"
            ));
        }
        text.push_str("</g>\n");
    }
    text.push_str("</svg>\n");
    Ok(text)
}

/// Writes `stack` as an SVGX running the settings in `program`.
///
/// # Errors
///
/// [`WriteFailure::PreviewSize`] when a rendering is not the size this format
/// fixes, [`WriteFailure::Unencodable`] for a turned panel, both raised before
/// any byte is written, and [`WriteFailure::Io`] when the file cannot be
/// written.
pub fn write_file(
    program: &PrintProgram,
    previews: Option<&PrintPreviews>,
    stack: &ContourStack,
    out: &mut dyn PrinterFileSink,
) -> Result<(), WriteFailure> {
    let (small, large) = previews_or_black(previews, PREVIEW_SIZES)?;
    let document = document(program, stack)?;
    let first = preview_bytes(&small);
    let second = preview_bytes(&large);
    let first_at = HEADER_LENGTH;
    let second_at = first_at + first.len() as u32;
    let document_at = second_at + second.len() as u32;

    let mut header = [0_u8; HEADER_LENGTH as usize];
    header[..IDENTIFIER.len()].copy_from_slice(IDENTIFIER);
    header[16..20].copy_from_slice(&first_at.to_le_bytes());
    header[20..24].copy_from_slice(&second_at.to_le_bytes());
    header[24..28].copy_from_slice(&document_at.to_le_bytes());
    out.write_all(&header)?;
    out.write_all(&first)?;
    out.write_all(&second)?;
    out.write_all(document.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests_svgx;
