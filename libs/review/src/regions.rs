/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The sweep (#432): one pass over the model's layers that builds the small
//! tables every check reads. It is the only step of a review that touches
//! pixels and it grows with the model, never with the supports, whose
//! geometry it does not read.
//!
//! # What one layer goes through
//!
//! Each layer is rasterised once at the printer's own resolution, with the
//! slicer's binary rasteriser (the pixels a printer file gets, without
//! anti-aliasing), and kept as row runs ([`LayerPixels`]):
//!
//! 1. **Clean-up.** The layer's pixels are opened by a square of
//!    [`SweepOptions::min_wall_px`] pixels: what no such square of solid
//!    covers is thinner than the printer can make, a wall, a pin or an edge
//!    of one pixel. Those are the layer's *stray pixels*: they leave the
//!    layer, and every later step reads what is left.
//! 2. **Solid regions**: the connected parts of what is left, with
//!    **8-connectivity**: pixels that touch by an edge or a corner belong
//!    together. Each [`Region`] carries its area, pixel bounding box,
//!    centroid, a pixel inside it and whether it reaches the screen edge.
//! 3. **Air regions.** *Air* is every other pixel, which in the vat is liquid
//!    resin. Resin needs an opening of some width to flow, so the air is
//!    opened by a square of [`SweepOptions::drain_width_px`] pixels: only the
//!    air pixels such a square of air covers are kept. A narrower crack is a
//!    hairline that thick resin barely passes and light bleed tends to cure
//!    shut. What is left is split with **4-connectivity**, the dual of the
//!    solid rule: a solid ring closed only through corners still seals the
//!    air inside it. An air region that reaches the screen edge is *open
//!    air*; every other air region is enclosed within its layer.
//! 4. **Links** with the layer below ([`Overlap`]): how many pixels every
//!    region shares with every region of the same kind straight below it
//!    (same `x`, `y`, never diagonally). Air regions only link where one
//!    side is enclosed, through the air the two layers share, opened the
//!    same way: open air is drained whatever lies below it.
//! 5. **Part starts** ([`PartStart`]): a solid region that no link holds
//!    keeps its pixels, the only pixels the sweep keeps of any region (see
//!    [`PartStart`] for what counts as a link).
//!
//! No image outlives its layer. What stays is [`PlateRegions`]: for every
//! layer, its regions, its links, its stray pixels and its part starts.
//! These tables are plain data: the checks read them, and they are all a
//! later review needs to re-judge a part without rasterising.
//!
//! # How the layers are walked
//!
//! Layers are cut into chunks that run in parallel, each walking its layers
//! from the bottom with the layer below in hand. The first layer of a chunk
//! rasterises the one below it again, so chunks never wait for one another:
//! what is in memory is two layers of runs per thread, however tall the
//! plate. The result does not depend on the number of threads.
//!
//! [`PlateRegions::sweep`] reports each finished layer and stops within a
//! layer of its [`CancelToken`] being tripped.
//!
//! # Conventions
//!
//! Raster space is the slicer's raster grid, mirror included: pixel `(x, y)`
//! covers `[x, x + 1) x [y, y + 1)`, row `y` from 0 to `height - 1`, row 0
//! at the plate's minimum Y (some printer files store their rows the other
//! way round). Convert to plate millimetres with
//! [`ReviewInput::pixel_to_plate`] or [`ReviewInput::focus`]. Layer 0 rests
//! on the build plate; nothing below it is represented. Nothing above the
//! last layer is represented either, although in reality it is all open
//! resin: an air region of the last layer is open towards the resin above
//! even when it is enclosed within its layer.
//!
//! Region indices are the order in which a raster scan (row by row, left to
//! right) first meets each region, so they are deterministic and identical
//! between [`PlateRegions`] and [`LayerPixels::compute`].

use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use rayon::prelude::*;
use sc_sl1cer::CancelToken;
use sc_sl1cer::contour::ContourLayer;
use sc_sl1cer::raster::rasterize_layers_binary;

use crate::input::ReviewInput;
use crate::runs::{self, Run, for_each_overlap};
use crate::union_find::{Groups, RegionRef, UnionFind};

/// Which neighbours of a pixel count as connected to it within a layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Connectivity {
    /// The 4 pixels sharing an edge.
    Four,
    /// The 8 pixels sharing an edge or a corner.
    Eight,
}

impl Connectivity {
    /// How far a run on the next row may start past the end of a run and
    /// still touch it.
    const fn reach(self) -> u32 {
        match self {
            Self::Four => 0,
            Self::Eight => 1,
        }
    }
}

