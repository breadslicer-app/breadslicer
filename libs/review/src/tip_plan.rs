/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Where the tips of the unsupported islands go (#437).
//!
//! [`plan_tips`] takes the islands of a review and gives the points to put a
//! tip on: what the question after Slice, Fix All and a row's "Add support
//! here" place, through the support engine. It reads only the islands; the
//! engine decides whether a point can be held and refuses the ones it cannot
//! reach, and nothing here draws anything.
//!
//! # The tips of an island
//!
//! - **The first one**, so it starts: on the first layer where a contact fits
//!   in the island, as near the middle of its first pixels as it fits. On an
//!   island that starts wide enough that is under the first pixels; a smaller
//!   one gets it once it has grown to the tip's contact diameter. An island
//!   that never does gets none ([`TipPlan::too_narrow`]).
//! - **One every [`MM2_PER_TIP`]** of the surface the island gains before it
//!   joins the rest.
//! - **[`GROWTH_TIPS`] more** on a layer that grows [`GROWTH_FACTOR`] times
//!   over the one under it, and [`CHAINED_GROWTH_TIPS`] more if the next one
//!   grows [`CHAINED_GROWTH_FACTOR`] times over that. The areas are lit
//!   pixels, so this counts from the first pixel that exists.
//!
//! What a layer asks for and does not fit waits for the layers above, where
//! the island is larger; what never fits is not placed. A tip fits where a
//! contact lies entirely on the island's pixels, [`SPACING_DIAMETERS`]
//! diameters from the island's other tips, and the extra ones go as far from
//! those as they can, which is where it grew: a small tree.
//!
//! An island too narrow for the plan's tip gets the smallest there is
//! ([`TipPlanOptions::smallest_contact_mm`]). Only one too narrow for that is
//! left with none ([`TipPlan::too_narrow`]), and it is a note, not an error:
//! it is too small for a tip ([`TipPlan::note_too_small`]).
//!
//! The tips an island has already, by hand or generated, count for what its
//! growth asks for, never for the first one: they stand higher than its first
//! pixels, or it would be held.
//!
//! A point is right above the underside of its pixel: the top of the first
//! layer of the island that lights it, inside the solid, where the surface
//! nearest is that underside.
//!
//! The engine may not reach a point: under the middle of an island the way
//! down can be blocked where one beside it is clear. So each tip comes with
//! the other points that would do ([`PlannedTip::others`]). A first tip's are
//! the rim of the first pixels, then the rest of them, then the layers right
//! above: it stays under what it must hold for as long as it can.
//!
//! # The tips of a region that grew at once
//!
//! A region held or not that grows at once wants tips of its own
//! ([`Growth`]). They go on the surface it gained, the pixels with no model
//! under them: as far from each other and from those it has as they can,
//! [`SPACING_DIAMETERS`] apart. What does not fit on its layer waits for the
//! layers above, where it has grown; what never fits is not placed, and a
//! region with room for none is [`TipPlan::no_room`].
//!
//! # Candidates
//!
//! A plan is candidates, not tips (#433). Whoever places them takes the first
//! the engine can reach and [`crate::IslandStart::held_by`] says holds the
//! island, the very decision the next review makes. An island none of them
//! holds gets no tip at all, and says why.

use std::sync::Arc;

use crate::checks::islands::{GROWTH_FACTOR, Growth, Island};
use crate::finding::{CheckKind, Finding, PlatePoint, Severity, SuggestedFix};
use crate::runs::{Run, difference, erode_ellipse};
use crate::tips::lit;

/// The surface an island gains for each extra tip, in square millimetres: 10.
pub const MM2_PER_TIP: f64 = 10.0;

/// The extra tips a layer [`GROWTH_FACTOR`] times the one under it asks for.
pub const GROWTH_TIPS: usize = 2;

/// How many times the layer that jumped the next layer must be for its own
/// tips: 5.
pub const CHAINED_GROWTH_FACTOR: f64 = 5.0;

/// The extra tips that second jump asks for: 3, of the three or four decided.
pub const CHAINED_GROWTH_TIPS: usize = 3;

/// How far apart the tips of an island stand, in contact diameters: 2, the
/// width of a contact between two.
pub const SPACING_DIAMETERS: f64 = 2.0;

/// How many layers above the one it fits on a first tip's other points reach.
const OTHERS_LAYERS: usize = 2;

/// The most points of one layer among a tip's others.
const OTHERS_PER_LAYER: u64 = 24;

/// The most points of the rim of the first pixels among a first tip's others:
/// a fifth of a millimetre apart on a rim of 3 mm, so they find the stretch
/// of it the engine reaches, a few tenths long on a real island.
const OTHERS_ON_THE_RIM: u64 = 64;

/// How wide the rim of the first pixels is, in pixels.
const RIM_PX: f64 = 3.0;

/// The most pixels of a layer weighed for a tip. Past this they are sampled
/// on a grid: a floating body covers millions, and a pixel more or less is
/// not worth reading them all.
const MAX_CANDIDATES: u64 = 4096;

/// The pixels of an island on one layer.
#[derive(Clone, Debug, PartialEq)]
pub struct IslandLayer {
    /// The layer.
    pub layer: usize,
    /// The bottom of its slab, in millimetres above the plate.
    pub bottom_mm: f64,
    /// The top of its slab.
    pub top_mm: f64,
    /// Its pixels, sorted by row.
    pub runs: Vec<Run>,
}

/// The pixels of an island on its first
/// [`MAX_FOOTPRINT_LAYERS`](crate::MAX_FOOTPRINT_LAYERS) layers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IslandPixels {
    /// Where the corner of pixel `(0, 0)` lies on the plate, in millimetres.
    pub origin_mm: [f64; 2],
    /// How far the next pixel along each axis is, in millimetres: negative
    /// on a mirrored screen.
    pub step_mm: [f64; 2],
    /// Its layers, from its first.
    pub layers: Vec<IslandLayer>,
}

