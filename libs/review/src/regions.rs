/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Per-layer regions and their links across layers: the machinery every
//! check shares, computed once per review by [`PlateRegions::compute`].
//!
//! # What is computed
//!
//! Each layer is rasterised at the printer's own resolution with the
//! slicer's binary rasteriser (the same pixels the printer file gets,
//! without anti-aliasing), once for the model leaves and once for the
//! support leaves, and kept as row runs ([`LayerPixels`]). Then:
//!
//! - **Solid regions**: the connected parts of the merged plate (model and
//!   support together), with **8-connectivity**: pixels that touch by an edge
//!   or a corner belong together. Each [`Region`] carries its area, pixel
//!   bounding box, centroid, a pixel inside it, whether it reaches the screen
//!   edge, and how many of its pixels are model and how many support.
//! - **Air regions**: the connected parts of the complement, with
//!   **4-connectivity**: air pixels belong together only through an edge.
//!   This is the dual of the solid rule: a solid ring closed only through
//!   corners still seals the air inside it, so a region is never both
//!   walled in and leaking. An air region that reaches the screen edge is
//!   *open air*; every other air region is enclosed within its layer.
//! - **Links**: for each pair of consecutive layers, how many pixels every
//!   region of the lower layer shares with every region of the same kind in
//!   the upper layer ([`Overlap`]). Pixels are linked across layers only
//!   straight up (same `x`, `y`), never diagonally.
//!
//! [`PlateRegions::group_solid`] and [`PlateRegions::group_air`] join regions
//! across layers through the links (union-find), for bodies and cavities
//! that span many layers.
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

use std::time::{Duration, Instant};

use rayon::prelude::*;
use sc_sl1cer::contour::{ContourLayer, Polygon};
use sc_sl1cer::raster::rasterize_layer_binary;

use crate::input::{ReviewInput, Role};
use crate::runs::{self, Run, for_each_overlap};
use crate::union_find::{Groups, UnionFind};

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
    /// Pixels that are model. Solid only; 0 for air.
    pub model_px: u64,
    /// Pixels that are support. Solid only; 0 for air. Model and support may
    /// share pixels (support tips enter the model), so `model_px +
    /// support_px` can exceed `area_px`.
    pub support_px: u64,
    /// Pixels with a pixel of the same kind straight below, in the previous
    /// layer: for solid, the pixels resting on cured resin. Always 0 on
    /// layer 0, which rests on the plate.
    pub below_px: u64,
    /// Pixels with a pixel of the same kind straight above, in the next
    /// layer. Always 0 on the last layer.
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

/// The regions of one layer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayerRegions {
    /// Solid regions of the merged plate, in scan order.
    pub solid: Vec<Region>,
    /// Air regions, in scan order. There is always at least one unless the
    /// layer is solid from edge to edge.
    pub air: Vec<Region>,
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
    /// Air regions below and above.
    pub air: Vec<Overlap>,
}

/// The pixels of one layer as row runs, with the region of every run.
///
/// [`PlateRegions`] keeps only the statistics; a check that needs the pixels
/// of a layer recomputes them with [`LayerPixels::compute`], which gives the
/// same region indices.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayerPixels {
    /// Model pixels.
    pub model: Vec<Run>,
    /// Support pixels.
    pub support: Vec<Run>,
    /// Solid pixels: model and support merged.
    pub solid: Vec<Run>,
    /// The solid region of each run of `solid`.
    pub solid_labels: Vec<u32>,
    /// Number of solid regions.
    pub solid_count: usize,
    /// Air pixels, the complement of `solid` on the screen.
    pub air: Vec<Run>,
    /// The air region of each run of `air`.
    pub air_labels: Vec<u32>,
    /// Number of air regions.
    pub air_count: usize,
}

