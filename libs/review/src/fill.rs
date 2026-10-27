/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Filling a cavity (#132): its exact pixels, kept in plate millimetres, and
//! the contour stack that cures them solid on a later slice.
//!
//! The resin-trap check offers [`SuggestedFix::Fill`](crate::SuggestedFix)
//! for cavities too small to drain. Its [`CavityFill`] holds the pixels the
//! review saw, layer by layer, as rectangles in plate millimetres with the
//! slab each layer cures. Millimetres, not pixels, so the fill outlives the
//! slice it was found on: a frontend keeps it, slices the same scene again,
//! and adds [`fill_stack`] to the plate as one more model leaf, before the
//! review and before the leaves are merged for export and preview. The
//! review then sees the cavity solid, and every later raster (the printer
//! file, the layer preview, the estimates read from it) cures it too.
//!
//! The rectangles follow the pixel edges exactly, so on the same screen they
//! light exactly the pixels the review found. On a slice with another layer
//! schedule, each layer takes the slab its cutting height falls in.
//!
//! Fix All (#156) uses the same pixels for two more fixes:
//!
//! - **Thickening** a feature thinner than a pixel: the thin-feature check
//!   offers [`SuggestedFix::Thicken`](crate::SuggestedFix) with the
//!   feature's pixels grown a pixel on every side, cured on the next slice
//!   exactly as a cavity's fill is.
//! - **Erasing** a floating speck: the output check offers
//!   [`SuggestedFix::Erase`](crate::SuggestedFix) with the speck's pixels
//!   grown a pixel on every side. [`erase_stack`] turns them into a stack
//!   that, merged with the stacks it was cut from, removes everything inside
//!   those rectangles. The slicer fills by the non-zero winding rule, so a
//!   ring clipped to a rectangle and reversed cancels the ring there exactly
//!   and changes nothing elsewhere: no polygon boolean is needed, and the
//!   model meshes are never touched. The pixels around a speck are air on
//!   its layer (solid is 8-connected), so the margin only takes the parts of
//!   its polygons too thin to light a pixel, which anti-aliasing would still
//!   grey.

use sc_sl1cer::contour::{ClosedRing, ContourLayer, ContourStack, Polygon};
use sc_sl1cer::geom2d::Point2D;
use sc_sl1cer::profile::LayerSchedule;

use crate::checks::thin::dilate_square;
use crate::input::ReviewInput;
use crate::runs::Run;

/// A rectangle on the plate, in printer-frame millimetres
/// (`x0_mm < x1_mm`, `y0_mm < y1_mm`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlateRect {
    /// Left edge.
    pub x0_mm: f64,
    /// Front edge.
    pub y0_mm: f64,
    /// Right edge.
    pub x1_mm: f64,
    /// Back edge.
    pub y1_mm: f64,
}

impl PlateRect {
    /// The rectangle's area in square millimetres.
    #[inline]
    pub fn area_mm2(&self) -> f64 {
        (self.x1_mm - self.x0_mm) * (self.y1_mm - self.y0_mm)
    }

    /// Whether the two rectangles share some area.
    pub fn overlaps(&self, other: &Self) -> bool {
        self.x0_mm < other.x1_mm
            && other.x0_mm < self.x1_mm
            && self.y0_mm < other.y1_mm
            && other.y0_mm < self.y1_mm
    }

    /// The rectangle as a counter-clockwise polygon.
    fn polygon(&self) -> Polygon {
        let ring = ClosedRing::new(vec![
            Point2D::new(self.x0_mm, self.y0_mm),
            Point2D::new(self.x1_mm, self.y0_mm),
            Point2D::new(self.x1_mm, self.y1_mm),
            Point2D::new(self.x0_mm, self.y1_mm),
        ]);
        Polygon::new_unchecked(ring, Vec::new())
    }
}