/// The connectivity of solid regions.
pub const SOLID_CONNECTIVITY: Connectivity = Connectivity::Eight;

/// The connectivity of air regions, the dual of [`SOLID_CONNECTIVITY`].
pub const AIR_CONNECTIVITY: Connectivity = Connectivity::Four;

/// Default of [`SweepOptions::min_wall_px`].
pub const DEFAULT_MIN_WALL_PX: u32 = 2;

/// Default of [`SweepOptions::drain_width_px`].
pub const DEFAULT_DRAIN_WIDTH_PX: u32 = 3;

/// Default of [`SweepOptions::min_link_mm2`]: a link under 0.05 mm² holds
/// nothing, on every printer, whatever its pixel. It is the one number to
/// raise if prints ask for it.
pub const DEFAULT_MIN_LINK_MM2: f64 = 0.05;

/// How far around a part start's box the sweep keeps the solid of the layer
/// below: past the reach of any tip that touches it, on any screen.
pub const START_MARGIN_PX: u32 = 32;

/// What the sweep reads a layer with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SweepOptions {
    /// The side of the square that opens the solid, in pixels: what no such
    /// square of solid covers is a stray pixel. 2 takes everything one pixel
    /// wide, which is what a feature thinner than a pixel lights, and leaves
    /// whatever is two pixels wide. 0 or 1 keeps every pixel.
    pub min_wall_px: u32,
    /// The side of the square that opens the air, in pixels: the narrowest
    /// opening resin flows through, within a layer or from one layer to the
    /// next. 3 pixels is 57 x 72 µm on a 19 x 24 µm screen. 0 or 1 keeps
    /// every air pixel.
    pub drain_width_px: u32,
    /// The smallest link with the layer below that holds a larger part, in
    /// square millimetres (see [`PartStart`]).
    pub min_link_mm2: f64,
}

impl Default for SweepOptions {
    fn default() -> Self {
        Self {
            min_wall_px: DEFAULT_MIN_WALL_PX,
            drain_width_px: DEFAULT_DRAIN_WIDTH_PX,
            min_link_mm2: DEFAULT_MIN_LINK_MM2,
        }
    }
}

impl SweepOptions {
    /// [`min_link_mm2`](Self::min_link_mm2) in whole pixels of `input`, at
    /// least one.
    pub(crate) fn min_link_px(&self, input: &ReviewInput<'_>) -> u64 {
        ((self.min_link_mm2 / input.pixel_area_mm2()).ceil() as u64).max(1)
    }
}

/// Whether a link of `link_px` pixels with the layer below holds a part of
/// `area_px` pixels: at least `min_link_px` of them, or at least half of the
/// part, when the part is small enough to rest whole on what is below.
#[inline]
pub const fn link_holds(link_px: u64, area_px: u64, min_link_px: u64) -> bool {
    link_px >= min_link_px || 2 * link_px >= area_px
}

/// A pixel rectangle, half-open: columns `x0..x1`, rows `y0..y1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PixelBox {
    /// First column.
    pub x0: u32,
    /// First row.
    pub y0: u32,
    /// One past the last column.
    pub x1: u32,
    /// One past the last row.
    pub y1: u32,
}

impl PixelBox {
    fn of_run(run: &Run) -> Self {
        Self {
            x0: run.x0,
            y0: run.row,
            x1: run.x1,
            y1: run.row + 1,
        }
    }

    fn include(&mut self, run: &Run) {
        self.x0 = self.x0.min(run.x0);
        self.x1 = self.x1.max(run.x1);
        self.y0 = self.y0.min(run.row);
        self.y1 = self.y1.max(run.row + 1);
    }

    /// Columns covered.
    #[inline]
    pub const fn width(&self) -> u32 {
        self.x1 - self.x0
    }

    /// Rows covered.
    #[inline]
    pub const fn height(&self) -> u32 {
        self.y1 - self.y0
    }