impl IslandPixels {
    /// The centre of pixel `(x, y)` on the plate.
    fn centre_mm(&self, x: u32, y: u32) -> [f64; 2] {
        [
            (f64::from(x) + 0.5).mul_add(self.step_mm[0], self.origin_mm[0]),
            (f64::from(y) + 0.5).mul_add(self.step_mm[1], self.origin_mm[1]),
        ]
    }

    /// Where a tip on pixel `(x, y)` is aimed at: right above its underside,
    /// the top of the first layer up to `layers[upto]` that lights it.
    fn aim(&self, upto: usize, x: u32, y: u32) -> PlatePoint {
        let z_mm = self.layers[..=upto]
            .iter()
            .find(|layer| lit(&layer.runs, x, y))
            .unwrap_or(&self.layers[upto])
            .top_mm;
        let [x_mm, y_mm] = self.centre_mm(x, y);
        PlatePoint::new(x_mm, y_mm, z_mm)
    }

    /// The middle of `runs`, on the plate.
    fn middle_mm(&self, runs: &[Run]) -> [f64; 2] {
        let mut sums = [0.0, 0.0];
        let mut pixels = 0.0;
        for run in runs {
            let len = f64::from(run.len());
            sums[0] += len * (f64::from(run.x0) + f64::from(run.x1)) * 0.5;
            sums[1] += len * (f64::from(run.row) + 0.5);
            pixels += len;
        }
        if pixels == 0.0 {
            return self.origin_mm;
        }
        [
            (sums[0] / pixels).mul_add(self.step_mm[0], self.origin_mm[0]),
            (sums[1] / pixels).mul_add(self.step_mm[1], self.origin_mm[1]),
        ]
    }
}

/// How the tips of a plan are worked out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TipPlanOptions {
    /// The contact diameter of the tips placed, in millimetres.
    pub contact_diameter_mm: f64,
    /// See [`MM2_PER_TIP`].
    pub mm2_per_tip: f64,
    /// See [`GROWTH_FACTOR`].
    pub growth_factor: f64,
    /// See [`GROWTH_TIPS`].
    pub growth_tips: usize,
    /// See [`CHAINED_GROWTH_FACTOR`].
    pub chained_growth_factor: f64,
    /// See [`CHAINED_GROWTH_TIPS`].
    pub chained_growth_tips: usize,
    /// See [`SPACING_DIAMETERS`].
    pub spacing_diameters: f64,
    /// The contact of the smallest tip there is: an island too narrow for
    /// `contact_diameter_mm` gets that one before it is given up.
    pub smallest_contact_mm: Option<f64>,
}

impl TipPlanOptions {
    /// The decided rules, for tips of a contact of `contact_diameter_mm`.
    pub fn for_tip(contact_diameter_mm: f64) -> Self {
        Self {
            contact_diameter_mm,
            mm2_per_tip: MM2_PER_TIP,
            growth_factor: GROWTH_FACTOR,
            growth_tips: GROWTH_TIPS,
            chained_growth_factor: CHAINED_GROWTH_FACTOR,
            chained_growth_tips: CHAINED_GROWTH_TIPS,
            spacing_diameters: SPACING_DIAMETERS,
            smallest_contact_mm: None,
        }
    }

