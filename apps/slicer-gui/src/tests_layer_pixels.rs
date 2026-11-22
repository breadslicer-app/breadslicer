/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The layer pane's real pixels (#429): what it shows of a layer is what the
//! exported file holds for it, pixel for pixel.

use std::num::NonZeroU32;
use std::time::Instant;

use bread_sla_formats_uvtools::read::{read_anycubic_zip, read_cxdlp};
use sc_print_glue::export::{export_raster, prepare_export};
use sc_print_primitives::{AaLevel, MaterialId, PositiveFiniteMm, PrinterId};
use sc_print_profiles::{ProfileLibraries, SceneOverrides};
use sc_sl1cer::contour::{ClosedRing, ContourLayer, ContourStack, Polygon};
use sc_sl1cer::geom2d::Point2D;
use sc_sl1cer::profile::{LayerSchedule, UniformSchedule};
use sc_sla_formats::layer_images::layer_pixels;
use sc_sla_formats::{AnycubicZipVariant, CxdlpVersion, PrinterFileFormat};

use super::*;

fn mm(v: f64) -> PositiveFiniteMm {
    PositiveFiniteMm::new(v).expect("positive")
}

/// A ring of `n` points round `(cx, cy)`, counter-clockwise, turned by `turn`
/// radians: a disc for many points, a turned square for 4.
fn ring(cx: f64, cy: f64, r: f64, n: usize, turn: f64) -> ClosedRing {
    ClosedRing::new(
        (0..n)
            .map(|i| {
                let a = turn + i as f64 / n as f64 * std::f64::consts::TAU;
                Point2D::new(cx + r * a.cos(), cy + r * a.sin())
            })
            .collect(),
    )
}

/// A disc off the plate's centre towards +X +Y, a turned square towards
/// -X -Y and a sliver thinner than a pixel: edges at every angle, so the
/// anti-aliasing greys show, and nothing symmetric, so a mirror shows.
fn layer(z_mm: f64) -> ContourLayer {
    let sliver = ClosedRing::new(vec![
        Point2D::new(2.0, -12.0),
        Point2D::new(14.0, -11.0),
        Point2D::new(14.0, -10.98),
        Point2D::new(2.0, -11.98),
    ]);
    ContourLayer::new(
        z_mm,
        vec![
            Polygon::new(ring(10.0, 5.0, 6.0, 90, 0.0), vec![]).expect("ccw"),
            Polygon::new(ring(-15.0, -8.0, 3.5, 4, 0.5), vec![]).expect("ccw"),
            Polygon::new(sliver, vec![]).expect("ccw"),
        ],
    )
}

/// 2 layers at 50 µm.
fn contours() -> PlateContours {
    let stack = ContourStack::new(vec![layer(0.025), layer(0.075)]).expect("ascending");
    let schedule = LayerSchedule::Uniform(UniformSchedule::new(
        mm(0.05),
        mm(0.05),
        NonZeroU32::new(2).expect("2"),
    ));
    PlateContours::new(stack, schedule).expect("2 layers")
}

/// What the pane shows of layer 1 drawn one texel to a printer pixel: the
/// part from -20 to +20 mm round the plate's centre, where the parts lie, as
/// greys, rows from the back. With the plate's size in printer pixels and
/// the part's first column and row.
struct Shown {
    size: (usize, usize),
    x0: usize,
    y0: usize,
    rows: Vec<Vec<u8>>,
}

