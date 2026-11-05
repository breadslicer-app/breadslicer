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
//! the few parts that ask ([`TipIndex::contact`]): a pixel of a layer
//! belongs to a tip when its centre, at the height the layer was cut at,
//! lies inside the cone. That is what the printer file lights there, without
//! slicing a triangle of it.

use sc_sl1cer::geom2d::Point2D;
use sc_sl1cer::raster::pixel_map::PixelMap;

use crate::input::{ReviewInput, pixel_to_plate};
use crate::regions::PixelBox;
use crate::runs::Run;
use crate::tip_plan::IslandPixels;

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

/// The screen a tip's pixels are read on: all of a slice that deciding
/// whether a tip holds a part needs, once the slice's input is gone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TipGrid {
    pixel_map: PixelMap,
    pixel_area_mm2: f64,
}

impl TipGrid {
    /// The screen of `input`.
    pub fn of(input: &ReviewInput<'_>) -> Self {
        Self {
            pixel_map: *input.pixel_map(),
            pixel_area_mm2: input.pixel_area_mm2(),
        }
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

    /// The ends of the tips that stand inside `pixels`, in plate millimetres.
    /// Ends closer than `apart_mm` are one tip: one with a neck is two cones.
    pub fn standing_in(
        &self,
        input: &ReviewInput<'_>,
        pixels: &IslandPixels,
        apart_mm: f64,
    ) -> Vec<[f64; 3]> {
        let mut found: Vec<[f64; 3]> = Vec::new();
        for end in self.cones.iter().map(|cone| cone.tip) {
            let Some(layer) = pixels
                .layers
                .iter()
                .rev()
                .find(|layer| layer.bottom_mm <= end[2])
            else {
                continue;
            };
            let (x, y) = input.pixel_map().mm_to_pixel(Point2D::new(end[0], end[1]));
            if x < 0.0 || y < 0.0 || !lit(&layer.runs, x as u32, y as u32) {
                continue;
            }
            let twin = found
                .iter()
                .any(|other| (other[0] - end[0]).hypot(other[1] - end[1]) < apart_mm);
            if !twin {
                found.push(end);
            }
        }
        found
    }

    /// The cones that span height `z_mm`.
    fn at(&self, z_mm: f64) -> impl Iterator<Item = &Placed> + '_ {
        let from = self.cones.partition_point(|c| c.low < z_mm - self.tallest);
        self.cones[from..]
            .iter()
            .take_while(move |c| c.low <= z_mm)
            .filter(move |c| c.high >= z_mm)
    }

    /// How the tips at height `z_mm`, that of the layer below a part, touch
    /// the part starting right above it: `start` is its first pixels and what
    /// the model has on that layer, and `reach_px` (columns, rows) how far
    /// from the part a tip's pixel still touches it. Pixels of the model do
    /// not count: a tip inside the model holds the model there, not this
    /// part.
    ///
    /// This is the one decision of whether a part is held: the review asks
    /// it of a plate's tips, and a tip being placed asks it of itself
    /// ([`crate::IslandStart::held_by`]).
    pub fn contact(
        &self,
        grid: &TipGrid,
        z_mm: f64,
        start: &TipTarget<'_>,
        reach_px: [u32; 2],
        share: f64,
    ) -> TipContact {
        let pixel_mm2 = grid.pixel_area_mm2;
        let reach = [f64::from(reach_px[0]), f64::from(reach_px[1])];
        let mut touched: Vec<(u32, u32)> = Vec::new();
        let mut reaches_share = false;
        for cone in self.at(z_mm) {
            let from = touched.len();
            for_each_pixel(grid, cone, z_mm, start.bbox, reach, |x, y| {
                if near(start.runs, x, y, reach_px) && !lit(start.below, x, y) {
                    touched.push((y, x));
                }
            });
            let section_px = std::f64::consts::PI * cone.tip_radius * cone.tip_radius / pixel_mm2;
            reaches_share |= (touched.len() - from) as f64 >= share * section_px;
        }
        touched.sort_unstable();
        touched.dedup();
        let under = touched
            .iter()
            .filter(|&&(y, x)| lit(start.runs, x, y))
            .count() as u64;
        TipContact {
            px: touched.len(),
            holds: !touched.is_empty() && (reaches_share || 2 * under >= start.area_px),
        }
    }
}

/// How close two points are to be the same joint of a tip, in millimetres:
/// their own rounding, nothing a real distance could reach.
const SAME_JOINT_MM: f64 = 1e-6;

/// The ends of a stretch of a tip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TipStretch {
    /// The end that goes into the model.
    pub tip_mm: [f64; 3],
    /// The other end.
    pub base_mm: [f64; 3],
}