    /// These options, with `smallest_mm` as the smallest tip there is.
    #[must_use]
    pub fn with_smallest(mut self, smallest_mm: f64) -> Self {
        self.smallest_contact_mm = Some(smallest_mm);
        self
    }
}

/// A tip to place.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedTip {
    /// The island it holds, by its index in the list planned.
    pub island: usize,
    /// Where it touches, in plate millimetres.
    pub at: PlatePoint,
    /// The layer it was placed for.
    pub layer: usize,
    /// The points that would do as well, best first, if the engine cannot
    /// hold `at`.
    pub others: Vec<PlatePoint>,
    /// It takes the smallest tip: the plan's would not fit.
    pub small: bool,
}

/// The tips of a list of islands.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TipPlan {
    /// Every tip, island by island, the first of each first.
    pub tips: Vec<PlannedTip>,
    /// The islands that never get as wide as a contact, by index: nothing
    /// goes on them.
    pub too_narrow: Vec<usize>,
    /// The tips of the regions that grew at once: [`PlannedTip::island`] is
    /// the region's index in the list planned.
    pub growth: Vec<PlannedTip>,
    /// The regions no contact fits on, by index.
    pub no_room: Vec<usize>,
}

impl TipPlan {
    /// The tips of island `island`, the one that starts it first.
    pub fn of(&self, island: usize) -> impl Iterator<Item = &PlannedTip> + '_ {
        self.tips.iter().filter(move |tip| tip.island == island)
    }

    /// Writes down what is left of the islands no tip fits on: not an error
    /// anyone can act on, but a note that says they are too small for one.
    pub fn note_too_small(&self, findings: &mut [Finding], islands: &[Island]) {
        for island in self.too_narrow.iter().filter_map(|&i| islands.get(i)) {
            let finding = findings.iter_mut().find(|finding| {
                finding.check == CheckKind::Island
                    && finding.first_layer == island.first_layer
                    && matches!(finding.fix, Some(SuggestedFix::AddSupport { at }) if at == island.at)
            });
            if let Some(finding) = finding {
                finding.severity = Severity::Info;
                finding.title = TOO_SMALL_TITLE.to_owned();
            }
        }
    }

    /// The tips of region `growth`.
    pub fn of_growth(&self, growth: usize) -> impl Iterator<Item = &PlannedTip> + '_ {
        self.growth.iter().filter(move |tip| tip.island == growth)
    }
}

/// The title of an island that never gets as wide as the smallest tip.
pub const TOO_SMALL_TITLE: &str = "Island too small for a tip";

/// The tips `islands` need: see the module documentation.
pub fn plan_tips(islands: &[Island], options: &TipPlanOptions) -> TipPlan {
    let mut plan = TipPlan::default();
    for (index, island) in islands.iter().enumerate() {
        let mut tips = Placing::new(island, options).run();
        if let (true, Some(smallest)) = (tips.is_empty(), options.smallest_contact_mm) {
            let small = TipPlanOptions {
                contact_diameter_mm: smallest,
                ..*options
            };
            tips = Placing::new(island, &small).run();
            tips.iter_mut().for_each(|tip| tip.small = true);
        }
        if tips.is_empty() {
            plan.too_narrow.push(index);
        }
        plan.tips.extend(tips.into_iter().map(|mut tip| {
            tip.island = index;
            tip
        }));
    }
    plan
}

/// [`plan_tips`], and the tips of the regions that grew at once.
pub fn plan_all_tips(islands: &[Island], growths: &[Growth], options: &TipPlanOptions) -> TipPlan {
    let mut plan = plan_tips(islands, options);
    for (index, growth) in growths.iter().enumerate() {
        let tips = Placing::for_growth(growth, options).run_growth();
        if tips.is_empty() {
            plan.no_room.push(index);
        }
        plan.growth.extend(tips.into_iter().map(|mut tip| {
            tip.island = index;
            tip
        }));
    }
    plan
}

/// One island's tips being worked out, or one region's.
struct Placing<'i> {
    growth_mm2: &'i [f64],
    pixels: Arc<IslandPixels>,
    options: &'i TipPlanOptions,
    /// The tips it has by now, on the plate: those it came with and those
    /// placed.
    standing: Vec<[f64; 2]>,
    /// How many it came with that nothing has counted yet.
    spare: usize,
    /// The tips placed.
    placed: Vec<PlannedTip>,
    /// The extra tips asked for that have not found a place.
    owed: usize,
}