    /// The smallest box holding both.
    pub fn union(&self, other: &Self) -> Self {
        Self {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }

    /// The box's centre in raster space.
    pub fn centre(&self) -> [f64; 2] {
        [
            0.5 * (f64::from(self.x0) + f64::from(self.x1)),
            0.5 * (f64::from(self.y0) + f64::from(self.y1)),
        ]
    }
}

/// One connected region of one layer, solid or air.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    /// Pixels in the region.
    pub area_px: u64,
    /// The pixels it spans.
    pub bbox: PixelBox,
    /// Its centre of mass in raster space (pixel centres at `+ 0.5`). It can
    /// fall outside a region that is not convex, such as a ring.
    pub centroid_px: [f64; 2],
    /// A pixel of the region nearest to its centroid: where to point at it,
    /// or to aim a support or a drain.
    pub inside_px: [u32; 2],
    /// Whether it has a pixel on the screen's outer edge. For solid, the
    /// part is cut by the screen edge; for air, it is open air. Air on the
    /// last layer is open towards the resin above as well, whatever this
    /// says.
    pub touches_border: bool,
    /// Pixels with a pixel of the same kind straight below, in the previous
    /// layer: for solid, the pixels resting on cured resin. Always 0 on
    /// layer 0, which rests on the plate. For air, only the links an
    /// enclosed region has count.
    pub below_px: u64,
    /// Pixels with a pixel of the same kind straight above, in the next
    /// layer. Always 0 on the last layer. Counted as `below_px` is.
    pub above_px: u64,
}

impl Region {
    /// The centre of `inside_px` in raster space.
    pub fn inside_point(&self) -> [f64; 2] {
        [
            f64::from(self.inside_px[0]) + 0.5,
            f64::from(self.inside_px[1]) + 0.5,
        ]
    }
}

/// The first pixels of a solid region that the layer below does not hold.
///
/// A link holds a part when it shares at least
/// [`min_link_mm2`](SweepOptions::min_link_mm2) with the layer below, or at
/// least half of its own pixels: a thin pin standing on a surface rests
/// whole on it, however small it is. Any other region is a part of its own
/// from this layer on, whatever it may touch: a neck that fine holds nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartStart {
    /// The region's index in [`LayerRegions::solid`].
    pub region: u32,
    /// Its pixels.
    pub runs: Vec<Run>,
    /// What the layer below has around them, within
    /// [`START_MARGIN_PX`] pixels of the region's box.
    pub below: Vec<Run>,
}

/// The tables of one layer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayerRegions {
    /// Solid regions, in scan order.
    pub solid: Vec<Region>,
    /// Air regions, in scan order, of the opened air. There is at least one
    /// unless the layer is solid from edge to edge.
    pub air: Vec<Region>,
    /// The stray pixels the clean-up took off this layer, in no region.
    pub stray: Vec<Run>,
    /// The solid regions that start a part on this layer, by region index.
    pub starts: Vec<PartStart>,
}

impl LayerRegions {
    /// Solid pixels in the layer.
    pub fn solid_area_px(&self) -> u64 {
        self.solid.iter().map(|region| region.area_px).sum()
    }

    /// Whether the layer has no solid pixel.
    pub fn is_empty(&self) -> bool {
        self.solid.is_empty()
    }

    /// The pixels the layer's solid spans, if it has any.
    pub fn solid_bbox(&self) -> Option<PixelBox> {
        self.solid
            .iter()
            .map(|region| region.bbox)
            .reduce(|a, b| a.union(&b))
    }

    /// The part start of solid region `region`, if it is one.
    pub fn start_of(&self, region: u32) -> Option<&PartStart> {
        self.starts
            .binary_search_by_key(&region, |start| start.region)
            .ok()
            .map(|at| &self.starts[at])
    }
}

/// Pixels shared by a region of one layer and a region of the next.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Overlap {
    /// Index of the region in the lower layer.
    pub lower: u32,
    /// Index of the region in the upper layer.
    pub upper: u32,
    /// Pixels they share (straight above one another).
    pub pixels: u64,
}

/// Overlaps between one layer and the layer below it, sorted by
/// `(lower, upper)`. Only pairs that share at least one pixel are listed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayerLinks {
    /// Solid regions below and above.
    pub solid: Vec<Overlap>,
    /// Air regions below and above, where at least one of the two is
    /// enclosed, counting the opened air they share.
    pub air: Vec<Overlap>,
}

/// The pixels of one layer as row runs, with the region of every run.
///
/// [`PlateRegions`] keeps only the tables; a check that needs the pixels of
/// a layer recomputes them with [`LayerPixels::compute`], which gives the
/// same region indices.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayerPixels {
    /// The layer's pixels as rasterised.
    pub model: Vec<Run>,
    /// What the clean-up leaves of them: the layer's solid.
    pub solid: Vec<Run>,
    /// What the clean-up took: `model` less `solid`.
    pub stray: Vec<Run>,
    /// The solid region of each run of `solid`.
    pub solid_labels: Vec<u32>,
    /// Number of solid regions.
    pub solid_count: usize,
    /// The opened air, within the complement of `solid` on the screen.
    pub air: Vec<Run>,
    /// The air region of each run of `air`.
    pub air_labels: Vec<u32>,
    /// Number of air regions.
    pub air_count: usize,
    /// Whether each air region is enclosed within the layer.
    pub air_enclosed: Vec<bool>,
}