/// What one layer of a cavity fills: the slab it cures and its pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct FillSlab {
    /// Bottom of the layer's slab, in millimetres above the plate.
    pub bottom_mm: f64,
    /// Top of the layer's slab.
    pub top_mm: f64,
    /// The cavity's pixels on this layer, as disjoint rectangles.
    pub rects: Vec<PlateRect>,
}

impl FillSlab {
    /// Whether this slab is the one cut at height `z_mm`.
    fn holds(&self, z_mm: f64) -> bool {
        self.bottom_mm <= z_mm && z_mm < self.top_mm
    }
}

/// The pixels of one cavity, to cure solid on a later slice.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CavityFill {
    /// Its layers, from the plate up.
    pub slabs: Vec<FillSlab>,
}

impl CavityFill {
    /// The fill of the run lists `layers` (layer index, its runs) of
    /// `input`'s raster.
    pub(crate) fn from_runs(input: &ReviewInput<'_>, layers: Vec<(usize, Vec<Run>)>) -> Self {
        let slabs = layers
            .into_iter()
            .map(|(layer, runs)| {
                let span = input.layers()[layer];
                FillSlab {
                    bottom_mm: span.bottom_mm,
                    top_mm: span.top_mm,
                    rects: pixel_rects(&runs)
                        .into_iter()
                        .map(|[x0, y0, x1, y1]| {
                            let a = input.pixel_to_plate([f64::from(x0), f64::from(y0)]);
                            let b = input.pixel_to_plate([f64::from(x1), f64::from(y1)]);
                            PlateRect {
                                x0_mm: a[0].min(b[0]),
                                y0_mm: a[1].min(b[1]),
                                x1_mm: a[0].max(b[0]),
                                y1_mm: a[1].max(b[1]),
                            }
                        })
                        .collect(),
                }
            })
            .collect();
        Self { slabs }
    }

    /// The fill of the run lists `layers`, each grown `margin_px` pixels on
    /// every side (a square), within the screen: a feature to thicken or a
    /// speck to erase with its margin.
    pub(crate) fn grown_from_runs(
        input: &ReviewInput<'_>,
        layers: Vec<(usize, Vec<Run>)>,
        margin_px: u32,
    ) -> Self {
        let (width, height) = (input.width_px(), input.height_px());
        let grown = layers
            .into_iter()
            .map(|(layer, runs)| (layer, dilate_square(&runs, margin_px, width, height)))
            .collect();
        Self::from_runs(input, grown)
    }

    /// The resin it cures, in cubic millimetres.
    pub fn volume_mm3(&self) -> f64 {
        self.slabs
            .iter()
            .map(|slab| {
                let area: f64 = slab.rects.iter().map(PlateRect::area_mm2).sum();
                area * (slab.top_mm - slab.bottom_mm)
            })
            .sum()
    }

    /// Whether it fills nothing.
    pub fn is_empty(&self) -> bool {
        self.slabs.iter().all(|slab| slab.rects.is_empty())
    }

    /// Whether a rectangle of it overlaps one of `other` on overlapping
    /// slabs.
    pub fn overlaps(&self, other: &Self) -> bool {
        self.slabs.iter().any(|a| {
            other
                .slabs
                .iter()
                .filter(|b| a.bottom_mm < b.top_mm && b.bottom_mm < a.top_mm)
                .any(|b| {
                    a.rects
                        .iter()
                        .any(|r| b.rects.iter().any(|q| r.overlaps(q)))
                })
        })
    }
}