impl<'i> Placing<'i> {
    fn new(island: &'i Island, options: &'i TipPlanOptions) -> Self {
        Self {
            growth_mm2: &island.growth_mm2,
            pixels: island.pixels.clone(),
            options,
            standing: island.tips_mm.iter().map(|tip| [tip[0], tip[1]]).collect(),
            spare: island.tips_mm.len(),
            placed: Vec::new(),
            owed: 0,
        }
    }

    fn for_growth(growth: &'i Growth, options: &'i TipPlanOptions) -> Self {
        Self {
            growth_mm2: &[],
            pixels: growth.pixels.clone(),
            options,
            standing: growth.tips_mm.iter().map(|tip| [tip[0], tip[1]]).collect(),
            spare: 0,
            placed: Vec::new(),
            owed: growth.wanted.saturating_sub(growth.tips_mm.len()),
        }
    }

    /// What a region's growth is owed, layer by layer until it is paid.
    fn run_growth(mut self) -> Vec<PlannedTip> {
        let pixels = self.pixels.clone();
        for at in 0..pixels.layers.len() {
            if self.owed == 0 {
                break;
            }
            let spots = self.spots_on(at);
            if spots.is_empty() {
                continue;
            }
            // A surface with no tip yet: the first goes in the middle of it.
            if self.standing.is_empty() {
                let mm = |&(x, y): &(u32, u32)| pixels.centre_mm(x, y);
                let middle = pixels.middle_mm(&pixels.layers[at].runs);
                let spot = spots
                    .iter()
                    .min_by(|a, b| distance2(mm(a), middle).total_cmp(&distance2(mm(b), middle)))
                    .copied()
                    .expect("spots to pick from");
                let others = self.others_near(spot, &spots, at);
                self.place(spot, at, others);
                self.owed -= 1;
            }
            self.place_owed(at, &spots);
        }
        self.placed
    }

    fn run(mut self) -> Vec<PlannedTip> {
        let pixels = self.pixels.clone();
        let growth = self.growth_mm2;
        let first_mm2 = growth.first().copied().unwrap_or(0.0);
        let mut peak_mm2 = first_mm2;
        let mut for_surface = 0;
        let mut jumped = false;
        for (at, &area) in growth.iter().enumerate() {
            peak_mm2 = peak_mm2.max(area);
            let surface = ((peak_mm2 - first_mm2) / self.options.mm2_per_tip).floor() as usize;
            self.owed += surface - for_surface;
            for_surface = surface;
            let below = if at == 0 { 0.0 } else { growth[at - 1] };
            let jump = below > 0.0 && area >= self.options.growth_factor * below;
            if jump {
                self.owed += self.options.growth_tips;
            } else if jumped && area >= self.options.chained_growth_factor * below {
                self.owed += self.options.chained_growth_tips;
            }
            jumped = jump;
            if at < pixels.layers.len() {
                self.settle(at);
            }
        }
        // What the layers past the pixels asked for goes on the last known.
        if growth.len() > pixels.layers.len() && !pixels.layers.is_empty() {
            self.settle(pixels.layers.len() - 1);
        }
        self.placed
    }

    /// Places on layer `at` what is due and fits there.
    fn settle(&mut self, at: usize) {
        // The tips the island came with pay for what is owed.
        let paid = self.owed.min(self.spare);
        self.owed -= paid;
        self.spare -= paid;
        if !self.placed.is_empty() && self.owed == 0 {
            return;
        }
        let pixels = self.pixels.clone();
        let spots = self.spots_on(at);
        if spots.is_empty() {
            return;
        }
        let mm = |&(x, y): &(u32, u32)| pixels.centre_mm(x, y);
        if self.placed.is_empty() {
            let start = pixels.middle_mm(&pixels.layers[0].runs);
            let spot = spots
                .iter()
                .min_by(|a, b| distance2(mm(a), start).total_cmp(&distance2(mm(b), start)))
                .copied()
                .expect("spots to pick from");
            let others = self.first_others(spot, at);
            self.place(spot, at, others);
        }
        self.place_owed(at, &spots);
    }

    /// Where a whole contact fits on layer `at`.
    fn spots_on(&self, at: usize) -> Vec<(u32, u32)> {
        let pixels = &self.pixels;
        // The centre of a pixel: half of it counts towards the contact.
        let [rx, ry] = pixels
            .step_mm
            .map(|step| (0.5 * self.options.contact_diameter_mm / step.abs() - 0.5).max(0.0));
        candidates(&erode_ellipse(&pixels.layers[at].runs, rx, ry))
    }

