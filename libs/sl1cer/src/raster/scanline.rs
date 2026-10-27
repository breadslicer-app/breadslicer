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
//! Only the edges whose Y span reaches a row are tested against it
//! ([`EdgeRows`]): each row scans the few edges that can cross it
//! instead of the whole layer. The crossing test and the crossing
//! itself are the same arithmetic either way, and the edges are kept
//! in their original order, so the output does not depend on it.
//!
//! For each row, runs of equal-value adjacent pixels are pushed into
//! a single [`RleRunAccumulator`], which merges across row breaks
//! when consecutive rows end and begin with the same value.
//!
//! The per-row crossing sort uses `f64::total_cmp`, so contour input
//! carrying NaN coordinates (constructible through the public
//! [`ContourLayer`] API, which performs no coordinate validation)
//! degrades to deterministic garbage output instead of panicking.

use crate::contour::{ClosedRing, ContourLayer};
use crate::profile::AaLevel;
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

/// The edges a pixel row can cross, kept up to date row by row.
///
/// An edge can only cross a sample line `y` with
/// `min(ay, by) <= y < max(ay, by)`, so for the rows `[row, row + 1)`
/// it matters from row `floor(min)` to row `floor(max)`. The window
/// kept here is one row wider on each side, a superset, because the
/// exact crossing test still runs on every edge it returns. Edges
/// enter in row order and stay sorted by their index in the layer,
/// which is the order the full scan visited them in.
struct EdgeRows {
    /// First and last row each edge can matter for.
    spans: Vec<(i64, i64)>,
    /// Edge indices sorted by their first row (stable, so by index
    /// within a row).
    by_first_row: Vec<u32>,
    next: usize,
    active: Vec<u32>,
}

impl EdgeRows {
    fn new(edges: &[Edge]) -> Self {
        let spans: Vec<(i64, i64)> = edges
            .iter()
            .map(|e| {
                let lo = e.ay.min(e.by).floor() as i64;
                let hi = e.ay.max(e.by).floor() as i64;
                (lo.saturating_sub(1), hi.saturating_add(1))
            })
            .collect();
        let mut by_first_row: Vec<u32> = (0..edges.len() as u32).collect();
        by_first_row.sort_by_key(|&i| spans[i as usize].0);
        Self {
            spans,
            by_first_row,
            next: 0,
            active: Vec::new(),
        }
    }