/// The contour stack that cures `fills` on a plate cut with `schedule`: one
/// layer per scheduled layer, at the schedule's cutting heights, so it
/// merges with the plate's own stacks and reviews with them. Each layer
/// takes the slabs its cutting height falls in. `None` when nothing is
/// filled on any layer.
pub fn fill_stack<'f>(
    fills: impl IntoIterator<Item = &'f CavityFill>,
    schedule: &LayerSchedule,
) -> Option<ContourStack> {
    let slabs: Vec<&FillSlab> = fills
        .into_iter()
        .flat_map(|fill| &fill.slabs)
        .filter(|slab| !slab.rects.is_empty())
        .collect();
    if slabs.is_empty() {
        return None;
    }
    let mut lit = false;
    let layers = schedule
        .iter_z()
        .map(|z_mm| {
            let polygons: Vec<Polygon> = slabs
                .iter()
                .filter(|slab| slab.holds(z_mm))
                .flat_map(|slab| slab.rects.iter().map(PlateRect::polygon))
                .collect();
            lit |= !polygons.is_empty();
            ContourLayer::new(z_mm, polygons)
        })
        .collect();
    lit.then(|| ContourStack::new_unchecked(layers))
}

/// The contour stack that erases `erasures` from `stacks`, all cut with
/// `schedule`: merged with them, nothing of them is left inside the
/// erasures' rectangles, and nothing changes outside them. Each layer holds,
/// for every ring of `stacks` on that layer and every rectangle of the
/// erasures' slab there, the ring clipped to the rectangle and reversed.
///
/// The stack cancels only the stacks it is cut from: erase each role with
/// its own stack, since the review rasterises the roles apart. The
/// rectangles must be disjoint on each layer, as those of one review are,
/// or a ring would be cancelled twice. `None` when nothing is erased on any
/// layer.
pub fn erase_stack<'e>(
    erasures: impl IntoIterator<Item = &'e CavityFill>,
    stacks: &[&ContourStack],
    schedule: &LayerSchedule,
) -> Option<ContourStack> {
    let slabs: Vec<&FillSlab> = erasures
        .into_iter()
        .flat_map(|erasure| &erasure.slabs)
        .filter(|slab| !slab.rects.is_empty())
        .collect();
    if slabs.is_empty() {
        return None;
    }
    let mut cut = false;
    let layers = schedule
        .iter_z()
        .enumerate()
        .map(|(layer, z_mm)| {
            let rects: Vec<&PlateRect> = slabs
                .iter()
                .filter(|slab| slab.holds(z_mm))
                .flat_map(|slab| &slab.rects)
                .collect();
            let mut polygons = Vec::new();
            if !rects.is_empty() {
                let rings = stacks
                    .iter()
                    .filter_map(|stack| stack.layers().get(layer))
                    .flat_map(ContourLayer::polygons)
                    .flat_map(|polygon| std::iter::once(polygon.outer()).chain(polygon.holes()));
                for ring in rings {
                    let Some(bounds) = ring_bounds(ring) else {
                        continue;
                    };
                    for rect in rects.iter().filter(|rect| rect.overlaps(&bounds)) {
                        if let Some(mut clipped) = clip_to_rect(ring, rect) {
                            clipped.reverse();
                            polygons.push(Polygon::new_unchecked(clipped, Vec::new()));
                        }
                    }
                }
            }
            cut |= !polygons.is_empty();
            ContourLayer::new(z_mm, polygons)
        })
        .collect();
    cut.then(|| ContourStack::new_unchecked(layers))
}

/// The bounding box of a ring, `None` for an empty one.
fn ring_bounds(ring: &ClosedRing) -> Option<PlateRect> {
    let first = ring.vertices().first()?;
    let mut bounds = PlateRect {
        x0_mm: first.x,
        y0_mm: first.y,
        x1_mm: first.x,
        y1_mm: first.y,
    };
    for p in ring.vertices() {
        bounds.x0_mm = bounds.x0_mm.min(p.x);
        bounds.y0_mm = bounds.y0_mm.min(p.y);
        bounds.x1_mm = bounds.x1_mm.max(p.x);
        bounds.y1_mm = bounds.y1_mm.max(p.y);
    }
    Some(bounds)
}