impl LayerPixels {
    /// Rasterises, cleans and labels layer `layer` of `input`.
    ///
    /// # Panics
    ///
    /// When `layer` is out of range.
    pub fn compute(input: &ReviewInput<'_>, layer: usize, options: &SweepOptions) -> Self {
        Self::solid_of(input, layer, options).with_air(input, options)
    }

    /// [`compute`](Self::compute) without the air, for what reads the solid
    /// alone.
    fn solid_of(input: &ReviewInput<'_>, layer: usize, options: &SweepOptions) -> Self {
        let model = model_runs(input, layer);
        let solid = runs::open(&model, options.min_wall_px, input.height_px());
        let stray = if solid.len() == model.len() && solid == model {
            Vec::new()
        } else {
            runs::difference(&model, &solid)
        };
        let (solid_labels, solid_count) = label_runs(&solid, SOLID_CONNECTIVITY);
        Self {
            model,
            solid,
            stray,
            solid_labels,
            solid_count,
            ..Self::default()
        }
    }

    /// These pixels with their air opened and labelled.
    fn with_air(mut self, input: &ReviewInput<'_>, options: &SweepOptions) -> Self {
        let (width_px, height_px) = (input.width_px(), input.height_px());
        let air = runs::open(
            &runs::complement(&self.solid, width_px, height_px),
            options.drain_width_px,
            height_px,
        );
        let (air_labels, air_count) = label_runs(&air, AIR_CONNECTIVITY);
        let mut air_enclosed = vec![true; air_count];
        for (run, &label) in air.iter().zip(&air_labels) {
            if run.row == 0 || run.row + 1 == height_px || run.x0 == 0 || run.x1 == width_px {
                air_enclosed[label as usize] = false;
            }
        }
        self.air = air;
        self.air_labels = air_labels;
        self.air_count = air_count;
        self.air_enclosed = air_enclosed;
        self
    }

    /// The runs of solid region `region`. It scans every run of the layer:
    /// to visit every region, walk `solid` and `solid_labels` together once
    /// instead.
    pub fn solid_runs_of(&self, region: u32) -> impl Iterator<Item = &Run> + '_ {
        runs_of(&self.solid, &self.solid_labels, region)
    }

    /// The runs of air region `region`. It scans every run of the layer, as
    /// [`solid_runs_of`](Self::solid_runs_of) does.
    pub fn air_runs_of(&self, region: u32) -> impl Iterator<Item = &Run> + '_ {
        runs_of(&self.air, &self.air_labels, region)
    }

    /// The statistics of every region, without the links to other layers
    /// (`below_px` and `above_px` are 0).
    pub fn regions(&self, width_px: u32, height_px: u32) -> LayerRegions {
        LayerRegions {
            solid: measure_runs(
                &self.solid,
                &self.solid_labels,
                self.solid_count,
                width_px,
                height_px,
            ),
            air: measure_runs(
                &self.air,
                &self.air_labels,
                self.air_count,
                width_px,
                height_px,
            ),
            stray: Vec::new(),
            starts: Vec::new(),
        }
    }

    /// The part starts among `solid`, the layer's regions with their links
    /// below added up in `below_px`, over the layer `below`.
    fn starts(&self, solid: &[Region], below: Option<&Self>, min_link_px: u64) -> Vec<PartStart> {
        let mut slot = vec![u32::MAX; solid.len()];
        let mut starts: Vec<PartStart> = Vec::new();
        for (region, statistics) in solid.iter().enumerate() {
            if !link_holds(statistics.below_px, statistics.area_px, min_link_px) {
                slot[region] = starts.len() as u32;
                starts.push(PartStart {
                    region: region as u32,
                    runs: Vec::new(),
                    below: below.map_or_else(Vec::new, |below| below.around(&statistics.bbox)),
                });
            }
        }
        if !starts.is_empty() {
            for (run, &label) in self.solid.iter().zip(&self.solid_labels) {
                let at = slot[label as usize];
                if at != u32::MAX {
                    starts[at as usize].runs.push(*run);
                }
            }
        }
        starts
    }
}

impl LayerPixels {
    /// The stretches of this layer's solid within [`START_MARGIN_PX`] of
    /// `bbox`.
    fn around(&self, bbox: &PixelBox) -> Vec<Run> {
        let (top, bottom) = (
            bbox.y0.saturating_sub(START_MARGIN_PX),
            bbox.y1.saturating_add(START_MARGIN_PX),
        );
        let (left, right) = (
            bbox.x0.saturating_sub(START_MARGIN_PX),
            bbox.x1.saturating_add(START_MARGIN_PX),
        );
        let first = self.solid.partition_point(|run| run.row < top);
        self.solid[first..]
            .iter()
            .take_while(|run| run.row < bottom)
            .filter(|run| run.x0 < right && left < run.x1)
            .map(|run| Run::new(run.row, run.x0.max(left), run.x1.min(right)))
            .collect()
    }
}