fn shown(
    contours: &PlateContours,
    printer: &PrinterProfile,
    aa: AaLevel,
    format: PrinterFileFormat,
) -> Shown {
    // The formats' writers, and what they store.
    bread_sla_formats_uvtools::install();
    let setup = export_raster(contours, printer, aa);
    let raster = layer_pixels(&contours.stack().layers()[1], &setup);
    let layer = PixelLayer::new(&raster, &setup);
    let (w, h) = layer.size();
    let plate = printer.hardware.build_volume();
    let (fx, fy) = (20.0 / plate.x_mm(), 20.0 / plate.y_mm());
    let tile = tile(
        &layer,
        View {
            u0: (0.5 - fx) as f32,
            v0: (0.5 - fy) as f32,
            u1: (0.5 + fx) as f32,
            v1: (0.5 + fy) as f32,
            shown_width: w as f32,
            shown_height: h as f32,
        },
        &[],
        &layer.stored_greys(Some(format)),
    )
    .expect("a tile");
    let (tw, th) = (tile.image.width() as usize, tile.image.height() as usize);
    let x0 = (f64::from(tile.u0) * f64::from(w)).round() as usize;
    let y0 = (f64::from(tile.v0) * f64::from(h)).round() as usize;
    let x1 = (f64::from(tile.u1) * f64::from(w)).round() as usize;
    let y1 = (f64::from(tile.v1) * f64::from(h)).round() as usize;
    assert_eq!((x1 - x0, y1 - y0), (tw, th), "one texel to a printer pixel");
    Shown {
        size: (w as usize, h as usize),
        x0,
        y0,
        rows: (0..th)
            .map(|row| (0..tw).map(|x| tile.greys[row * tw + x]).collect())
            .collect(),
    }
}

fn profiles(printer: &str, resin: &str) -> (PrinterProfile, sc_print_profiles::EffectiveMaterial) {
    let libraries = ProfileLibraries::platform_default();
    let printer = libraries
        .load_printer_by_id(&PrinterId::new(printer).expect("id"))
        .expect("bundled printer");
    let (material, _) = libraries
        .effective_material_by_id(&MaterialId::new(resin).expect("id"))
        .expect("bundled resin");
    (printer, material)
}

/// Exports `contours` for `printer` and returns the file's bytes.
fn export(
    contours: &PlateContours,
    printer: &PrinterProfile,
    material: &sc_print_profiles::EffectiveMaterial,
    aa: AaLevel,
    format: PrinterFileFormat,
    name: &str,
) -> Vec<u8> {
    bread_sla_formats_uvtools::install();
    let job = prepare_export(
        contours.clone(),
        printer,
        material,
        &SceneOverrides::default(),
        aa,
        "test",
    )
    .expect("the export prepares");
    let dir = tempfile::tempdir().expect("a folder");
    let path = dir.path().join(name);
    job.write(&path, format, None).expect("the file writes");
    std::fs::read(&path).expect("the file reads")
}

/// The pixels shown match the M7 Pro's file, mirrored in X, with the greys of
/// its anti-aliasing.
#[test]
fn the_pixels_shown_are_the_pwsz_files_own() {
    let (printer, material) = profiles(
        "anycubic-photon-mono-m7-pro",
        "anycubic-abs-like-resin-plus-50um",
    );
    assert!(printer.hardware.mirror().x, "the M7 Pro mirrors X");
    let contours = contours();
    let format = PrinterFileFormat::AnycubicZip(AnycubicZipVariant::Pwsz);
    let shown = shown(&contours, &printer, AaLevel::X4, format);
    let bytes = export(
        &contours,
        &printer,
        &material,
        AaLevel::X4,
        format,
        "plate.pwsz",
    );
    let file = read_anycubic_zip(std::io::Cursor::new(bytes)).expect("the file reads back");
    let image = &file.layers[1];
    let (w, h) = shown.size;
    assert_eq!(image.len(), w * h);
    // The file's rows run from the plate's front, its columns from the
    // plate's maximum X unless the printer mirrors X: the M7 Pro does.
    let mut differ = 0;
    let mut greys = 0;
    let mut lit = 0u64;
    for (r, line) in shown.rows.iter().enumerate() {
        let file_row = h - 1 - (shown.y0 + r);
        for (c, &grey) in line.iter().enumerate() {
            let x = shown.x0 + c;
            if image[file_row * w + x] != grey {
                differ += 1;
            }
            if grey != 0 && grey != 255 {
                greys += 1;
            }
            lit += u64::from(grey);
        }
    }
    assert_eq!(differ, 0, "pixels shown that the file does not hold");
    assert!(greys > 100, "the anti-aliasing greys show: {greys}");
    assert!(shown.rows.iter().flatten().filter(|&&g| g == 255).count() > 10_000);
    let in_file: u64 = image.iter().map(|&g| u64::from(g)).sum();
    assert_eq!(
        in_file, lit,
        "the file lights nothing outside the part shown"
    );
}