/// `ring` clipped to `rect` (Sutherland–Hodgman), in the ring's direction.
/// For a concave ring the result may run back and forth along the
/// rectangle's edges; those stretches enclose no area, so its winding
/// number is the ring's at every point inside the rectangle and 0 outside.
/// `None` when nothing with area is left.
fn clip_to_rect(ring: &ClosedRing, rect: &PlateRect) -> Option<ClosedRing> {
    // Each side: the coordinate (0 = x, 1 = y), its limit, and whether the
    // inside lies above the limit.
    let sides = [
        (0, rect.x0_mm, true),
        (0, rect.x1_mm, false),
        (1, rect.y0_mm, true),
        (1, rect.y1_mm, false),
    ];
    let mut points: Vec<Point2D> = ring.vertices().to_vec();
    let mut next = Vec::with_capacity(points.len() + 4);
    for (axis, limit, above) in sides {
        let &last = points.last()?;
        let inside = |p: Point2D| {
            let v = if axis == 0 { p.x } else { p.y };
            if above { v >= limit } else { v <= limit }
        };
        next.clear();
        let mut previous = last;
        for &current in &points {
            match (inside(previous), inside(current)) {
                (true, true) => next.push(current),
                (true, false) => next.push(crossing(previous, current, axis, limit)),
                (false, true) => {
                    next.push(crossing(previous, current, axis, limit));
                    next.push(current);
                }
                (false, false) => {}
            }
            previous = current;
        }
        std::mem::swap(&mut points, &mut next);
    }
    let clipped = ClosedRing::new(points);
    (clipped.len() >= 3 && clipped.signed_area() != 0.0).then_some(clipped)
}

/// Where the segment from `a` to `b` crosses the line where coordinate
/// `axis` (0 = x, 1 = y) is `limit`; its ends lie on either side of it.
fn crossing(a: Point2D, b: Point2D, axis: usize, limit: f64) -> Point2D {
    if axis == 0 {
        let t = (limit - a.x) / (b.x - a.x);
        Point2D::new(limit, a.y + t * (b.y - a.y))
    } else {
        let t = (limit - a.y) / (b.y - a.y);
        Point2D::new(a.x + t * (b.x - a.x), limit)
    }
}

/// The pixels of a run list as disjoint rectangles `[x0, y0, x1, y1]` in
/// raster space: runs that repeat the same span on consecutive rows join.
fn pixel_rects(runs: &[Run]) -> Vec<[u32; 4]> {
    let mut done: Vec<[u32; 4]> = Vec::new();
    // Rectangles still open on the previous row, by span, in row order.
    let mut open: Vec<[u32; 4]> = Vec::new();
    let mut next: Vec<[u32; 4]> = Vec::new();
    for row_runs in runs.chunk_by(|a, b| a.row == b.row) {
        let row = row_runs[0].row;
        next.clear();
        let mut kept = 0;
        for run in row_runs {
            // Both lists are sorted by x0; skip what this row does not
            // continue.
            while kept < open.len() && open[kept][0] < run.x0 {
                done.push(open[kept]);
                kept += 1;
            }
            match open.get(kept) {
                Some(rect) if rect[0] == run.x0 && rect[2] == run.x1 && rect[3] == row => {
                    next.push([rect[0], rect[1], rect[2], row + 1]);
                    kept += 1;
                }
                _ => next.push([run.x0, row, run.x1, row + 1]),
            }
        }
        done.extend_from_slice(&open[kept..]);
        std::mem::swap(&mut open, &mut next);
    }
    done.extend_from_slice(&open);
    done.sort_unstable_by_key(|r| (r[1], r[0]));
    done
}

#[cfg(test)]
mod fill_tests {
    use super::*;