fn runs_of<'a>(
    runs: &'a [Run],
    labels: &'a [u32],
    region: u32,
) -> impl Iterator<Item = &'a Run> + 'a {
    runs.iter()
        .zip(labels)
        .filter(move |(_, label)| **label == region)
        .map(|(run, _)| run)
}

/// The lit pixels of every leaf on `layer`. Leaves are rasterised together,
/// so overlapping leaves merge exactly as they do in the printer file, and a
/// stack that erases cancels the ones it was cut from.
fn model_runs(input: &ReviewInput<'_>, layer: usize) -> Vec<Run> {
    let contours: Vec<&ContourLayer> = input
        .leaves()
        .iter()
        .map(|leaf| &leaf.stack.layers()[layer])
        .filter(|contour| !contour.polygons().is_empty())
        .collect();
    if contours.is_empty() {
        return Vec::new();
    }
    runs::runs_from_raster(&rasterize_layers_binary(&contours, input.pixel_map()))
}

/// Labels the connected regions of a run list with `connectivity`. Returns
/// the region of every run, numbered in scan order, and the number of
/// regions.
pub fn label_runs(runs: &[Run], connectivity: Connectivity) -> (Vec<u32>, usize) {
    let reach = connectivity.reach();
    let mut sets = UnionFind::new(runs.len());
    let mut prev = 0..0;
    let mut i = 0;
    while i < runs.len() {
        let row = runs[i].row;
        let start = i;
        while i < runs.len() && runs[i].row == row {
            i += 1;
        }
        if !prev.is_empty() && runs[prev.start].row + 1 == row {
            let mut p = prev.start;
            for c in start..i {
                let run = runs[c];
                while p < prev.end && runs[p].x1 + reach <= run.x0 {
                    p += 1;
                }
                let mut q = p;
                while q < prev.end && runs[q].x0 < run.x1 + reach {
                    sets.union(c, q);
                    q += 1;
                }
            }
        }
        prev = start..i;
    }
    let mut labels = vec![0u32; runs.len()];
    let mut label_of_root = vec![u32::MAX; runs.len()];
    let mut count = 0u32;
    for (j, slot) in labels.iter_mut().enumerate() {
        let root = sets.find(j);
        if label_of_root[root] == u32::MAX {
            label_of_root[root] = count;
            count += 1;
        }
        *slot = label_of_root[root];
    }
    (labels, count as usize)
}

/// Area, box, centroid, inside pixel and border contact of every region
/// labelled by [`label_runs`] on a `width_px` x `height_px` screen. Links are
/// left at 0.
pub fn measure_runs(
    runs: &[Run],
    labels: &[u32],
    count: usize,
    width_px: u32,
    height_px: u32,
) -> Vec<Region> {
    // Twice the coordinate sums of the pixel centres, kept as integers so the
    // centroid is exact and independent of summation order.
    let mut sums = vec![(0u64, 0u64); count];
    let mut regions: Vec<Region> = Vec::with_capacity(count);
    for (run, &label) in runs.iter().zip(labels) {
        let label = label as usize;
        let len = u64::from(run.len());
        let border = run.row == 0 || run.row + 1 == height_px || run.x0 == 0 || run.x1 == width_px;
        if label == regions.len() {
            regions.push(Region {
                area_px: 0,
                bbox: PixelBox::of_run(run),
                centroid_px: [0.0, 0.0],
                inside_px: [run.x0, run.row],
                touches_border: false,
                below_px: 0,
                above_px: 0,
            });
        }
        let region = &mut regions[label];
        region.area_px += len;
        region.bbox.include(run);
        region.touches_border |= border;
        let sum = &mut sums[label];
        sum.0 += len * u64::from(run.x0 + run.x1);
        sum.1 += len * (2 * u64::from(run.row) + 1);
    }
    for (region, sum) in regions.iter_mut().zip(&sums) {
        let twice_area = 2.0 * region.area_px as f64;
        region.centroid_px = [sum.0 as f64 / twice_area, sum.1 as f64 / twice_area];
    }
    let mut best = vec![f64::INFINITY; count];
    for (run, &label) in runs.iter().zip(labels) {
        let region = &mut regions[label as usize];
        let [cx, cy] = region.centroid_px;
        let column = (cx.floor().max(0.0) as u32).clamp(run.x0, run.x1 - 1);
        let dx = f64::from(column) + 0.5 - cx;
        let dy = f64::from(run.row) + 0.5 - cy;
        let distance = dx.mul_add(dx, dy * dy);
        if distance < best[label as usize] {
            best[label as usize] = distance;
            region.inside_px = [column, run.row];
        }
    }
    regions
}

