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

use sc_sl1cer::contour::{ClosedRing, ContourLayer, ContourStack, Polygon};
use sc_sl1cer::geom2d::Point2D;
use sc_sl1cer::profile::LayerSchedule;

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