impl LayerPixels {
    /// Rasterises and labels layer `layer` of `input`.
    ///
    /// # Panics
    ///
    /// When `layer` is out of range.
    pub fn compute(input: &ReviewInput<'_>, layer: usize) -> Self {
        let _clock = crate::clock::scope("pixels.compute");
        let model = role_runs(input, layer, Role::Model);
        let support = role_runs(input, layer, Role::Support);
        let solid = runs::union(&model, &support);
        let air = runs::complement(&solid, input.width_px(), input.height_px());
        let (solid_labels, solid_count) = label_runs(&solid, SOLID_CONNECTIVITY);
        let (air_labels, air_count) = label_runs(&air, AIR_CONNECTIVITY);
        Self {
            model,
            support,
            solid,
            solid_labels,
            solid_count,
            air,
            air_labels,
            air_count,
        }
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
        let _clock = crate::clock::scope("regions.measure");
        let mut solid = measure_runs(
            &self.solid,
            &self.solid_labels,
            self.solid_count,
            width_px,
            height_px,
        );
        for_each_overlap(&self.solid, &self.model, |i, _, px| {
            solid[self.solid_labels[i] as usize].model_px += u64::from(px);
        });
        for_each_overlap(&self.solid, &self.support, |i, _, px| {
            solid[self.solid_labels[i] as usize].support_px += u64::from(px);
        });
        let air = measure_runs(
            &self.air,
            &self.air_labels,
            self.air_count,
            width_px,
            height_px,
        );
        LayerRegions { solid, air }
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

/// The lit pixels of every `role` leaf on `layer`. Leaves of one role are
/// rasterised together, so overlapping leaves merge exactly as they do in
/// the printer file.
fn role_runs(input: &ReviewInput<'_>, layer: usize, role: Role) -> Vec<Run> {
    let _clock = crate::clock::scope("pixels.role_runs");
    let contours: Vec<&ContourLayer> = input
        .leaves()
        .iter()
        .filter(|leaf| leaf.role == role)
        .map(|leaf| &leaf.stack.layers()[layer])
        .filter(|contour| !contour.polygons().is_empty())
        .collect();
    #[cfg(feature = "stage-clock")]
    for contour in &contours {
        crate::clock::count("items.raster_polygons", contour.polygons().len() as u64);
        crate::clock::count(
            "items.raster_points",
            contour
                .polygons()
                .iter()
                .map(|p| p.outer().len() + p.holes().iter().map(|h| h.len()).sum::<usize>())
                .sum::<usize>() as u64,
        );
    }
    match contours.as_slice() {
        [] => Vec::new(),
        [only] => runs::runs_from_raster(&rasterize_layer_binary(only, input.pixel_map())),
        many => {
            let polygons: Vec<Polygon> = many
                .iter()
                .flat_map(|contour| contour.polygons().iter().cloned())
                .collect();
            let merged = ContourLayer::new(many[0].z_mm(), polygons);
            runs::runs_from_raster(&rasterize_layer_binary(&merged, input.pixel_map()))
        }
    }
}

/// Labels the connected regions of a run list with `connectivity`. Returns
/// the region of every run, numbered in scan order, and the number of
/// regions. [`LayerPixels::compute`] uses it on the merged solid and on the
/// air; a check can use it on one role alone (`LayerPixels::model`).
pub fn label_runs(runs: &[Run], connectivity: Connectivity) -> (Vec<u32>, usize) {
    let _clock = crate::clock::scope("runs.label");
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
/// labelled by [`label_runs`] on a `width_px` x `height_px` screen. Role
/// counts and links are left at 0.
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
                model_px: 0,
                support_px: 0,
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

/// The overlaps of the labelled runs of a lower and an upper layer, merged
/// per region pair and sorted by `(lower, upper)`.
fn link(lower: &[Run], lower_labels: &[u32], upper: &[Run], upper_labels: &[u32]) -> Vec<Overlap> {
    let mut pairs: Vec<Overlap> = Vec::new();
    for_each_overlap(lower, upper, |i, j, px| {
        let (a, b) = (lower_labels[i], upper_labels[j]);
        match pairs.last_mut() {
            Some(last) if last.lower == a && last.upper == b => last.pixels += u64::from(px),
            _ => pairs.push(Overlap {
                lower: a,
                upper: b,
                pixels: u64::from(px),
            }),
        }
    });
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

fn link_layers(lower: &LayerPixels, upper: &LayerPixels) -> LayerLinks {
    let _clock = crate::clock::scope("regions.link_layers");
    LayerLinks {
        solid: link(
            &lower.solid,
            &lower.solid_labels,
            &upper.solid,
            &upper.solid_labels,
        ),
        air: link(&lower.air, &lower.air_labels, &upper.air, &upper.air_labels),
    }
}

/// Adds the links between `lower` and `upper` to their regions' `above_px`
/// and `below_px`.
fn apply_links(lower: &mut LayerRegions, upper: &mut LayerRegions, links: &LayerLinks) {
    let _clock = crate::clock::scope("regions.apply_links");
    for o in &links.solid {
        lower.solid[o.lower as usize].above_px += o.pixels;
        upper.solid[o.upper as usize].below_px += o.pixels;
    }
    for o in &links.air {
        lower.air[o.lower as usize].above_px += o.pixels;
        upper.air[o.upper as usize].below_px += o.pixels;
    }
}

/// Every layer's regions and the links between consecutive layers.
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
    /// Rasterises, labels and links every layer of `input`.
    ///
    /// Layers are processed in parallel, a batch of a few per thread at a
    /// time, so only one batch of pixels is held in memory at once; the
    /// result keeps statistics only. The result does not depend on the
    /// number of threads.
    pub fn compute(input: &ReviewInput<'_>) -> Self {
        let _clock = crate::clock::scope("regions.compute");
        let started = Instant::now();
        let (width_px, height_px) = (input.width_px(), input.height_px());
        let count = input.layer_count();
        let batch = (rayon::current_num_threads() * 4).max(8);
        let mut layers: Vec<LayerRegions> = Vec::with_capacity(count);
        let mut links: Vec<LayerLinks> = Vec::with_capacity(count);
        let mut previous: Option<LayerPixels> = None;
        let mut start = 0;
        while start < count {
            let end = (start + batch).min(count);
            let (mut pixels, mut regions): (Vec<LayerPixels>, Vec<LayerRegions>) = (start..end)
                .into_par_iter()
                .map(|layer| {
                    let pixels = LayerPixels::compute(input, layer);
                    let regions = pixels.regions(width_px, height_px);
                    (pixels, regions)
                })
                .unzip();
            let batch_links: Vec<LayerLinks> = (0..pixels.len())
                .into_par_iter()
                .map(|i| {
                    let lower = if i == 0 {
                        previous.as_ref()
                    } else {
                        Some(&pixels[i - 1])
                    };
                    lower.map_or_else(LayerLinks::default, |lower| link_layers(lower, &pixels[i]))
                })
                .collect();
            for (i, layer_links) in batch_links.iter().enumerate() {
                if i == 0 {
                    if let Some(lower) = layers.last_mut() {
                        apply_links(lower, &mut regions[0], layer_links);
                    }
                } else {
                    let (below, above) = regions.split_at_mut(i);
                    apply_links(&mut below[i - 1], &mut above[0], layer_links);
                }
            }
            layers.append(&mut regions);
            links.extend(batch_links);
            previous = pixels.pop();
            start = end;
        }
        Self {
            width_px,
            height_px,
            layers,
            links,
            elapsed: started.elapsed(),
        }
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

    /// Every layer's regions, from the plate up.
    #[inline]
    pub fn layers(&self) -> &[LayerRegions] {
        &self.layers
    }

    /// The regions of layer `layer`.
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

    /// The overlaps between the air regions of layer `layer - 1` and layer
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
    /// [`group_solid`](Self::group_solid) does for solid. With `|_, _| true`
    /// a group is a cavity, or open air when one of its regions touches the
    /// screen edge or lies on the last layer (open towards the resin above,
    /// which is not represented).
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

    /// How long [`compute`](Self::compute) took.
    #[inline]
    pub fn elapsed(&self) -> Duration {
        self.elapsed
    }
}

#[cfg(test)]
#[path = "tests_regions.rs"]
mod tests_regions;

#[cfg(test)]
#[path = "tests_reference.rs"]
mod tests_reference;