/// Sums the pixels of pairs of regions that repeat in `pairs`, sorted by
/// `(lower, upper)`.
fn merged_pairs(mut pairs: Vec<Overlap>) -> Vec<Overlap> {
    pairs.sort_unstable_by_key(|o| (o.lower, o.upper));
    pairs.dedup_by(|next, kept| {
        if next.lower == kept.lower && next.upper == kept.upper {
            kept.pixels += next.pixels;
            true
        } else {
            false
        }
    });
    pairs
}

/// The overlaps of the labelled solid runs of a lower and an upper layer.
fn link_solid(lower: &LayerPixels, upper: &LayerPixels) -> Vec<Overlap> {
    let mut pairs: Vec<Overlap> = Vec::new();
    for_each_overlap(&lower.solid, &upper.solid, |i, j, px| {
        let (a, b) = (lower.solid_labels[i], upper.solid_labels[j]);
        match pairs.last_mut() {
            Some(last) if last.lower == a && last.upper == b => last.pixels += u64::from(px),
            _ => pairs.push(Overlap {
                lower: a,
                upper: b,
                pixels: u64::from(px),
            }),
        }
    });
    merged_pairs(pairs)
}

/// The links of the opened air of a lower and an upper layer: the air both
/// share, opened again, so a link narrower than a drain is no link. Only
/// links to an enclosed region matter (open air is drained whatever lies
/// below it), and no square of air straddles two regions, so the rest is
/// left out before opening.
fn link_air(
    lower: &LayerPixels,
    upper: &LayerPixels,
    drain_width_px: u32,
    height_px: u32,
) -> Vec<Overlap> {
    let mut shared: Vec<Run> = Vec::new();
    for_each_overlap(&lower.air, &upper.air, |i, j, _| {
        if lower.air_enclosed[lower.air_labels[i] as usize]
            || upper.air_enclosed[upper.air_labels[j] as usize]
        {
            let (a, b) = (lower.air[i], upper.air[j]);
            shared.push(Run::new(a.row, a.x0.max(b.x0), a.x1.min(b.x1)));
        }
    });
    let shared = runs::open(&shared, drain_width_px, height_px);
    // Each opened run lies within one run of each layer.
    let mut pairs: Vec<Overlap> = vec![
        Overlap {
            lower: 0,
            upper: 0,
            pixels: 0
        };
        shared.len()
    ];
    for_each_overlap(&shared, &lower.air, |i, j, px| {
        pairs[i].lower = lower.air_labels[j];
        pairs[i].pixels = u64::from(px);
    });
    for_each_overlap(&shared, &upper.air, |i, j, _| {
        pairs[i].upper = upper.air_labels[j];
    });
    merged_pairs(pairs)
}

/// What watches a sweep: the token that stops it, and who hears of each
/// layer done, called from any thread with the number of layers done so far.
#[derive(Clone, Copy)]
pub struct SweepWatch<'a> {
    /// Stops the sweep at the next layer of every thread.
    pub cancel: &'a CancelToken,
    /// Hears every finished layer.
    pub on_layer: &'a (dyn Fn(usize) + Sync),
}

impl std::fmt::Debug for SweepWatch<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SweepWatch").finish_non_exhaustive()
    }
}

/// Every layer's tables and the links between consecutive layers.
#[derive(Clone, Debug, Default)]
pub struct PlateRegions {
    width_px: u32,
    height_px: u32,
    layers: Vec<LayerRegions>,
    /// `links[k]` joins layer `k - 1` to layer `k`; `links[0]` is empty.
    links: Vec<LayerLinks>,
    elapsed: Duration,
}

impl PlateRegions {
    /// Sweeps every layer of `input`, with nobody watching.
    pub fn compute(input: &ReviewInput<'_>, options: &SweepOptions) -> Self {
        let cancel = CancelToken::new();
        let watch = SweepWatch {
            cancel: &cancel,
            on_layer: &|_| {},
        };
        Self::sweep(input, options, watch).expect("a token nobody holds is never tripped")
    }