/// The pixels shown match the Halot One's file, which turns the plate a
/// quarter turn on its panel.
#[test]
fn the_pixels_shown_are_the_cxdlp_files_own_on_a_turned_panel() {
    let (printer, material) = profiles("creality-halot-one", "creality-abs-like-resin-50um");
    let contours = contours();
    let format = PrinterFileFormat::Cxdlp(CxdlpVersion::V3);
    let shown = shown(&contours, &printer, AaLevel::X8, format);
    let bytes = export(
        &contours,
        &printer,
        &material,
        AaLevel::X8,
        format,
        "plate.cxdlp",
    );
    let file = read_cxdlp(&bytes).expect("the file reads back");
    let image = &file.layers[1];
    let (w, h) = shown.size;
    assert_eq!(image.len(), w * h);
    // Unturned, the raster's rows run from the plate's front and its columns
    // from its maximum X (no mirror); turned a quarter turn counter-clockwise
    // the frame's last column is the file's first row, and its rows are the
    // file's columns.
    let mut differ = 0;
    let mut lit = 0u64;
    for (r, line) in shown.rows.iter().enumerate() {
        let raster_row = h - 1 - (shown.y0 + r);
        for (c, &grey) in line.iter().enumerate() {
            let raster_col = w - 1 - (shown.x0 + c);
            let (file_row, file_col) = (w - 1 - raster_col, raster_row);
            if image[file_row * h + file_col] != grey {
                differ += 1;
            }
            lit += u64::from(grey);
        }
    }
    assert_eq!(differ, 0, "pixels shown that the file does not hold");
    let in_file: u64 = image.iter().map(|&g| u64::from(g)).sum();
    assert_eq!(
        in_file, lit,
        "the file lights nothing outside the part shown"
    );
}

fn square_layer() -> (PixelLayer, RasterSetup) {
    let (printer, _) = profiles(
        "anycubic-photon-mono-m7-pro",
        "anycubic-abs-like-resin-plus-50um",
    );
    let contours = contours();
    let setup = export_raster(&contours, &printer, AaLevel::Off);
    let raster = layer_pixels(&contours.stack().layers()[1], &setup);
    (PixelLayer::new(&raster, &setup), setup)
}