    #[test]
    fn repeated_spans_join_into_tall_rectangles() {
        let runs = [
            Run::new(2, 1, 4),
            Run::new(2, 6, 8),
            Run::new(3, 1, 4),
            Run::new(3, 6, 9),
            Run::new(4, 1, 4),
            Run::new(6, 1, 4),
        ];
        assert_eq!(
            pixel_rects(&runs),
            vec![[1, 2, 4, 5], [6, 2, 8, 3], [6, 3, 9, 4], [1, 6, 4, 7]]
        );
        let area: u32 = pixel_rects(&runs)
            .iter()
            .map(|r| (r[2] - r[0]) * (r[3] - r[1]))
            .sum();
        assert_eq!(area, 3 * 3 + 2 + 3 + 3);
    }

    /// The winding number of `rings` around `p`.
    fn winding(rings: &[&ClosedRing], p: Point2D) -> i32 {
        let mut total = 0;
        for ring in rings {
            let v = ring.vertices();
            for (i, &a) in v.iter().enumerate() {
                let b = v[(i + 1) % v.len()];
                let side = (b.x - a.x) * (p.y - a.y) - (p.x - a.x) * (b.y - a.y);
                if a.y <= p.y && b.y > p.y && side > 0.0 {
                    total += 1;
                } else if a.y > p.y && b.y <= p.y && side < 0.0 {
                    total -= 1;
                }
            }
        }
        total
    }

    fn rings(stack: &ContourStack, layer: usize) -> Vec<&ClosedRing> {
        stack.layers()[layer]
            .polygons()
            .iter()
            .flat_map(|p| std::iter::once(p.outer()).chain(p.holes()))
            .collect()
    }

    fn ring(points: &[[f64; 2]]) -> ClosedRing {
        ClosedRing::new(points.iter().map(|&[x, y]| Point2D::new(x, y)).collect())
    }

    fn slab(bottom_mm: f64, top_mm: f64, rects: Vec<PlateRect>) -> FillSlab {
        FillSlab {
            bottom_mm,
            top_mm,
            rects,
        }
    }

    fn rect(x0_mm: f64, y0_mm: f64, x1_mm: f64, y1_mm: f64) -> PlateRect {
        PlateRect {
            x0_mm,
            y0_mm,
            x1_mm,
            y1_mm,
        }
    }

    #[test]
    fn an_erasure_cancels_every_ring_inside_its_rects_and_nothing_else() {
        // A U-shaped polygon with a hole in its base, an overlapping square
        // (winding 2 where they meet), on layer 1 of 3; the erasure takes
        // two rectangles across the U's arms, its hole and the overlap.
        let schedule = LayerSchedule::uniform(0.05, 0.05, 3).unwrap();
        let u = Polygon::new_unchecked(
            ring(&[
                [0.0, 0.0],
                [6.0, 0.0],
                [6.0, 6.0],
                [4.0, 6.0],
                [4.0, 2.0],
                [2.0, 2.0],
                [2.0, 6.0],
                [0.0, 6.0],
            ]),
            vec![ring(&[[2.5, 0.5], [2.5, 1.5], [3.5, 1.5], [3.5, 0.5]])],
        );
        let square = Polygon::new_unchecked(
            ring(&[[5.0, 5.0], [7.0, 5.0], [7.0, 7.0], [5.0, 7.0]]),
            Vec::new(),
        );
        let layers = schedule
            .iter_z()
            .enumerate()
            .map(|(k, z)| {
                let polygons = if k == 1 {
                    vec![u.clone(), square.clone()]
                } else {
                    Vec::new()
                };
                ContourLayer::new(z, polygons)
            })
            .collect();
        let plate = ContourStack::new_unchecked(layers);
        let erasure = CavityFill {
            slabs: vec![slab(
                0.05,
                0.10,
                vec![rect(1.0, 1.0, 5.0, 4.0), rect(1.0, 4.0, 5.5, 5.5)],
            )],
        };
        let erased = erase_stack([&erasure], &[&plate], &schedule).expect("rings to cancel");
        assert!(rings(&erased, 0).is_empty() && rings(&erased, 2).is_empty());
        let before = rings(&plate, 1);
        let mut after = before.clone();
        after.extend(rings(&erased, 1));
        let inside = |x: f64, y: f64| {
            erasure.slabs[0]
                .rects
                .iter()
                .any(|r| r.x0_mm < x && x < r.x1_mm && r.y0_mm < y && y < r.y1_mm)
        };
        let mut cancelled = 0;
        for i in 0..80 {
            for j in 0..80 {
                let (x, y) = (-0.45 + 0.1 * f64::from(i), -0.45 + 0.1 * f64::from(j));
                let p = Point2D::new(x, y);
                let expected = if inside(x, y) { 0 } else { winding(&before, p) };
                cancelled += i32::from(inside(x, y) && winding(&before, p) != 0);
                assert_eq!(winding(&after, p), expected, "at ({x}, {y})");
            }
        }
        assert!(cancelled > 100, "the erasure covers solid: {cancelled}");
        // Nothing to erase from an empty plate or with nothing to erase.
        let empty = ContourStack::new_unchecked(
            schedule
                .iter_z()
                .map(|z| ContourLayer::new(z, Vec::new()))
                .collect(),
        );
        assert!(erase_stack([&erasure], &[&empty], &schedule).is_none());
        assert!(erase_stack([&CavityFill::default()], &[&plate], &schedule).is_none());
    }