impl TipStretch {
    /// Whether `other` is this stretch, or the rest of the same tip: a tip
    /// with a neck is two cones, one going on where the other ends. Two tips
    /// may end a hair apart inside the model and still be two tips.
    fn same_tip(&self, other: &Self) -> bool {
        let meet = |a: [f64; 3], b: [f64; 3]| {
            (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt() < SAME_JOINT_MM
        };
        meet(self.tip_mm, other.tip_mm)
            || meet(self.base_mm, other.tip_mm)
            || meet(self.tip_mm, other.base_mm)
    }
}

impl TipIndex {
    /// The stretches of the tips at height `z_mm` that touch `start`, as
    /// [`Self::contact`] reads a touch, added to `found`: a tip counts once,
    /// on however many layers it touches.
    pub fn touching(
        &self,
        grid: &TipGrid,
        z_mm: f64,
        start: &TipTarget<'_>,
        reach_px: [u32; 2],
        found: &mut Vec<TipStretch>,
    ) {
        let reach = [f64::from(reach_px[0]), f64::from(reach_px[1])];
        for cone in self.at(z_mm) {
            let mut touches = false;
            for_each_pixel(grid, cone, z_mm, start.bbox, reach, |x, y| {
                touches |= near(start.runs, x, y, reach_px) && !lit(start.below, x, y);
            });
            let stretch = TipStretch {
                tip_mm: cone.tip,
                base_mm: [0, 1, 2].map(|i| cone.axis[i].mul_add(cone.length, cone.tip[i])),
            };
            if touches && !found.iter().any(|known| known.same_tip(&stretch)) {
                found.push(stretch);
            }
        }
    }
}

/// What [`TipIndex::contact`] judges: the first pixels of a part.
#[derive(Clone, Copy, Debug)]
pub struct TipTarget<'a> {
    /// Its pixels, sorted by row.
    pub runs: &'a [Run],
    /// How many they are.
    pub area_px: u64,
    /// The box they span.
    pub bbox: PixelBox,
    /// What the model has on the layer below, sorted by row.
    pub below: &'a [Run],
}

/// How tips touch a part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TipContact {
    /// The tip pixels that touch it.
    pub px: usize,
    /// Whether they hold it: a tip reaches it with its share of its section,
    /// or the part is small enough to stand with half of itself on tips.
    pub holds: bool,
}

/// Whether pixel `(x, y)` is in the run list `runs`, sorted by row.
pub(crate) fn lit(runs: &[Run], x: u32, y: u32) -> bool {
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

/// Calls `visit` with every pixel within `reach` of `bbox` whose centre lies
/// inside `cone` at height `z_mm`.
fn for_each_pixel(
    grid: &TipGrid,
    cone: &Placed,
    z_mm: f64,
    bbox: PixelBox,
    reach: [f64; 2],
    mut visit: impl FnMut(u32, u32),
) {
    let [x0, y0, x1, y1] = cone.bounds_at(z_mm);
    let map = &grid.pixel_map;
    let a = map.mm_to_pixel(Point2D::new(x0, y0));
    let b = map.mm_to_pixel(Point2D::new(x1, y1));
    // Screen mirrors swap the corners.
    let left = (a.0.min(b.0).floor() - 1.0).max(f64::from(bbox.x0) - reach[0]);
    let right = (a.0.max(b.0).ceil() + 1.0).min(f64::from(bbox.x1) + reach[0]);
    let top = (a.1.min(b.1).floor() - 1.0).max(f64::from(bbox.y0) - reach[1]);
    let bottom = (a.1.max(b.1).ceil() + 1.0).min(f64::from(bbox.y1) + reach[1]);
    if left >= right || top >= bottom {
        return;
    }
    for y in top.max(0.0) as u32..bottom.max(0.0) as u32 {
        for x in left.max(0.0) as u32..right.max(0.0) as u32 {
            let [px, py] = pixel_to_plate(map, [f64::from(x) + 0.5, f64::from(y) + 0.5]);
            if cone.holds([px, py, z_mm]) {
                visit(x, y);
            }
        }
    }
}

#[cfg(test)]
mod tips_tests {
    use super::*;

    /// #433: two tips can end a hair apart inside the model, each leaning its
    /// own way; a neck and its cone meet end to end.
    #[test]
    fn stretches_are_one_tip_only_when_they_meet() {
        let neck = TipStretch {
            tip_mm: [0.0, 0.0, 10.0],
            base_mm: [0.0, 0.0, 9.5],
        };
        let cone = TipStretch {
            tip_mm: [0.0, 0.0, 9.5],
            base_mm: [1.0, 0.0, 7.0],
        };
        let other = TipStretch {
            tip_mm: [0.1, 0.05, 10.0],
            base_mm: [-1.0, 0.0, 7.0],
        };
        assert!(neck.same_tip(&cone) && cone.same_tip(&neck));
        assert!(!neck.same_tip(&other) && !cone.same_tip(&other));
    }

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