/// Zoomed out, a texel is the mean of the printer pixels it covers; zoomed
/// in, a printer pixel is a block of texels with the grid along its far
/// edges, and only the part on show is drawn.
#[test]
fn zoomed_out_texels_average_and_zoomed_in_pixels_are_blocks_with_a_grid() {
    let (layer, _) = square_layer();
    let every = layer.stored_greys(None);
    let (w, h) = layer.size();
    let out = tile(
        &layer,
        View {
            u0: 0.0,
            v0: 0.0,
            u1: 1.0,
            v1: 1.0,
            shown_width: w as f32 / 8.0,
            shown_height: h as f32 / 8.0,
        },
        &[],
        &every,
    )
    .expect("a tile");
    assert_eq!(out.image.width(), w.div_ceil(8));
    let sum: u64 = (0..h)
        .map(|r| layer.row(r).iter().map(|&g| u64::from(g)).sum::<u64>())
        .sum();
    let texels: u64 = out.greys.iter().map(|&g| u64::from(g)).sum();
    // Opaque everywhere: the pane's coarse image never shows through.
    assert!(out.image.as_slice().iter().all(|p| p.a == 255));
    let ratio = texels as f64 * 64.0 / sum as f64;
    assert!(
        (ratio - 1.0).abs() < 0.01,
        "means keep the lit area: {ratio}"
    );

    // The disc's centre at (10, 5) mm, 8 screen pixels to a printer pixel.
    let (cx, cy) = (
        ((10.0 + 223.642 / 2.0) / 223.642) as f32,
        ((126.976 / 2.0 - 5.0) / 126.976) as f32,
    );
    let zoom = View {
        u0: cx - 0.001,
        v0: cy - 0.001,
        u1: cx + 0.001,
        v1: cy + 0.001,
        shown_width: w as f32 * 8.0,
        shown_height: h as f32 * 8.0,
    };
    let near = tile(&layer, zoom, &[], &every).expect("a tile");
    let (tw, th) = (near.image.width(), near.image.height());
    assert!(tw < 300 && th < 300, "only the part on show: {tw} × {th}");
    assert_eq!(tw % 8, 0);
    let px = near.image.as_slice();
    // Inside the disc: a lit block, its last column and row darker.
    assert_eq!(near.greys[0], 255);
    assert!(px.iter().all(|p| p.a == 255), "opaque");
    assert!(
        px[7].r < px[0].r,
        "the grid runs along the block's far edge"
    );
    assert!(px[7 * tw as usize].r < px[0].r);
    // The highlight marks the pixels it reaches.
    let x0 = (f64::from(near.u0) * f64::from(w)) as u32;
    let y0 = (f64::from(near.v0) * f64::from(h)) as u32;
    let marked = tile(&layer, zoom, &[[x0, y0, x0 + 1, y0 + 1]], &every).expect("a tile");
    let [hr, hg, hb] = crate::slice_cap::HIGHLIGHT_RGB;
    let first = marked.image.as_slice()[0];
    assert_eq!((first.r, first.g, first.b), (hr, hg, hb));
    assert_ne!(
        marked.image.as_slice()[8].r,
        hr,
        "the next pixel is not marked"
    );
}

/// An overlay made with its margin still holds the view after a pan of a
/// third of the view, at the same scale; a zoom does not fit it.
#[test]
fn the_margin_holds_a_pan_and_not_a_zoom() {
    let (layer, _) = square_layer();
    let (w, h) = layer.size();
    let view = View {
        u0: 0.50,
        v0: 0.40,
        u1: 0.52,
        v1: 0.43,
        shown_width: w as f32 * 4.0,
        shown_height: h as f32 * 4.0,
    };
    let (x, y) = axes(&layer, view, MARGIN).expect("axes");
    let panned = View {
        u0: 0.5066,
        u1: 0.5266,
        ..view
    };
    let (px, py) = axes(&layer, panned, 0.0).expect("axes");
    assert!(x.holds(&px) && y.holds(&py), "a pan stays inside");
    let zoomed = View {
        shown_width: w as f32 * 8.0,
        shown_height: h as f32 * 8.0,
        ..view
    };
    let (zx, _) = axes(&layer, zoomed, 0.0).expect("axes");
    assert!(!x.holds(&zx), "another scale is made again");
    let far = View {
        u0: 0.6,
        u1: 0.62,
        ..view
    };
    let (fx, _) = axes(&layer, far, 0.0).expect("axes");
    assert!(!x.holds(&fx), "a view out of the margin is made again");
}

/// A review rectangle in mm lands on the printer pixels it covers.
#[test]
fn a_highlight_in_mm_lands_on_its_printer_pixels() {
    // A 100 × 50 mm plate of 1000 × 500 pixels: a pixel is 0.1 mm.
    let pixels = highlight_pixels(&[[0.0, 0.0, 0.25, 0.1]], (100.0, 50.0), (1000, 500));
    // From the plate's centre: columns 500..503, the row just behind the
    // centre line (rows from the back: 249).
    assert_eq!(pixels, vec![[500, 249, 503, 250]]);
}

