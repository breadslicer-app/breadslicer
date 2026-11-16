/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! What the raster keeps of the contours (#43), as the slicing research
//! measured it against UVtools: the binary raster is unbiased (a wall's
//! mean width is its width), anti-aliasing keeps the area, and its 50 %
//! threshold is the binary raster.

use sc_sl1cer::contour::ContourLayer;
use sc_sl1cer::profile::AaLevel;
use sc_sl1cer::raster::pixel_map::PixelMap;
use sc_sl1cer::raster::rasterize_layer;

use crate::overlapping_parts::{PLATE_MM, PLATE_PX, Welded, pixel_map, slice};

const PIXEL_MM: f64 = PLATE_MM / PLATE_PX as f64;

/// Each pixel's grey, row-major.
fn greys(layer: &ContourLayer, map: &PixelMap, aa: AaLevel) -> Vec<u8> {
    rasterize_layer(layer, map, aa, 0.0)
        .runs()
        .iter()
        .flat_map(|run| std::iter::repeat_n(run.value, run.length as usize))
        .collect()
}

/// The layer through the middle of a 0.5 mm tall prism of `outline`.
fn layer_of(outline: &[[f64; 2]], caps: &[[usize; 3]]) -> ContourLayer {
    let mut mesh = Welded::default();
    mesh.prism(outline, outline, caps, [0.0, 0.5]);
    slice(&mesh.mesh(), 10).layers()[5].clone()
}

/// A wall `width` mm wide leaning 7° from the Y axis, from y −8 to 8 mm:
/// its edges cross every pixel phase.
fn leaning_wall(width: f64) -> ContourLayer {
    let lean = 7f64.to_radians().tan();
    let x = |y: f64, edge: f64| edge + lean * y;
    layer_of(
        &[
            [x(-8.0, -1.0), -8.0],
            [x(-8.0, -1.0 + width), -8.0],
            [x(8.0, -1.0 + width), 8.0],
            [x(8.0, -1.0), 8.0],
        ],
        &[[0, 1, 2], [0, 2, 3]],
    )
}

/// A 64-sided disc of radius `r` mm, and its area.
fn disc(r: f64) -> (ContourLayer, f64) {
    let n = 64;
    let outline: Vec<[f64; 2]> = (0..n)
        .map(|k| {
            let a = k as f64 / n as f64 * std::f64::consts::TAU + 0.1;
            [r * a.cos() + 0.37, r * a.sin() - 0.21]
        })
        .collect();
    let caps: Vec<[usize; 3]> = (1..n - 1).map(|k| [0, k, k + 1]).collect();
    let area = 0.5 * n as f64 * r * r * (std::f64::consts::TAU / n as f64).sin();
    (layer_of(&outline, &caps), area)
}

/// The binary raster is unbiased: across the rows of walls from 1 px to
/// a few millimetres, the mean lit width is the wall's within 0.05 px,
/// and no row of a 1 px wall is empty.
#[test]
fn a_walls_mean_width_is_its_width() {
    let map = pixel_map();
    let w = PLATE_PX as usize;
    for width_px in [1.0, 1.37, 2.5, 10.0, 83.3] {
        let layer = leaning_wall(width_px * PIXEL_MM);
        let lit = greys(&layer, &map, AaLevel::Off);
        let rows: Vec<usize> = lit
            .chunks(w)
            .map(|row| row.iter().filter(|&&g| g != 0).count())
            .filter(|&n| n > 0)
            .collect();
        // Rows wholly within the wall's height, away from its ends.
        let inner = &rows[20..rows.len() - 20];
        let mean = inner.iter().sum::<usize>() as f64 / inner.len() as f64;
        assert!(
            (mean - width_px).abs() <= 0.05,
            "{width_px} px wide: {mean}"
        );
        if width_px >= 1.0 {
            assert!(inner.iter().all(|&n| n > 0), "{width_px} px: a row empty");
        }
    }
}

/// Anti-aliasing keeps the area: the greys add up to the polygon's area
/// within 0.01 %, at every level.
#[test]
fn anti_aliasing_keeps_the_area() {
    let map = pixel_map();
    let (layer, area) = disc(5.0);
    for aa in [AaLevel::X2, AaLevel::X4, AaLevel::X8] {
        let lit: f64 = greys(&layer, &map, aa)
            .iter()
            .map(|&g| f64::from(g) / 255.0)
            .sum::<f64>()
            * PIXEL_MM
            * PIXEL_MM;
        assert!(
            ((lit - area) / area).abs() < 1e-4,
            "{aa:?}: {lit} mm² against {area}"
        );
    }
}

/// At 50 %, an anti-aliased layer is the binary one: the pixels whose
/// coverage is at least half are the pixels whose centre is inside, but
/// for the few an edge crosses right at their centre.
#[test]
fn half_coverage_is_the_binary_raster() {
    let map = pixel_map();
    let (layer, _) = disc(5.0);
    let binary = greys(&layer, &map, AaLevel::Off);
    let edge_pixels = (std::f64::consts::TAU * 5.0 / PIXEL_MM) as usize;
    for aa in [AaLevel::X4, AaLevel::X8] {
        let differ = greys(&layer, &map, aa)
            .iter()
            .zip(&binary)
            .filter(|&(&g, &b)| (g >= 128) != (b != 0))
            .count();
        assert!(
            differ * 50 <= edge_pixels,
            "{aa:?}: {differ} of {edge_pixels} edge pixels differ"
        );
    }
}