    /// Sweeps every layer of `input`, in parallel, telling `watch` of every
    /// layer done. `None` when `watch` stopped it.
    pub fn sweep(
        input: &ReviewInput<'_>,
        options: &SweepOptions,
        watch: SweepWatch<'_>,
    ) -> Option<Self> {
        let started = Instant::now();
        let count = input.layer_count();
        let size = chunk_size(count, rayon::current_num_threads());
        let chunks: Vec<Range<usize>> = (0..count)
            .step_by(size)
            .map(|start| start..(start + size).min(count))
            .collect();
        let done = AtomicUsize::new(0);
        let min_link_px = options.min_link_px(input);
        let swept: Vec<Vec<(LayerRegions, LayerLinks)>> = chunks
            .into_par_iter()
            .map(|chunk| sweep_chunk(input, options, min_link_px, chunk, watch, &done))
            .collect::<Option<_>>()?;
        let mut layers: Vec<LayerRegions> = Vec::with_capacity(count);
        let mut links: Vec<LayerLinks> = Vec::with_capacity(count);
        for (layer, layer_links) in swept.into_iter().flatten() {
            layers.push(layer);
            links.push(layer_links);
        }
        for upper in 1..layers.len() {
            let lower = &mut layers[upper - 1];
            for overlap in &links[upper].solid {
                lower.solid[overlap.lower as usize].above_px += overlap.pixels;
            }
            for overlap in &links[upper].air {
                lower.air[overlap.lower as usize].above_px += overlap.pixels;
            }
        }
        Some(Self {
            width_px: input.width_px(),
            height_px: input.height_px(),
            layers,
            links,
            elapsed: started.elapsed(),
        })
    }

    /// The screen width in pixels.
    #[inline]
    pub fn width_px(&self) -> u32 {
        self.width_px
    }

    /// The screen height in pixels.
    #[inline]
    pub fn height_px(&self) -> u32 {
        self.height_px
    }

    /// Every layer's tables, from the plate up.
    #[inline]
    pub fn layers(&self) -> &[LayerRegions] {
        &self.layers
    }

    /// The tables of layer `layer`.
    ///
    /// # Panics
    ///
    /// When `layer` is out of range.
    #[inline]
    pub fn layer(&self, layer: usize) -> &LayerRegions {
        &self.layers[layer]
    }

    /// The overlaps between the solid regions of layer `layer - 1`
    /// ([`Overlap::lower`]) and layer `layer` ([`Overlap::upper`]). Empty for
    /// layer 0.
    ///
    /// # Panics
    ///
    /// When `layer` is out of range.
    #[inline]
    pub fn solid_links(&self, layer: usize) -> &[Overlap] {
        &self.links[layer].solid
    }

    /// The links between the air regions of layer `layer - 1` and layer
    /// `layer`. Empty for layer 0.
    ///
    /// # Panics
    ///
    /// When `layer` is out of range.
    #[inline]
    pub fn air_links(&self, layer: usize) -> &[Overlap] {
        &self.links[layer].air
    }

    /// Groups the solid regions of all layers: two regions of consecutive
    /// layers join when `keep(upper_layer, overlap)` accepts their overlap.
    /// `|_, _| true` gives the printed bodies.
    pub fn group_solid(&self, keep: impl FnMut(usize, &Overlap) -> bool) -> Groups {
        self.group(|layer| layer.solid.len(), |links| &links.solid, keep)
    }

    /// Groups the air regions of all layers, as
    /// [`group_solid`](Self::group_solid) does for solid.
    pub fn group_air(&self, keep: impl FnMut(usize, &Overlap) -> bool) -> Groups {
        self.group(|layer| layer.air.len(), |links| &links.air, keep)
    }

    fn group(
        &self,
        len: impl Fn(&LayerRegions) -> usize,
        links_of: impl Fn(&LayerLinks) -> &[Overlap],
        mut keep: impl FnMut(usize, &Overlap) -> bool,
    ) -> Groups {
        let mut offsets = Vec::with_capacity(self.layers.len() + 1);
        let mut total = 0;
        offsets.push(0);
        for layer in &self.layers {
            total += len(layer);
            offsets.push(total);
        }
        let mut sets = UnionFind::new(total);
        for (layer, layer_links) in self.links.iter().enumerate().skip(1) {
            for overlap in links_of(layer_links) {
                if keep(layer, overlap) {
                    sets.union(
                        offsets[layer - 1] + overlap.lower as usize,
                        offsets[layer] + overlap.upper as usize,
                    );
                }
            }
        }
        Groups::from_union_find(offsets, &mut sets)
    }

    /// How long the sweep took.
    #[inline]
    pub fn elapsed(&self) -> Duration {
        self.elapsed
    }
}