/// How long a layer takes to rasterise at the printer's resolution on the
/// largest panel in the catalogue, and to cut the part on show out of it at
/// full zoom. Run with `--release --ignored --nocapture`.
#[test]
#[ignore = "timing, run by hand in release"]
fn a_layer_of_the_largest_panel_rasterises_in_time() {
    let libraries = ProfileLibraries::platform_default();
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/profiles/printers");
    let mut largest: Option<(u64, PrinterProfile)> = None;
    for entry in std::fs::read_dir(dir).expect("printers") {
        let path = entry.expect("entry").path();
        let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(id) = PrinterId::new(id) else { continue };
        let Ok(printer) = libraries.load_printer_by_id(&id) else {
            continue;
        };
        let res = printer.hardware.pixel_resolution();
        let pixels = u64::from(res.x()) * u64::from(res.y());
        if largest.as_ref().is_none_or(|(most, _)| pixels > *most) {
            largest = Some((pixels, printer));
        }
    }
    let (pixels, printer) = largest.expect("a printer");
    let plate = printer.hardware.build_volume();
    // A layer across most of the plate: discs and squares in a grid.
    let mut polygons = Vec::new();
    let (px, py) = (plate.x_mm() * 0.4, plate.y_mm() * 0.4);
    for i in 0..6 {
        for j in 0..4 {
            let cx = -px + i as f64 * px / 2.5;
            let cy = -py + j as f64 * py / 1.5;
            polygons.push(Polygon::new(ring(cx, cy, 7.0, 120, 0.0), vec![]).expect("ccw"));
            polygons.push(Polygon::new(ring(cx + 9.0, cy, 3.0, 4, 0.3), vec![]).expect("ccw"));
        }
    }
    let stack = ContourStack::new(vec![ContourLayer::new(0.025, polygons)]).expect("1 layer");
    let contours = PlateContours::new(
        stack,
        LayerSchedule::Uniform(UniformSchedule::new(
            mm(0.05),
            mm(0.05),
            NonZeroU32::new(1).expect("1"),
        )),
    )
    .expect("1 layer");
    for aa in [AaLevel::Off, AaLevel::X4, AaLevel::X8] {
        let setup = export_raster(&contours, &printer, aa);
        let start = Instant::now();
        let raster = layer_pixels(&contours.stack().layers()[0], &setup);
        let rasterised = start.elapsed();
        let start = Instant::now();
        let layer = PixelLayer::new(&raster, &setup);
        let indexed = start.elapsed();
        let (w, h) = layer.size();
        // Full zoom: a printer pixel 24 screen pixels across, a pane of
        // 1200 × 900.
        let view = View {
            u0: 0.3,
            v0: 0.3,
            u1: 0.3 + 1200.0 / 24.0 / w as f32,
            v1: 0.3 + 900.0 / 24.0 / h as f32,
            shown_width: w as f32 * 24.0,
            shown_height: h as f32 * 24.0,
        };
        let start = Instant::now();
        let near = tile(&layer, view, &[], &layer.stored_greys(None)).expect("a tile");
        let cut_near = start.elapsed();
        let fit = View {
            u0: 0.0,
            v0: 0.0,
            u1: 1.0,
            v1: 1.0,
            shown_width: 1200.0,
            shown_height: 1200.0 * h as f32 / w as f32,
        };
        let start = Instant::now();
        let far = tile(&layer, fit, &[], &layer.stored_greys(None)).expect("a tile");
        let cut_far = start.elapsed();
        println!(
            "{} ({} × {}, {pixels} px), {aa:?}: rasterise {rasterised:?}, index {indexed:?}, \
             full zoom {cut_near:?} ({} × {}), fitted {cut_far:?} ({} × {})",
            printer.identity.name,
            w,
            h,
            near.image.width(),
            near.image.height(),
            far.image.width(),
            far.image.height()
        );
    }
}