    /// Places on `spots`, of layer `at`, what is owed and fits there, as far
    /// from the tips that stand as it can.
    fn place_owed(&mut self, at: usize, spots: &[(u32, u32)]) {
        let pixels = self.pixels.clone();
        let mm = |&(x, y): &(u32, u32)| pixels.centre_mm(x, y);
        let spacing = self.options.spacing_diameters * self.options.contact_diameter_mm;
        while self.owed > 0 {
            let nearest = |spot: &(u32, u32)| {
                self.standing
                    .iter()
                    .map(|tip| distance2(mm(spot), *tip))
                    .fold(f64::INFINITY, f64::min)
            };
            let Some(spot) = spots
                .iter()
                .max_by(|a, b| nearest(a).total_cmp(&nearest(b)))
                .filter(|spot| nearest(spot) >= spacing * spacing)
                .copied()
            else {
                return;
            };
            let free: Vec<(u32, u32)> = spots
                .iter()
                .filter(|other| nearest(other) >= spacing * spacing)
                .copied()
                .collect();
            let others = self.others_near(spot, &free, at);
            self.place(spot, at, others);
            self.owed -= 1;
        }
    }

    /// The spots of `spots`, of layer `at`, that would do for `spot`.
    fn others_near(&self, spot: (u32, u32), spots: &[(u32, u32)], at: usize) -> Vec<PlatePoint> {
        self.nearest_first(spot, spots.to_vec())
            .into_iter()
            .take(OTHERS_PER_LAYER as usize)
            .map(|(x, y)| self.pixels.aim(at, x, y))
            .collect()
    }

    /// `spots` without `spot`, the nearest to it first.
    fn nearest_first(&self, spot: (u32, u32), mut spots: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
        let from = self.pixels.centre_mm(spot.0, spot.1);
        let mm = |&(x, y): &(u32, u32)| self.pixels.centre_mm(x, y);
        spots.retain(|other| *other != spot);
        spots.sort_by(|a, b| distance2(mm(a), from).total_cmp(&distance2(mm(b), from)));
        spots
    }

    /// The other points of a first tip placed at `spot`, on layer `at`.
    fn first_others(&self, spot: (u32, u32), at: usize) -> Vec<PlatePoint> {
        let pixels = &self.pixels;
        let first = &pixels.layers[0].runs;
        let inside = erode_ellipse(first, RIM_PX, RIM_PX);
        let rim = difference(first, &inside);
        let mut others = Vec::new();
        let mut add = |layer: usize, picked: Vec<(u32, u32)>| {
            let near = self.nearest_first(spot, picked);
            others.extend(near.into_iter().map(|(x, y)| pixels.aim(layer, x, y)));
        };
        add(0, sampled(&rim, OTHERS_ON_THE_RIM));
        add(0, sampled(&inside, OTHERS_PER_LAYER));
        let reach = (at + OTHERS_LAYERS).min(pixels.layers.len() - 1);
        for layer in 1..=reach {
            add(layer, sampled(&pixels.layers[layer].runs, OTHERS_PER_LAYER));
        }
        others
    }

    fn place(&mut self, (x, y): (u32, u32), at: usize, others: Vec<PlatePoint>) {
        let point = self.pixels.aim(at, x, y);
        self.standing.push([point.x_mm, point.y_mm]);
        self.placed.push(PlannedTip {
            island: 0,
            at: point,
            layer: self.pixels.layers[at].layer,
            others,
            small: false,
        });
    }
}

fn distance2(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).mul_add(a[0] - b[0], (a[1] - b[1]) * (a[1] - b[1]))
}

/// The pixels of `room` to weigh: all of them, or a grid over them when they
/// are more than [`MAX_CANDIDATES`].
fn candidates(room: &[Run]) -> Vec<(u32, u32)> {
    sampled(room, MAX_CANDIDATES)
}

/// The pixels of `room`, on a grid that leaves about `most` when they are
/// more.
fn sampled(room: &[Run], most: u64) -> Vec<(u32, u32)> {
    let area: u64 = room.iter().map(|run| u64::from(run.len())).sum();
    let step = ((area as f64 / most as f64).sqrt().ceil() as u32).max(1);
    room.iter()
        .filter(|run| run.row % step == 0)
        .flat_map(|run| {
            let first = run.x0.next_multiple_of(step);
            (first..run.x1)
                .step_by(step as usize)
                .map(move |x| (x, run.row))
        })
        .collect()
}

#[cfg(test)]
#[path = "tests_tip_plan.rs"]
mod tests;
