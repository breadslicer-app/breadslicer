/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Support tips as data (#432): the only thing of the supports the review
//! reads.
//!
//! The review never rasterises support geometry. What the engine knows of a
//! tip (where it stands, its axis, its radii, how far it goes into the
//! model) comes in as [`TipCone`]s, and a tip's pixels are worked out for
//! the few parts that ask ([`TipIndex::contact_px`]): a pixel of a layer
//! belongs to a tip when its centre, at the height the layer was cut at,
//! lies inside the cone. That is what the printer file lights there, without
//! slicing a triangle of it.

use sc_sl1cer::geom2d::Point2D;

use crate::input::ReviewInput;
use crate::runs::Run;

/// One stretch of a support's tip, in printer-frame millimetres: a cone cut
/// flat at both ends, from one circle to another. A tip with a break-point
/// neck is two: the neck and the cone above it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TipCone {
    /// The centre of one end, the one that goes into the model.
    pub tip_mm: [f64; 3],
    /// Its radius.
    pub tip_radius_mm: f64,
    /// The centre of the other end.
    pub base_mm: [f64; 3],
    /// Its radius.
    pub base_radius_mm: f64,
}

impl TipCone {
    /// The stretches of a tip that ends at `tip_mm`, inside the model, and
    /// meets its shaft at `socket_mm`, with a neck of its tip's radius up to
    /// `neck_mm` when it has one.
    pub fn stretches(
        tip_mm: [f64; 3],
        tip_radius_mm: f64,
        neck_mm: Option<[f64; 3]>,
        socket_mm: [f64; 3],
        socket_radius_mm: f64,
    ) -> impl Iterator<Item = Self> {
        let cone = |from: [f64; 3], base_mm: [f64; 3], base_radius_mm: f64| Self {
            tip_mm: from,
            tip_radius_mm,
            base_mm,
            base_radius_mm,
        };
        let neck = neck_mm.map(|neck| cone(tip_mm, neck, tip_radius_mm));
        let body = cone(neck_mm.unwrap_or(tip_mm), socket_mm, socket_radius_mm);
        neck.into_iter().chain(std::iter::once(body))
    }
}

/// A cone, ready to be asked about points.
#[derive(Clone, Copy, Debug)]
struct Placed {
    tip: [f64; 3],
    /// Unit axis from the tip to the base.
    axis: [f64; 3],
    length: f64,
    tip_radius: f64,
    /// How much the radius grows per millimetre of axis.
    slope: f64,
    widest: f64,
    /// The heights it spans.
    low: f64,
    high: f64,
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0].mul_add(b[0], a[1].mul_add(b[1], a[2] * b[2]))
}

impl Placed {
    fn new(cone: &TipCone) -> Option<Self> {
        let d = [0, 1, 2].map(|i| cone.base_mm[i] - cone.tip_mm[i]);
        let length = dot(d, d).sqrt();
        let ok = length.is_finite()
            && length > 1e-9
            && cone.tip_radius_mm.is_finite()
            && cone.base_radius_mm.is_finite();
        if !ok {
            return None;
        }
        let axis = d.map(|v| v / length);
        let widest = cone.tip_radius_mm.max(cone.base_radius_mm);
        // The ends are circles square to the axis: their rims reach
        // `radius * sin(tilt)` above and below their centres.
        let lean = (1.0 - axis[2] * axis[2]).max(0.0).sqrt();
        let reach = |z: f64, radius: f64| (z - radius * lean, z + radius * lean);
        let (a_low, a_high) = reach(cone.tip_mm[2], cone.tip_radius_mm);
        let (b_low, b_high) = reach(cone.base_mm[2], cone.base_radius_mm);
        Some(Self {
            tip: cone.tip_mm,
            axis,
            length,
            tip_radius: cone.tip_radius_mm,
            slope: (cone.base_radius_mm - cone.tip_radius_mm) / length,
            widest,
            low: a_low.min(b_low),
            high: a_high.max(b_high),
        })
    }

