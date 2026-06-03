/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Binary scanline polygon fill from a vector contour layer.
//!
//! Each integer pixel row is sampled at its centre `Y + 0.5`. The
//! row's edge crossings are computed in subpixel pixel space, sorted
//! by X, and walked left to right while maintaining a signed winding
//! count: an upward-going edge contributes `+1`, a downward one `-1`.
//! A pixel is filled (`255`) iff the running count is non-zero at
//! its centre `X + 0.5`, and `0` otherwise.
//!
//! The non-zero winding rule is invariant under raster mirroring:
//! flipping an axis flips the sign of every edge's contribution, but
//! the `!= 0` partition stays the same. The same fill rule therefore
//! handles outer-CCW + holes-CW polygons (interior count = `+1`,
//! hole interior count = `0`) and their mirror-image variants
//! without special casing.
//!
//! For each row, runs of equal-value adjacent pixels are pushed into
//! a single [`RleRunAccumulator`], which merges across row breaks
//! when consecutive rows end and begin with the same value.

use crate::contour::{ClosedRing, ContourLayer};
use crate::raster::pixel_map::PixelMap;
use crate::raster::rle::{LayerRaster, RleRunAccumulator};

#[derive(Clone, Copy)]
struct Edge {
    ax: f64,
    ay: f64,
    bx: f64,
    by: f64,
}

fn push_ring_edges(ring: &ClosedRing, pixel_map: &PixelMap, edges: &mut Vec<Edge>) {
    let vertices = ring.vertices();
    let n = vertices.len();
    if n < 2 {
        return;
    }
    let pts: Vec<(f64, f64)> = vertices.iter().map(|v| pixel_map.mm_to_pixel(*v)).collect();
    for i in 0..n {
        let (ax, ay) = pts[i];
        let (bx, by) = pts[(i + 1) % n];
        edges.push(Edge { ax, ay, bx, by });
    }
}

fn collect_edges(layer: &ContourLayer, pixel_map: &PixelMap) -> Vec<Edge> {
    let mut edges = Vec::new();
    for polygon in layer.polygons() {
        push_ring_edges(polygon.outer(), pixel_map, &mut edges);
        for hole in polygon.holes() {
            push_ring_edges(hole, pixel_map, &mut edges);
        }
    }
    edges
}

/// Rasterise `layer` to a binary (`0` or `255`) [`LayerRaster`]
/// sized by `pixel_map`'s pixel resolution. The output layer
/// inherits `layer.z_mm()`.
pub fn rasterize_layer_binary(layer: &ContourLayer, pixel_map: &PixelMap) -> LayerRaster {
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
        crossings.sort_by(|p, q| p.0.partial_cmp(&q.0).unwrap());

        let width_minus_1 = i64::from(width_px) - 1;
        let mut prev_pixel: u32 = 0;
        let mut winding: i32 = 0;
        for (x_c, dir) in &crossings {
            let last_before = ((*x_c - 0.5).ceil() as i64 - 1).clamp(-1, width_minus_1);
            if last_before >= i64::from(prev_pixel) {
                let count = (last_before as u32) - prev_pixel + 1;
                let value = if winding != 0 { 255 } else { 0 };
                acc.push(value, count);
                prev_pixel = last_before as u32 + 1;
            }
            winding += dir;
        }
        if prev_pixel < width_px {
            let count = width_px - prev_pixel;
            let value = if winding != 0 { 255 } else { 0 };
            acc.push(value, count);
        }
    }

    LayerRaster::new(layer.z_mm(), width_px, height_px, acc.into_runs())
}

#[cfg(test)]
mod scanline_tests {
    use super::*;
    use crate::contour::{ClosedRing, Polygon};
    use crate::geom2d::Point2D;
    use crate::profile::{BuildVolume, Mirror, PixelResolution};

    fn p(x: f64, y: f64) -> Point2D {
        Point2D::new(x, y)
    }

    /// 1 mm x 1 mm build, 100 px x 100 px resolution. One pixel
    /// equals 1/100 mm.
    fn one_mm_per_hundred_pixels() -> PixelMap {
        PixelMap::new(
            BuildVolume::new(1.0, 1.0, 1.0).unwrap(),
            PixelResolution::new(100, 100).unwrap(),
            Mirror::default(),
        )
    }

    fn pixel_value(layer: &LayerRaster, x: u32, y: u32) -> u8 {
        let target_idx = u64::from(y) * u64::from(layer.width_px()) + u64::from(x);
        let mut cumulative = 0u64;
        for run in layer.runs() {
            cumulative += u64::from(run.length);
            if cumulative > target_idx {
                return run.value;
            }
        }
        panic!("pixel ({x}, {y}) sits past the end of the run buffer");
    }

    fn ccw_square(min: Point2D, max: Point2D) -> ClosedRing {
        ClosedRing::new(vec![min, p(max.x, min.y), max, p(min.x, max.y)])
    }