    /// The edges that can cross row `row`, in layer order. Rows must
    /// be visited in increasing order.
    fn at(&mut self, row: u32) -> &[u32] {
        let row = i64::from(row);
        let spans = &self.spans;
        self.active.retain(|&i| spans[i as usize].1 >= row);
        while let Some(&i) = self.by_first_row.get(self.next) {
            if spans[i as usize].0 > row {
                break;
            }
            self.next += 1;
            if spans[i as usize].1 >= row {
                let at = self.active.partition_point(|&a| a < i);
                self.active.insert(at, i);
            }
        }
        &self.active
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
/// sized by `pixel_map`'s pixel resolution. The layer's coordinates
/// are printer-frame millimetres (XY centred on the plate origin);
/// the centring shift into the pixel grid is `pixel_map`'s job, not
/// the caller's. The output layer inherits `layer.z_mm()`.
pub fn rasterize_layer_binary(layer: &ContourLayer, pixel_map: &PixelMap) -> LayerRaster {
    let width_px = pixel_map.pixel_resolution().x();
    let height_px = pixel_map.pixel_resolution().y();
    let edges = collect_edges(layer, pixel_map);

    let mut rows = EdgeRows::new(&edges);
    let mut acc = RleRunAccumulator::new();
    let mut crossings: Vec<(f64, i32)> = Vec::new();

    for row in 0..height_px {
        let y = f64::from(row) + 0.5;
        crossings.clear();
        for &i in rows.at(row) {
            let edge = &edges[i as usize];
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

/// Rasterise `layer` with the chosen anti-aliasing level. As in
/// [`rasterize_layer_binary`], the layer's coordinates are
/// printer-frame millimetres (XY centred on the plate origin).
///
/// [`AaLevel::Off`] short-circuits to [`rasterize_layer_binary`].
/// [`AaLevel::X2`] / [`AaLevel::X4`] / [`AaLevel::X8`] run N Y
/// sub-scanlines per pixel row and accumulate analytic X-span
/// coverage per pixel column. The per-pixel coverage in `[0, 1]` is
/// then mapped to `[0, 255]` and rounded to the nearest integer.
///
/// `min_alpha_pct` is the resin printer's practical exposure floor:
/// any non-zero pixel whose 8-bit value sits strictly below
/// `round(min_alpha_pct / 100 * 255)` is clamped to `0`. Pass `0.0`
/// to disable the floor.
pub fn rasterize_layer(
    layer: &ContourLayer,
    pixel_map: &PixelMap,
    aa: AaLevel,
    min_alpha_pct: f32,
) -> LayerRaster {
    match aa {
        AaLevel::Off => rasterize_layer_binary(layer, pixel_map),
        AaLevel::X2 => rasterize_layer_aa(layer, pixel_map, 2, min_alpha_pct),
        AaLevel::X4 => rasterize_layer_aa(layer, pixel_map, 4, min_alpha_pct),
        AaLevel::X8 => rasterize_layer_aa(layer, pixel_map, 8, min_alpha_pct),
    }
}

fn rasterize_layer_aa(
    layer: &ContourLayer,
    pixel_map: &PixelMap,
    n: u32,
    min_alpha_pct: f32,
) -> LayerRaster {
    let width_px = pixel_map.pixel_resolution().x();
    let height_px = pixel_map.pixel_resolution().y();
    let edges = collect_edges(layer, pixel_map);

    let mut rows = EdgeRows::new(&edges);
    let mut acc = RleRunAccumulator::new();
    let mut row_acc: Vec<f64> = vec![0.0; width_px as usize];
    let mut crossings: Vec<(f64, i32)> = Vec::new();

    let threshold_raw = (f64::from(min_alpha_pct) / 100.0 * 255.0).round() as i32;
    let threshold = threshold_raw.clamp(0, 256) as u32;
    let per_sub_strip = 1.0 / f64::from(n);

    for row in 0..height_px {
        // Columns `[touched_lo, touched_hi)` received coverage this row;
        // every other column is exactly zero and leaves as one run.
        let mut touched_lo = width_px as usize;
        let mut touched_hi = 0usize;
        let active = rows.at(row);

        for k in 0..n {
            let y = f64::from(row) + (f64::from(k) + 0.5) / f64::from(n);
            crossings.clear();
            for &i in active {
                let edge = &edges[i as usize];
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
                let prev_winding = winding;
                winding += dir;
                let was_filled = prev_winding != 0;
                let is_filled = winding != 0;
                if !was_filled && is_filled {
                    span_start = Some(*x_c);
                } else if was_filled
                    && !is_filled
                    && let Some(start) = span_start.take()
                    && let Some((lo, hi)) =
                        accumulate_span(&mut row_acc, start, *x_c, width_px, per_sub_strip)
                {
                    touched_lo = touched_lo.min(lo);
                    touched_hi = touched_hi.max(hi);
                }
            }
        }

        if touched_lo >= touched_hi {
            acc.push(0, width_px);
            continue;
        }
        acc.push(0, touched_lo as u32);
        let mut run_value = 0u8;
        let mut run_length = 0u32;
        for cov in &mut row_acc[touched_lo..touched_hi] {
            let raw_value = (*cov * 255.0).round() as i32;
            let clamped = raw_value.clamp(0, 255) as u32;
            let final_value = if clamped > 0 && clamped < threshold {
                0
            } else {
                clamped as u8
            };
            *cov = 0.0;
            if final_value == run_value {
                run_length += 1;
            } else {
                acc.push(run_value, run_length);
                run_value = final_value;
                run_length = 1;
            }
        }
        acc.push(run_value, run_length);
        acc.push(0, width_px - touched_hi as u32);
    }

    LayerRaster::new(layer.z_mm(), width_px, height_px, acc.into_runs())
}

/// Distribute one sub-scanline's `[x_start, x_end]` X interval over
/// the pixel columns it touches, weighted by `per_sub_strip` (the
/// 1 / N Y-fraction one sub-scanline contributes). Returns the
/// columns written, `[first, last + 1)`, or `None` for an interval
/// that misses the raster.
fn accumulate_span(
    row_acc: &mut [f64],
    x_start: f64,
    x_end: f64,
    width_px: u32,
    per_sub_strip: f64,
) -> Option<(usize, usize)> {
    let x_start = x_start.max(0.0);
    let x_end = x_end.min(f64::from(width_px));
    if x_end <= x_start {
        return None;
    }

    let left = x_start.floor() as i64;
    let right = x_end.ceil() as i64 - 1;

    if left == right {
        let col = left as usize;
        let frac = x_end - x_start;
        row_acc[col] += per_sub_strip * frac;
    } else {
        let left_col = left as usize;
        let left_frac = (left + 1) as f64 - x_start;
        row_acc[left_col] += per_sub_strip * left_frac;
        for v in &mut row_acc[(left + 1) as usize..right as usize] {
            *v += per_sub_strip;
        }
        let right_col = right as usize;
        let right_frac = x_end - right as f64;
        row_acc[right_col] += per_sub_strip * right_frac;
    }
    Some((left as usize, right as usize + 1))
}

#[cfg(test)]
#[path = "tests_active_edges.rs"]
mod tests_active_edges;

#[cfg(test)]
mod scanline_tests {
    use super::*;
    use crate::contour::{ClosedRing, Polygon};
    use crate::geom2d::Point2D;
    use crate::profile::{Mirror, PixelResolution, PositiveFiniteMm};

    fn p(x: f64, y: f64) -> Point2D {
        Point2D::new(x, y)
    }

    fn mm(v: f64) -> PositiveFiniteMm {
        PositiveFiniteMm::new(v).unwrap()
    }

    /// 1 mm x 1 mm plate, 100 px x 100 px resolution. One pixel
    /// equals 1/100 mm.
    fn one_mm_per_hundred_pixels() -> PixelMap {
        PixelMap::new(
            mm(1.0),
            mm(1.0),
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
        let outer = ccw_square(p(-0.5, -0.5), p(0.5, 0.5));
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
        // CCW outer covering the whole [-0.5, 0.5]^2 mm plate, with
        // a CW hole over [-0.25, 0.25]^2 mm. On a 100x100 / 1x1 mm
        // build the hole covers integer pixel rows / cols 25..=74.
        let outer = ccw_square(p(-0.5, -0.5), p(0.5, 0.5));
        let hole = cw_square(p(-0.25, -0.25), p(0.25, 0.25));
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
        // Half-mm square centred on the build plate. With 1 mm
        // build and 100 px resolution this covers pixel rows /
        // cols 25..=74.
        let outer = ccw_square(p(-0.25, -0.25), p(0.25, 0.25));
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
    fn negative_quadrant_polygon_fills_the_lower_left_pixels() {
        // Printer-frame coordinates are centred on the plate origin:
        // a square at x, y in [-0.4, -0.1] mm on the 1 mm plate sits
        // in the plate's lower-left quarter and must render there
        // (pixel rows / cols 10..=39), not be clipped away.
        let outer = ccw_square(p(-0.4, -0.4), p(-0.1, -0.1));
        let polygon = Polygon::new(outer, vec![]).unwrap();
        let layer = ContourLayer::new(0.05, vec![polygon]);
        let raster = rasterize_layer_binary(&layer, &one_mm_per_hundred_pixels());

        // Inside the square.
        assert_eq!(pixel_value(&raster, 25, 25), 255);
        assert_eq!(pixel_value(&raster, 10, 10), 255);
        assert_eq!(pixel_value(&raster, 39, 39), 255);
        // Outside: below the square, the plate origin, the far corner.
        assert_eq!(pixel_value(&raster, 5, 5), 0);
        assert_eq!(pixel_value(&raster, 50, 50), 0);
        assert_eq!(pixel_value(&raster, 90, 90), 0);

        // Exactly the 30x30 covered pixels are filled.
        let filled: u64 = raster
            .runs()
            .iter()
            .filter(|r| r.value == 255)
            .map(|r| u64::from(r.length))
            .sum();
        assert_eq!(filled, 30 * 30);
    }

    #[test]
    fn rasterise_inherits_layer_z_and_pixel_map_dimensions() {
        let outer = ccw_square(p(-0.5, -0.5), p(0.5, 0.5));
        let polygon = Polygon::new(outer, vec![]).unwrap();
        let layer = ContourLayer::new(0.42, vec![polygon]);
        let pm = PixelMap::new(
            mm(1.0),
            mm(1.0),
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

    fn unit_diamond_layer() -> ContourLayer {
        // Unit square rotated 45 degrees about the plate origin.
        // Every edge crosses pixels at non-integer Y, so the AA pass
        // produces gradient pixels along every edge.
        let outer = ClosedRing::new(vec![p(0.0, -0.5), p(0.5, 0.0), p(0.0, 0.5), p(-0.5, 0.0)]);
        let polygon = Polygon::new(outer, vec![]).unwrap();
        ContourLayer::new(0.05, vec![polygon])
    }

    #[test]
    fn aa_off_matches_the_binary_path_bitwise() {
        let outer = ccw_square(p(-0.5, -0.5), p(0.5, 0.5));
        let hole = cw_square(p(-0.25, -0.25), p(0.25, 0.25));
        let polygon = Polygon::new(outer, vec![hole]).unwrap();
        let layer = ContourLayer::new(0.05, vec![polygon]);
        let pm = one_mm_per_hundred_pixels();
        let binary = rasterize_layer_binary(&layer, &pm);
        let aa_off = rasterize_layer(&layer, &pm, AaLevel::Off, 0.0);
        assert_eq!(aa_off, binary);
    }

    #[test]
    fn aa_produces_intermediate_grayscale_values_on_diagonal_edges() {
        let layer = unit_diamond_layer();
        let raster = rasterize_layer(&layer, &one_mm_per_hundred_pixels(), AaLevel::X4, 0.0);
        let has_intermediate = raster.runs().iter().any(|r| r.value > 0 && r.value < 255);
        assert!(
            has_intermediate,
            "AA on a diamond must produce some grayscale (0 < value < 255) pixels",
        );
    }

    #[test]
    fn aa_axis_aligned_unit_square_is_uniformly_filled() {
        // Pixel edges line up with the polygon's mm edges, so even
        // with AA on every pixel ends up at exactly 100% coverage.
        let outer = ccw_square(p(-0.5, -0.5), p(0.5, 0.5));
        let polygon = Polygon::new(outer, vec![]).unwrap();
        let layer = ContourLayer::new(0.05, vec![polygon]);
        let raster = rasterize_layer(&layer, &one_mm_per_hundred_pixels(), AaLevel::X4, 0.0);
        for run in raster.runs() {
            assert_eq!(run.value, 255);
        }
        let total: u64 = raster.runs().iter().map(|r| u64::from(r.length)).sum();
        assert_eq!(total, 100 * 100);
    }

    #[test]
    fn aa_output_is_deterministic_across_invocations() {
        let layer = unit_diamond_layer();
        let pm = one_mm_per_hundred_pixels();
        let first = rasterize_layer(&layer, &pm, AaLevel::X8, 0.0);
        let second = rasterize_layer(&layer, &pm, AaLevel::X8, 0.0);
        assert_eq!(first, second);
    }

    #[test]
    fn aa_min_alpha_threshold_clamps_sub_threshold_pixels_to_zero() {
        let layer = unit_diamond_layer();
        let pm = one_mm_per_hundred_pixels();
        // No threshold: gradient pixels appear in the (0, 128) range.
        let no_floor = rasterize_layer(&layer, &pm, AaLevel::X8, 0.0);
        let any_sub_half = no_floor.runs().iter().any(|r| r.value > 0 && r.value < 128);
        assert!(
            any_sub_half,
            "AA without floor must surface at least one sub-50% pixel on a diamond",
        );

        // 50% floor: every pixel is either 0 or at least round(50/100 * 255) = 128.
        let with_floor = rasterize_layer(&layer, &pm, AaLevel::X8, 50.0);
        for run in with_floor.runs() {
            assert!(
                run.value == 0 || run.value >= 128,
                "pixel value {} fell into the clamped band",
                run.value,
            );
        }
    }

    /// A unit square whose first vertex has a NaN X coordinate but a
    /// finite Y: the edge into that vertex still straddles sample
    /// rows, so its crossing interpolates to a NaN X and lands in the
    /// per-row sort. `ClosedRing` accepts the point verbatim (the
    /// pre-slice gate is a different layer), so the raster tier must
    /// stay panic-free on its own.
    fn nan_x_layer() -> ContourLayer {
        let ring = ClosedRing::new(vec![
            p(f64::NAN, -0.5),
            p(0.5, -0.5),
            p(0.5, 0.5),
            p(-0.5, 0.5),
        ]);
        ContourLayer::new(0.05, vec![Polygon::new_unchecked(ring, vec![])])
    }

    #[test]
    fn nan_x_contour_does_not_panic_in_the_binary_fill() {
        let raster = rasterize_layer_binary(&nan_x_layer(), &one_mm_per_hundred_pixels());
        let total: u64 = raster.runs().iter().map(|r| u64::from(r.length)).sum();
        assert_eq!(total, 100 * 100, "the raster must still cover every pixel");
    }

    #[test]
    fn nan_x_contour_does_not_panic_in_the_aa_fill() {
        let raster = rasterize_layer(
            &nan_x_layer(),
            &one_mm_per_hundred_pixels(),
            AaLevel::X4,
            0.0,
        );
        let total: u64 = raster.runs().iter().map(|r| u64::from(r.length)).sum();
        assert_eq!(total, 100 * 100, "the raster must still cover every pixel");
    }

    #[test]
    fn nan_x_contour_output_is_deterministic_across_invocations() {
        let layer = nan_x_layer();
        let pm = one_mm_per_hundred_pixels();
        assert_eq!(
            rasterize_layer_binary(&layer, &pm),
            rasterize_layer_binary(&layer, &pm),
        );
        assert_eq!(
            rasterize_layer(&layer, &pm, AaLevel::X4, 0.0),
            rasterize_layer(&layer, &pm, AaLevel::X4, 0.0),
        );
    }
}
