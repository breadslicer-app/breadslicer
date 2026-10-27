/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The active edge list must not change a single pixel: the fills
//! here are checked against the full scan they replaced, which tested
//! every edge on every row.

use proptest::prelude::*;

use super::*;
use crate::contour::{ClosedRing, Polygon};
use crate::geom2d::Point2D;
use crate::profile::{Mirror, PixelResolution, PositiveFiniteMm};

/// The binary fill as it was, scanning every edge on every row.
fn full_scan_binary(layer: &ContourLayer, pixel_map: &PixelMap) -> LayerRaster {
    let width_px = pixel_map.pixel_resolution().x();
    let height_px = pixel_map.pixel_resolution().y();
    let edges = collect_edges(layer, pixel_map);
    let mut acc = RleRunAccumulator::new();
    let mut crossings: Vec<(f64, i32)> = Vec::new();
    for row in 0..height_px {
        let y = f64::from(row) + 0.5;
        crossings.clear();
        for edge in &edges {
            if (edge.ay > y) != (edge.by > y) {
                let t = (y - edge.ay) / (edge.by - edge.ay);
                let x_int = (edge.bx - edge.ax).mul_add(t, edge.ax);
                let dir = if edge.ay < edge.by { 1 } else { -1 };
                crossings.push((x_int, dir));
            }
        }
        crossings.sort_by(|p, q| p.0.total_cmp(&q.0));
        let width_minus_1 = i64::from(width_px) - 1;
        let mut prev_pixel: u32 = 0;
        let mut winding: i32 = 0;
        for (x_c, dir) in &crossings {
            let last_before = ((*x_c - 0.5).ceil() as i64 - 1).clamp(-1, width_minus_1);
            if last_before >= i64::from(prev_pixel) {
                let count = (last_before as u32) - prev_pixel + 1;
                acc.push(if winding != 0 { 255 } else { 0 }, count);
                prev_pixel = last_before as u32 + 1;
            }
            winding += dir;
        }
        if prev_pixel < width_px {
            acc.push(if winding != 0 { 255 } else { 0 }, width_px - prev_pixel);
        }
    }
    LayerRaster::new(layer.z_mm(), width_px, height_px, acc.into_runs())
}

/// The anti-aliased fill as it was, scanning every edge on every
/// sub-scanline.
fn full_scan_aa(
    layer: &ContourLayer,
    pixel_map: &PixelMap,
    n: u32,
    min_alpha_pct: f32,
) -> LayerRaster {
    let width_px = pixel_map.pixel_resolution().x();
    let height_px = pixel_map.pixel_resolution().y();
    let edges = collect_edges(layer, pixel_map);
    let mut acc = RleRunAccumulator::new();
    let mut row_acc: Vec<f64> = vec![0.0; width_px as usize];
    let mut crossings: Vec<(f64, i32)> = Vec::new();
    let threshold =
        ((f64::from(min_alpha_pct) / 100.0 * 255.0).round() as i32).clamp(0, 256) as u32;
    let per_sub_strip = 1.0 / f64::from(n);
    for row in 0..height_px {
        row_acc.iter_mut().for_each(|v| *v = 0.0);
        for k in 0..n {
            let y = f64::from(row) + (f64::from(k) + 0.5) / f64::from(n);
            crossings.clear();
            for edge in &edges {
                if (edge.ay > y) != (edge.by > y) {
                    let t = (y - edge.ay) / (edge.by - edge.ay);
                    let x_int = (edge.bx - edge.ax).mul_add(t, edge.ax);
                    let dir = if edge.ay < edge.by { 1 } else { -1 };
                    crossings.push((x_int, dir));
                }
            }
            crossings.sort_by(|p, q| p.0.total_cmp(&q.0));
            let mut winding = 0i32;
            let mut span_start: Option<f64> = None;
            for (x_c, dir) in &crossings {
                let prev = winding;
                winding += dir;
                if prev == 0 && winding != 0 {
                    span_start = Some(*x_c);
                } else if prev != 0
                    && winding == 0
                    && let Some(start) = span_start.take()
                {
                    let _ = accumulate_span(&mut row_acc, start, *x_c, width_px, per_sub_strip);
                }
            }
        }
        for &cov in &row_acc {
            let clamped = ((cov * 255.0).round() as i32).clamp(0, 255) as u32;
            acc.push(
                if clamped > 0 && clamped < threshold {
                    0
                } else {
                    clamped as u8
                },
                1,
            );
        }
    }
    LayerRaster::new(layer.z_mm(), width_px, height_px, acc.into_runs())
}

fn pixel_map(w: u32, h: u32, mirror: Mirror) -> PixelMap {
    PixelMap::new(
        PositiveFiniteMm::new(10.0).unwrap(),
        PositiveFiniteMm::new(8.0).unwrap(),
        PixelResolution::new(w, h).unwrap(),
        mirror,
    )
}

/// Rings with arbitrary (self-crossing, touching, off-plate) vertices,
/// including vertices exactly on pixel centres and row boundaries.
fn layer_strategy() -> impl Strategy<Value = ContourLayer> {
    let coord = prop_oneof![
        3 => -7.0f64..7.0,
        1 => (-14i32..14).prop_map(|q| f64::from(q) * 0.5),
        1 => (-56i32..56).prop_map(|q| f64::from(q) * 0.125),
    ];
    let ring = prop::collection::vec((coord.clone(), coord), 3..14);
    prop::collection::vec(ring, 1..5).prop_map(|rings| {
        let polygons = rings
            .into_iter()
            .map(|pts| {
                let ring =
                    ClosedRing::new(pts.into_iter().map(|(x, y)| Point2D::new(x, y)).collect());
                Polygon::new_unchecked(ring, vec![])
            })
            .collect();
        ContourLayer::new(0.05, polygons)
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn the_binary_fill_matches_the_full_scan(layer in layer_strategy(), mirror_x in any::<bool>()) {
        let pm = pixel_map(61, 47, Mirror { x: mirror_x, y: false });
        prop_assert_eq!(rasterize_layer_binary(&layer, &pm), full_scan_binary(&layer, &pm));
    }

    #[test]
    fn the_aa_fill_matches_the_full_scan(layer in layer_strategy(), n in prop_oneof![Just(2u32), Just(4), Just(8)], floor in 0.0f32..30.0) {
        let pm = pixel_map(53, 41, Mirror::default());
        let aa = match n { 2 => AaLevel::X2, 4 => AaLevel::X4, _ => AaLevel::X8 };
        prop_assert_eq!(rasterize_layer(&layer, &pm, aa, floor), full_scan_aa(&layer, &pm, n, floor));
    }
}

#[test]
fn a_layer_with_no_edges_is_one_empty_run() {
    let pm = pixel_map(20, 10, Mirror::default());
    let layer = ContourLayer::new(0.05, vec![]);
    assert_eq!(
        rasterize_layer_binary(&layer, &pm),
        full_scan_binary(&layer, &pm)
    );
}