    fn holds(&self, point: [f64; 3]) -> bool {
        let d = [0, 1, 2].map(|i| point[i] - self.tip[i]);
        let along = dot(d, self.axis);
        if along < 0.0 || along > self.length {
            return false;
        }
        let radius = self.slope.mul_add(along, self.tip_radius);
        dot(d, d) - along * along <= radius * radius
    }

    /// The part of the plate the cone can cover at height `z_mm`, as
    /// `[min x, min y, max x, max y]` in millimetres.
    fn bounds_at(&self, z_mm: f64) -> [f64; 4] {
        // The stretch of the axis within `widest` of that height.
        let lean = (1.0 - self.axis[2] * self.axis[2]).max(0.0).sqrt();
        let (mut from, mut to) = (0.0, self.length);
        if self.axis[2].abs() > 1e-9 {
            let at = (z_mm - self.tip[2]) / self.axis[2];
            let spread = (self.widest * lean / self.axis[2]).abs();
            from = (at - spread).clamp(0.0, self.length);
            to = (at + spread).clamp(0.0, self.length);
        }
        let point = |along: f64| {
            [
                self.axis[0].mul_add(along, self.tip[0]),
                self.axis[1].mul_add(along, self.tip[1]),
            ]
        };
        let (a, b) = (point(from), point(to));
        [
            a[0].min(b[0]) - self.widest,
            a[1].min(b[1]) - self.widest,
            a[0].max(b[0]) + self.widest,
            a[1].max(b[1]) + self.widest,
        ]
    }
}

/// The tips of a plate by height.
#[derive(Clone, Debug, Default)]
pub struct TipIndex {
    /// The cones, lowest first.
    cones: Vec<Placed>,
    /// The tallest cone's height span.
    tallest: f64,
}

impl TipIndex {
    /// Indexes `cones`. A cone with no length, or one that is not a number,
    /// is no cone.
    pub fn new(cones: &[TipCone]) -> Self {
        let mut cones: Vec<Placed> = cones.iter().filter_map(Placed::new).collect();
        cones.sort_by(|a, b| a.low.total_cmp(&b.low));
        let tallest = cones.iter().map(|c| c.high - c.low).fold(0.0, f64::max);
        Self { cones, tallest }
    }

    /// The cones that span height `z_mm`.
    fn at(&self, z_mm: f64) -> impl Iterator<Item = &Placed> + '_ {
        let from = self.cones.partition_point(|c| c.low < z_mm - self.tallest);
        self.cones[from..]
            .iter()
            .take_while(move |c| c.low <= z_mm)
            .filter(move |c| c.high >= z_mm)
    }

    /// The pixels of the tips on layer `below` that a part starting right
    /// above it touches: those within `reach_px` (columns, rows) of `runs`,
    /// the part's pixels, and not among `solid_below`, what the model has on
    /// that layer. Both lists are sorted by row. A pixel two tips share
    /// counts once.
    pub fn contact_px(
        &self,
        input: &ReviewInput<'_>,
        below: usize,
        runs: &[Run],
        solid_below: &[Run],
        reach_px: [u32; 2],
    ) -> usize {
        let z_mm = input.layers()[below].sample_mm;
        let mut touched: Vec<(u32, u32)> = Vec::new();
        for cone in self.at(z_mm) {
            for_each_pixel(input, cone, z_mm, |x, y| {
                if near(runs, x, y, reach_px) && !lit(solid_below, x, y) {
                    touched.push((y, x));
                }
            });
        }
        touched.sort_unstable();
        touched.dedup();
        touched.len()
    }
}

/// Whether pixel `(x, y)` is in the run list `runs`, sorted by row.
fn lit(runs: &[Run], x: u32, y: u32) -> bool {
    let at = runs.partition_point(|run| (run.row, run.x1) <= (y, x));
    runs.get(at)
        .is_some_and(|run| run.row == y && run.x0 <= x && x < run.x1)
}