/// The layers a chunk walks: enough chunks for every thread to stay busy to
/// the end, and few enough that rasterising the layer below each one again
/// stays cheap.
fn chunk_size(layers: usize, threads: usize) -> usize {
    (layers / (threads.max(1) * 8)).clamp(4, 32)
}

/// Sweeps the layers of `chunk`, the layer below its first one rasterised
/// again. `None` when the watch stopped it.
fn sweep_chunk(
    input: &ReviewInput<'_>,
    options: &SweepOptions,
    min_link_px: u64,
    chunk: Range<usize>,
    watch: SweepWatch<'_>,
    done: &AtomicUsize,
) -> Option<Vec<(LayerRegions, LayerLinks)>> {
    let (width_px, height_px) = (input.width_px(), input.height_px());
    let mut below = chunk
        .start
        .checked_sub(1)
        .map(|layer| LayerPixels::compute(input, layer, options));
    let mut out = Vec::with_capacity(chunk.len());
    for layer in chunk {
        if watch.cancel.is_cancelled() {
            return None;
        }
        let pixels = LayerPixels::compute(input, layer, options);
        let mut regions = pixels.regions(width_px, height_px);
        let links = below
            .as_ref()
            .map_or_else(LayerLinks::default, |below| LayerLinks {
                solid: link_solid(below, &pixels),
                air: link_air(below, &pixels, options.drain_width_px, height_px),
            });
        for overlap in &links.solid {
            regions.solid[overlap.upper as usize].below_px += overlap.pixels;
        }
        for overlap in &links.air {
            regions.air[overlap.upper as usize].below_px += overlap.pixels;
        }
        if layer > 0 {
            regions.starts = pixels.starts(&regions.solid, below.as_ref(), min_link_px);
        }
        regions.stray.clone_from(&pixels.stray);
        out.push((regions, links));
        below = Some(pixels);
        (watch.on_layer)(done.fetch_add(1, Ordering::Relaxed) + 1);
    }
    Some(out)
}

/// One set of regions to read the pixels of: their layer, who asked (any
/// number, kept with what comes back) and their indices.
pub(crate) type WantedRegions = (usize, usize, u32);

/// What [`read_layers`] needs of a layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LayerRead {
    /// Its solid.
    Solid,
    /// Its solid and its air.
    All,
}

/// Calls `read` with the pixels of every layer `wanted` names and the
/// regions each asker wants there: each layer is rasterised once, in
/// parallel. What `read` gives back comes back with its asker, lowest layer
/// first.
pub(crate) fn read_layers<T: Send>(
    input: &ReviewInput<'_>,
    options: &SweepOptions,
    needs: LayerRead,
    mut wanted: Vec<WantedRegions>,
    askers: usize,
    read: impl Fn(&LayerPixels, &[WantedRegions]) -> T + Sync,
) -> Vec<Vec<(usize, T)>> {
    wanted.sort_unstable();
    let layers: Vec<&[WantedRegions]> = wanted.chunk_by(|a, b| a.0 == b.0).collect();
    let found: Vec<Vec<(usize, usize, T)>> = layers
        .par_iter()
        .map(|members| {
            let layer = members[0].0;
            let pixels = match needs {
                LayerRead::Solid => LayerPixels::solid_of(input, layer, options),
                LayerRead::All => LayerPixels::compute(input, layer, options),
            };
            members
                .chunk_by(|a, b| a.1 == b.1)
                .map(|regions| {
                    let asker = regions[0].1;
                    (asker, layer, read(&pixels, regions))
                })
                .collect()
        })
        .collect();
    let mut out: Vec<Vec<(usize, T)>> = Vec::with_capacity(askers);
    out.resize_with(askers, Vec::new);
    for (asker, layer, value) in found.into_iter().flatten() {
        out[asker].push((layer, value));
    }
    out
}

/// The runs among `runs`, labelled by `labels`, of the regions `wanted`
/// names.
pub(crate) fn runs_in(runs: &[Run], labels: &[u32], wanted: &[WantedRegions]) -> Vec<Run> {
    runs.iter()
        .zip(labels)
        .filter(|(_, label)| wanted.iter().any(|w| w.2 == **label))
        .map(|(run, _)| *run)
        .collect()
}

/// A [`RegionRef`] as one [`WantedRegions`] entry of `asker`.
pub(crate) const fn wanted(member: &RegionRef, asker: usize) -> WantedRegions {
    (member.layer, asker, member.index)
}

#[cfg(test)]
#[path = "tests_regions.rs"]
mod tests_regions;

#[cfg(test)]
#[path = "tests_reference.rs"]
mod tests_reference;