    #[test]
    fn fills_overlap_only_on_shared_slabs() {
        let a = CavityFill {
            slabs: vec![slab(0.0, 0.05, vec![rect(0.0, 0.0, 1.0, 1.0)])],
        };
        let beside = CavityFill {
            slabs: vec![slab(0.0, 0.05, vec![rect(1.0, 0.0, 2.0, 1.0)])],
        };
        let above = CavityFill {
            slabs: vec![slab(0.05, 0.10, vec![rect(0.0, 0.0, 1.0, 1.0)])],
        };
        let across = CavityFill {
            slabs: vec![slab(0.0, 0.05, vec![rect(0.5, 0.5, 2.0, 2.0)])],
        };
        assert!(!a.overlaps(&beside));
        assert!(!a.overlaps(&above));
        assert!(a.overlaps(&across) && across.overlaps(&a));
    }

    #[test]
    fn a_rect_polygon_is_counter_clockwise() {
        let rect = PlateRect {
            x0_mm: -1.0,
            y0_mm: 2.0,
            x1_mm: 3.0,
            y1_mm: 2.5,
        };
        assert!(rect.polygon().outer().signed_area() > 0.0);
        assert_eq!(rect.area_mm2(), 2.0);
    }

    #[test]
    fn slabs_fill_the_layers_cut_inside_them() {
        let schedule = LayerSchedule::uniform(0.05, 0.05, 6).unwrap();
        let rect = PlateRect {
            x0_mm: 0.0,
            y0_mm: 0.0,
            x1_mm: 1.0,
            y1_mm: 1.0,
        };
        let fill = CavityFill {
            slabs: vec![
                FillSlab {
                    bottom_mm: 0.10,
                    top_mm: 0.15,
                    rects: vec![rect],
                },
                FillSlab {
                    bottom_mm: 0.15,
                    top_mm: 0.20,
                    rects: vec![rect, rect],
                },
            ],
        };
        assert!((fill.volume_mm3() - 0.15).abs() < 1e-12);
        let stack = fill_stack([&fill], &schedule).expect("a filled layer");
        let counts: Vec<usize> = stack.layers().iter().map(|l| l.polygons().len()).collect();
        assert_eq!(counts, [0, 0, 1, 2, 0, 0]);
        let zs: Vec<f64> = stack.layers().iter().map(ContourLayer::z_mm).collect();
        assert_eq!(zs, schedule.iter_z().collect::<Vec<_>>());
        assert!(fill_stack([&CavityFill::default()], &schedule).is_none());
        assert!(fill_stack(std::iter::empty(), &schedule).is_none());
    }
}