/// Whether a pixel of `runs`, sorted by row, lies within `reach_px` of
/// `(x, y)`, along each axis.
fn near(runs: &[Run], x: u32, y: u32, reach_px: [u32; 2]) -> bool {
    let (left, right) = (x.saturating_sub(reach_px[0]), x.saturating_add(reach_px[0]));
    let (top, bottom) = (y.saturating_sub(reach_px[1]), y.saturating_add(reach_px[1]));
    let first = runs.partition_point(|run| run.row < top);
    runs[first..]
        .iter()
        .take_while(|run| run.row <= bottom)
        .any(|run| run.x0 <= right && left < run.x1)
}

/// Calls `visit` with every pixel whose centre lies inside `cone` at height
/// `z_mm`.
fn for_each_pixel(
    input: &ReviewInput<'_>,
    cone: &Placed,
    z_mm: f64,
    mut visit: impl FnMut(u32, u32),
) {
    let [x0, y0, x1, y1] = cone.bounds_at(z_mm);
    let map = input.pixel_map();
    let a = map.mm_to_pixel(Point2D::new(x0, y0));
    let b = map.mm_to_pixel(Point2D::new(x1, y1));
    // Screen mirrors swap the corners.
    let width = f64::from(input.width_px());
    let height = f64::from(input.height_px());
    let (left, right) = (
        (a.0.min(b.0).floor() - 1.0).clamp(0.0, width),
        (a.0.max(b.0).ceil() + 1.0).clamp(0.0, width),
    );
    let (top, bottom) = (
        (a.1.min(b.1).floor() - 1.0).clamp(0.0, height),
        (a.1.max(b.1).ceil() + 1.0).clamp(0.0, height),
    );
    for y in top as u32..bottom as u32 {
        for x in left as u32..right as u32 {
            let [px, py] = input.pixel_to_plate([f64::from(x) + 0.5, f64::from(y) + 0.5]);
            if cone.holds([px, py, z_mm]) {
                visit(x, y);
            }
        }
    }
}

#[cfg(test)]
mod tips_tests {
    use super::*;

    fn upright(radius: f64) -> TipCone {
        TipCone {
            tip_mm: [1.0, 2.0, 5.0],
            tip_radius_mm: radius,
            base_mm: [1.0, 2.0, 2.0],
            base_radius_mm: 3.0 * radius,
        }
    }

    #[test]
    fn a_point_is_inside_within_the_radius_of_its_height() {
        let cone = Placed::new(&upright(0.1)).expect("a cone");
        assert!(cone.holds([1.0, 2.0, 3.0]));
        assert!(cone.holds([1.09, 2.0, 5.0]));
        assert!(!cone.holds([1.11, 2.0, 5.0]));
        assert!(cone.holds([1.29, 2.0, 2.0]));
        assert!(!cone.holds([1.0, 2.0, 5.1]));
        assert!(!cone.holds([1.0, 2.0, 1.9]));
    }

    #[test]
    fn a_leaning_cone_shifts_with_its_height() {
        let cone = Placed::new(&TipCone {
            tip_mm: [0.0, 0.0, 4.0],
            tip_radius_mm: 0.1,
            base_mm: [2.0, 0.0, 2.0],
            base_radius_mm: 0.1,
        })
        .expect("a cone");
        assert!(cone.holds([1.0, 0.0, 3.0]));
        assert!(!cone.holds([0.0, 0.0, 3.0]));
        let [x0, _, x1, _] = cone.bounds_at(3.0);
        assert!(x0 <= 0.9 && x1 >= 1.1, "{x0} {x1}");
    }

    #[test]
    fn a_cone_with_no_length_is_no_cone() {
        let cone = TipCone {
            tip_mm: [1.0, 1.0, 1.0],
            tip_radius_mm: 0.1,
            base_mm: [1.0, 1.0, 1.0],
            base_radius_mm: 0.2,
        };
        assert!(Placed::new(&cone).is_none());
    }

    #[test]
    fn the_index_finds_the_cones_at_a_height() {
        let low = upright(0.1);
        let mut high = upright(0.1);
        high.tip_mm[2] = 15.0;
        high.base_mm[2] = 12.0;
        let index = TipIndex::new(&[high, low]);
        assert_eq!(index.at(3.0).count(), 1);
        assert_eq!(index.at(13.0).count(), 1);
        assert_eq!(index.at(8.0).count(), 0);
    }
}