    fn cw_square(min: Point2D, max: Point2D) -> ClosedRing {
        ClosedRing::new(vec![min, p(min.x, max.y), max, p(max.x, min.y)])
    }

    #[test]
    fn empty_layer_emits_a_single_zero_run_spanning_the_raster() {
        let layer = ContourLayer::new(0.05, vec![]);
        let raster = rasterize_layer_binary(&layer, &one_mm_per_hundred_pixels());
        assert_eq!(raster.runs().len(), 1);
        assert_eq!(raster.runs()[0].value, 0);
        assert_eq!(u64::from(raster.runs()[0].length), 100 * 100);
    }

    #[test]
    fn unit_square_fills_every_pixel_with_255() {
        let outer = ccw_square(p(0.0, 0.0), p(1.0, 1.0));
        let polygon = Polygon::new(outer, vec![]).unwrap();
        let layer = ContourLayer::new(0.05, vec![polygon]);
        let raster = rasterize_layer_binary(&layer, &one_mm_per_hundred_pixels());

        assert_eq!(raster.width_px(), 100);
        assert_eq!(raster.height_px(), 100);
        assert_eq!(raster.z_mm(), 0.05);

        let total: u64 = raster.runs().iter().map(|r| u64::from(r.length)).sum();
        assert_eq!(total, 100 * 100);
        for run in raster.runs() {
            assert_eq!(run.value, 255);
        }
    }

    #[test]
    fn polygon_with_hole_leaves_hole_pixels_at_zero() {
        // CCW outer covering [0, 1]^2 mm, with a CW hole over
        // [0.25, 0.75]^2 mm. On a 100x100 / 1x1 mm build the hole
        // covers integer pixel rows / cols 25..=74.
        let outer = ccw_square(p(0.0, 0.0), p(1.0, 1.0));
        let hole = cw_square(p(0.25, 0.25), p(0.75, 0.75));
        let polygon = Polygon::new(outer, vec![hole]).unwrap();
        let layer = ContourLayer::new(0.05, vec![polygon]);
        let raster = rasterize_layer_binary(&layer, &one_mm_per_hundred_pixels());

        // Centre of hole sits at 0.
        assert_eq!(pixel_value(&raster, 50, 50), 0);
        // Corner inside the hole sits at 0.
        assert_eq!(pixel_value(&raster, 30, 30), 0);
        assert_eq!(pixel_value(&raster, 70, 70), 0);
        // Outside-the-hole-but-inside-the-outer pixels are filled.
        assert_eq!(pixel_value(&raster, 10, 50), 255);
        assert_eq!(pixel_value(&raster, 90, 50), 255);
        assert_eq!(pixel_value(&raster, 50, 10), 255);
        assert_eq!(pixel_value(&raster, 50, 90), 255);
        // Top-left corner of the raster is inside the outer.
        assert_eq!(pixel_value(&raster, 0, 0), 255);
    }

    #[test]
    fn small_polygon_fills_only_the_pixels_it_covers() {
        // Half-mm square in the middle of the build plate. With
        // 1 mm build and 100 px resolution this covers pixel rows
        // / cols 25..=74.
        let outer = ccw_square(p(0.25, 0.25), p(0.75, 0.75));
        let polygon = Polygon::new(outer, vec![]).unwrap();
        let layer = ContourLayer::new(0.05, vec![polygon]);
        let raster = rasterize_layer_binary(&layer, &one_mm_per_hundred_pixels());

        // Outside the polygon stays 0.
        assert_eq!(pixel_value(&raster, 10, 10), 0);
        assert_eq!(pixel_value(&raster, 90, 90), 0);
        // Inside is 255.
        assert_eq!(pixel_value(&raster, 50, 50), 255);
        assert_eq!(pixel_value(&raster, 30, 30), 255);
        assert_eq!(pixel_value(&raster, 70, 70), 255);

        // Total 255 pixels: 50x50 = 2500.
        let filled: u64 = raster
            .runs()
            .iter()
            .filter(|r| r.value == 255)
            .map(|r| u64::from(r.length))
            .sum();
        assert_eq!(filled, 50 * 50);
    }

    #[test]
    fn rasterise_inherits_layer_z_and_pixel_map_dimensions() {
        let outer = ccw_square(p(0.0, 0.0), p(1.0, 1.0));
        let polygon = Polygon::new(outer, vec![]).unwrap();
        let layer = ContourLayer::new(0.42, vec![polygon]);
        let pm = PixelMap::new(
            BuildVolume::new(1.0, 1.0, 1.0).unwrap(),
            PixelResolution::new(50, 80).unwrap(),
            Mirror::default(),
        );
        let raster = rasterize_layer_binary(&layer, &pm);
        assert_eq!(raster.z_mm(), 0.42);
        assert_eq!(raster.width_px(), 50);
        assert_eq!(raster.height_px(), 80);
        let total: u64 = raster.runs().iter().map(|r| u64::from(r.length)).sum();
        assert_eq!(total, 50 * 80);
    }
}
