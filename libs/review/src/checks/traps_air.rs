/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The air map of the resin-trap check: every layer's air *opened* by the
//! drain width, split into regions, measured, and linked to the next layer.
//!
//! The shared [`PlateRegions`](crate::regions::PlateRegions) joins air
//! through a single pixel, within a layer and between layers, so a hairline
//! crack would drain a sealed cavity. This map keeps only air that resin can
//! flow through: the pixels that some square of air, `width` pixels on a
//! side, covers (a morphological opening). A gap narrower than the square
//! closes, whichever way it runs; anything wider keeps its exact shape. The
//! air two layers share is opened the same way before it links them.
//!
//! Besides area and position, every enclosed region gets the roles of the
//! solid around it ([`Sides`]): its walls on all four sides, the solid
//! straight below it (its floor) and straight above it (its lid).

use rayon::prelude::*;

use crate::checks::thin::{dilate_square, intersection};
use crate::fill::FillLayerRuns;
use crate::input::ReviewInput;
use crate::regions::{
    AIR_CONNECTIVITY, LayerPixels, Overlap, PixelBox, Region, label_runs, measure_runs,
};
use crate::runs::{Run, for_each_overlap};
use crate::union_find::{Groups, RegionRef, UnionFind};

/// Solid pixels by role. A pixel can be both model and support.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Roles {
    /// Solid pixels counted.
    pub(super) px: u64,
    /// Those that are model.
    pub(super) model: u64,
    /// Those that are support.
    pub(super) support: u64,
}

impl Roles {
    /// Adds `other` to these counts.
    pub(super) fn add(&mut self, other: Self) {
        self.px += other.px;
        self.model += other.model;
        self.support += other.support;
    }

    /// Whether at least half of the pixels are support (and there is one).
    pub(super) fn mostly_support(&self) -> bool {
        self.support > 0 && 2 * self.support >= self.px
    }
}

/// The solid around one air region, by role. Walls are counted once per
/// side of the region they touch, so a wall pixel beside two air pixels
/// counts twice: the counts measure the length of the boundary.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Sides {
    /// Every solid pixel beside the region within its layer, left, right,
    /// in front or behind.
    pub(super) walls: Roles,
    /// The part of `walls` that surrounds the region: everything except the
    /// islands standing inside it.
    pub(super) outer: Roles,
    /// The solid straight below the region, in the previous layer.
    pub(super) floor: Roles,
    /// The solid straight above the region, in the next layer.
    pub(super) lid: Roles,
}

/// The opened air regions of one layer. Only enclosed regions get their
/// [`Sides`] measured; open air keeps the default.
#[derive(Clone, Debug, Default)]
pub(super) struct AirLayer {
    /// The regions, in scan order (`touches_border` marks the open air).
    pub(super) regions: Vec<Region>,
    /// The solid around each region.
    pub(super) sides: Vec<Sides>,
}

/// Every layer's opened air and the links between consecutive layers.
#[derive(Clone, Debug, Default)]
pub(super) struct AirMap {
    /// The layers, from the plate up.
    pub(super) layers: Vec<AirLayer>,
    /// `links[k]` joins layer `k - 1` to layer `k`, sorted by `(lower,
    /// upper)`; `links[0]` is empty. Each counts the opened air the two
    /// regions share.
    pub(super) links: Vec<Vec<Overlap>>,
}

impl AirMap {
    /// Rasterises, opens, labels and links every layer of `input`, with an
    /// opening `width` pixels wide. Layers are processed in parallel, a batch
    /// at a time, so only one batch of pixels is held at once.
    pub(super) fn compute(input: &ReviewInput<'_>, width: u32) -> Self {
        let _clock = crate::clock::scope("traps.air_map");
        let count = input.layer_count();
        let batch = (rayon::current_num_threads() * 4).max(8);
        let mut layers: Vec<AirLayer> = Vec::with_capacity(count);
        let mut links: Vec<Vec<Overlap>> = Vec::with_capacity(count);
        let mut previous: Option<AirPixels> = None;
        let mut start = 0;
        while start < count {
            let end = (start + batch).min(count);
            let (mut pixels, mut batch_layers): (Vec<AirPixels>, Vec<AirLayer>) = (start..end)
                .into_par_iter()
                .map(|layer| {
                    let pixels =
                        AirPixels::from_pixels(LayerPixels::compute(input, layer), width, input);
                    let air = pixels.layer(input);
                    (pixels, air)
                })
                .unzip();
            let pairs: Vec<Option<Pair>> = (0..pixels.len())
                .into_par_iter()
                .map(|i| {
                    let lower = if i == 0 {
                        previous.as_ref()
                    } else {
                        Some(&pixels[i - 1])
                    };
                    lower.map(|lower| Pair::compute(lower, &pixels[i], width, input.height_px()))
                })
                .collect();
            for (i, pair) in pairs.into_iter().enumerate() {
                let Some(pair) = pair else {
                    links.push(Vec::new());
                    continue;
                };
                for (side, floor) in batch_layers[i].sides.iter_mut().zip(pair.upper_floor) {
                    side.floor = floor;
                }
                let lower = if i == 0 {
                    layers.last_mut().expect("a previous batch")
                } else {
                    &mut batch_layers[i - 1]
                };
                for (side, lid) in lower.sides.iter_mut().zip(pair.lower_lid) {
                    side.lid = lid;
                }
                links.push(pair.links);
            }
            layers.append(&mut batch_layers);
            previous = pixels.pop();
            start = end;
        }
        Self { layers, links }
    }

    /// Groups the regions of all layers: two regions of consecutive layers
    /// join when `keep(upper_layer, link)` accepts their link.
    pub(super) fn group(&self, mut keep: impl FnMut(usize, &Overlap) -> bool) -> Groups {
        let mut offsets = Vec::with_capacity(self.layers.len() + 1);
        let mut total = 0;
        offsets.push(0);
        for layer in &self.layers {
            total += layer.regions.len();
            offsets.push(total);
        }
        let mut sets = UnionFind::new(total);
        for (layer, links) in self.links.iter().enumerate().skip(1) {
            for link in links {
                if keep(layer, link) {
                    sets.union(
                        offsets[layer - 1] + link.lower as usize,
                        offsets[layer] + link.upper as usize,
                    );
                }
            }
        }
        Groups::from_union_find(offsets, &mut sets)
    }
}

/// One layer's pixels and its opened, labelled air.
struct AirPixels {
    /// The layer's model, support and solid runs.
    pixels: LayerPixels,
    /// The opened air.
    air: Vec<Run>,
    /// The region of each run of `air`.
    labels: Vec<u32>,
    /// Number of regions.
    count: usize,
    /// Whether each region is enclosed within the layer (does not reach the
    /// screen edge).
    enclosed: Vec<bool>,
    /// The runs of the enclosed regions, a subset of `air`.
    inner: Vec<Run>,
    /// The region of each run of `inner`.
    inner_labels: Vec<u32>,
}

impl AirPixels {
    fn from_pixels(pixels: LayerPixels, width: u32, input: &ReviewInput<'_>) -> Self {
        let _clock = crate::clock::scope("traps.air_pixels");
        let (width_px, height_px) = (input.width_px(), input.height_px());
        let air = open(&pixels.air, width, height_px);
        let (labels, count) = label_runs(&air, AIR_CONNECTIVITY);
        let mut enclosed = vec![true; count];
        for (run, &label) in air.iter().zip(&labels) {
            if run.row == 0 || run.row + 1 == height_px || run.x0 == 0 || run.x1 == width_px {
                enclosed[label as usize] = false;
            }
        }
        let (inner, inner_labels): (Vec<Run>, Vec<u32>) = air
            .iter()
            .zip(&labels)
            .filter(|(_, label)| enclosed[**label as usize])
            .map(|(run, label)| (*run, *label))
            .unzip();
        Self {
            pixels,
            air,
            labels,
            count,
            enclosed,
            inner,
            inner_labels,
        }
    }

    /// The region of the opened air at pixel `(x, row)`, if it is opened
    /// air.
    fn region_at(&self, row: u32, x: u32) -> Option<u32> {
        let at = self
            .air
            .partition_point(|run| (run.row, run.x1) <= (row, x));
        self.air
            .get(at)
            .filter(|run| run.row == row && run.x0 <= x)
            .map(|_| self.labels[at])
    }

    /// The regions and the walls of the enclosed ones.
    fn layer(&self, input: &ReviewInput<'_>) -> AirLayer {
        let _clock = crate::clock::scope("traps.air_layer");
        let (width_px, height_px) = (input.width_px(), input.height_px());
        let regions = measure_runs(&self.air, &self.labels, self.count, width_px, height_px);
        let mut sides = vec![Sides::default(); self.count];
        if !self.inner.is_empty() {
            self.measure_walls(&regions, &mut sides, width_px, height_px);
        }
        AirLayer { regions, sides }
    }

    /// The walls of every enclosed region: the solid pixels beside each of
    /// its runs, left and right on its row and over its length on the rows
    /// before and after. A wall belongs to the outer wall unless its solid
    /// region lies strictly inside the air region's box: an island inside
    /// an enclosed region is surrounded by it, so every straight line out of
    /// the island crosses the region first.
    fn measure_walls(
        &self,
        regions: &[Region],
        sides: &mut [Sides],
        width_px: u32,
        height_px: u32,
    ) {
        let _clock = crate::clock::scope("traps.walls");
        let pixels = &self.pixels;
        let solid_boxes: Vec<PixelBox> = measure_runs(
            &pixels.solid,
            &pixels.solid_labels,
            pixels.solid_count,
            width_px,
            height_px,
        )
        .iter()
        .map(|region| region.bbox)
        .collect();
        for (run, &label) in self.inner.iter().zip(&self.inner_labels) {
            let region = &regions[label as usize];
            let side = &mut sides[label as usize];
            let mut beside = |row: u32, x0: u32, x1: u32| {
                self.add_walls(&solid_boxes, &region.bbox, side, row, x0, x1);
            };
            if run.x0 > 0 {
                beside(run.row, run.x0 - 1, run.x0);
            }
            if run.x1 < width_px {
                beside(run.row, run.x1, run.x1 + 1);
            }
            if run.row > 0 {
                beside(run.row - 1, run.x0, run.x1);
            }
            if run.row + 1 < height_px {
                beside(run.row + 1, run.x0, run.x1);
            }
        }
    }

    /// Adds the solid pixels of `x0..x1` on `row` to the walls of a region
    /// whose box is `bbox`.
    fn add_walls(
        &self,
        solid_boxes: &[PixelBox],
        bbox: &PixelBox,
        side: &mut Sides,
        row: u32,
        x0: u32,
        x1: u32,
    ) {
        let pixels = &self.pixels;
        let first = pixels
            .solid
            .partition_point(|run| (run.row, run.x1) <= (row, x0));
        for (index, run) in pixels.solid.iter().enumerate().skip(first) {
            if run.row != row || run.x0 >= x1 {
                break;
            }
            let (a, b) = (run.x0.max(x0), run.x1.min(x1));
            let roles = Roles {
                px: u64::from(b - a),
                model: coverage(&pixels.model, row, a, b),
                support: coverage(&pixels.support, row, a, b),
            };
            side.walls.add(roles);
            if !strictly_inside(&solid_boxes[pixels.solid_labels[index] as usize], bbox) {
                side.outer.add(roles);
            }
        }
    }
}

/// Whether `inner` lies strictly inside `outer`, touching none of its
/// edges.
fn strictly_inside(inner: &PixelBox, outer: &PixelBox) -> bool {
    inner.x0 > outer.x0 && inner.x1 < outer.x1 && inner.y0 > outer.y0 && inner.y1 < outer.y1
}

/// The pixels of the run list `runs` within `x0..x1` of `row`.
fn coverage(runs: &[Run], row: u32, x0: u32, x1: u32) -> u64 {
    let first = runs.partition_point(|run| (run.row, run.x1) <= (row, x0));
    runs[first..]
        .iter()
        .take_while(|run| run.row == row && run.x0 < x1)
        .map(|run| u64::from(run.x1.min(x1) - run.x0.max(x0)))
        .sum()
}

/// What two consecutive layers give each other.
struct Pair {
    /// The links between their regions.
    links: Vec<Overlap>,
    /// The solid of the upper layer straight above each lower region.
    lower_lid: Vec<Roles>,
    /// The solid of the lower layer straight below each upper region.
    upper_floor: Vec<Roles>,
}

impl Pair {
    fn compute(lower: &AirPixels, upper: &AirPixels, width: u32, height_px: u32) -> Self {
        let _clock = crate::clock::scope("traps.pair");
        // The air both layers share, opened: a link narrower than a drain is
        // no link. Only links to an enclosed region matter (open air is
        // drained whatever lies below it), and no square of air straddles
        // two regions, so the rest is left out before opening.
        let mut shared: Vec<Run> = Vec::new();
        for_each_overlap(&lower.air, &upper.air, |i, j, _| {
            if lower.enclosed[lower.labels[i] as usize] || upper.enclosed[upper.labels[j] as usize]
            {
                let (a, b) = (lower.air[i], upper.air[j]);
                shared.push(Run::new(a.row, a.x0.max(b.x0), a.x1.min(b.x1)));
            }
        });
        let shared = open(&shared, width, height_px);
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
            pairs[i].lower = lower.labels[j];
            pairs[i].pixels = u64::from(px);
        });
        for_each_overlap(&shared, &upper.air, |i, j, _| {
            pairs[i].upper = upper.labels[j];
        });
        pairs.sort_unstable_by_key(|pair| (pair.lower, pair.upper));
        pairs.dedup_by(|next, kept| {
            if next.lower == kept.lower && next.upper == kept.upper {
                kept.pixels += next.pixels;
                true
            } else {
                false
            }
        });
        Self {
            links: pairs,
            lower_lid: solid_beyond(lower, &upper.pixels),
            upper_floor: solid_beyond(upper, &lower.pixels),
        }
    }
}

/// The solid of `other`, an adjacent layer, straight beyond each enclosed
/// region of `air` (open air keeps zero).
fn solid_beyond(air: &AirPixels, other: &LayerPixels) -> Vec<Roles> {
    let mut roles = vec![Roles::default(); air.count];
    let labels = &air.inner_labels;
    for_each_overlap(&air.inner, &other.solid, |i, _, px| {
        roles[labels[i] as usize].px += u64::from(px);
    });
    for_each_overlap(&air.inner, &other.model, |i, _, px| {
        roles[labels[i] as usize].model += u64::from(px);
    });
    for_each_overlap(&air.inner, &other.support, |i, _, px| {
        roles[labels[i] as usize].support += u64::from(px);
    });
    roles
}

/// What one pixel of a layer is, for a drain going through it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Pixel {
    /// Model (possibly support too).
    Model,
    /// Support alone.
    Support,
    /// Air: drained when it is opened air of a drained region, closed when
    /// it is a hairline or a pocket closed toward the plate.
    Air {
        /// Whether resin flows freely there.
        drained: bool,
    },
}

/// The pixels `(x, row)` of layers `layers`, classified with the drained
/// flags of the air map's regions. Layers are rasterised in parallel.
pub(super) fn classify(
    input: &ReviewInput<'_>,
    width: u32,
    drained: &[Vec<bool>],
    layers: &[usize],
    [x, row]: [u32; 2],
) -> Vec<Pixel> {
    let _clock = crate::clock::scope("traps.classify");
    layers
        .par_iter()
        .map(|&layer| {
            let pixels = LayerPixels::compute(input, layer);
            if covers(&pixels.model, row, x) {
                return Pixel::Model;
            }
            if covers(&pixels.support, row, x) {
                return Pixel::Support;
            }
            let air = AirPixels::from_pixels(pixels, width, input);
            Pixel::Air {
                drained: air
                    .region_at(row, x)
                    .is_some_and(|region| drained[layer][region as usize]),
            }
        })
        .collect()
}

/// The opened air runs of each cavity in `cavities` (its member regions),
/// layer by layer from the plate up, with the model runs of their rim: the
/// pixels a fill cures (see [`crate::fill`]). Each layer any cavity touches
/// is rasterised once, in parallel.
pub(super) fn cavity_runs(
    input: &ReviewInput<'_>,
    width: u32,
    cavities: &[&[RegionRef]],
) -> Vec<Vec<FillLayerRuns>> {
    let _clock = crate::clock::scope("traps.cavity_runs");
    let mut wanted: Vec<(usize, usize, u32)> = cavities
        .iter()
        .enumerate()
        .flat_map(|(cavity, members)| {
            members
                .iter()
                .map(move |member| (member.layer, cavity, member.index))
        })
        .collect();
    wanted.sort_unstable();
    let layers: Vec<&[(usize, usize, u32)]> = wanted.chunk_by(|a, b| a.0 == b.0).collect();
    let (width_px, height_px) = (input.width_px(), input.height_px());
    // Each cavity's index with its runs on the layer.
    let found: Vec<Vec<(usize, FillLayerRuns)>> = layers
        .par_iter()
        .map(|members| {
            let layer = members[0].0;
            let air = AirPixels::from_pixels(LayerPixels::compute(input, layer), width, input);
            members
                .chunk_by(|a, b| a.1 == b.1)
                .map(|regions| {
                    let runs: Vec<Run> = air
                        .air
                        .iter()
                        .zip(&air.labels)
                        .filter(|(_, label)| regions.iter().any(|r| r.2 == **label))
                        .map(|(run, _)| *run)
                        .collect();
                    // The model pixels touching the cavity, sides or corners.
                    let rim = intersection(
                        &dilate_square(&runs, 1, width_px, height_px),
                        &air.pixels.model,
                    );
                    (regions[0].1, (layer, runs, rim))
                })
                .collect()
        })
        .collect();
    let mut out: Vec<Vec<FillLayerRuns>> = vec![Vec::new(); cavities.len()];
    for (cavity, layer) in found.into_iter().flatten() {
        out[cavity].push(layer);
    }
    out
}

/// Whether the run list `runs` holds pixel `(x, row)`.
fn covers(runs: &[Run], row: u32, x: u32) -> bool {
    coverage(runs, row, x, x + 1) > 0
}

/// The morphological opening of the run list `runs` by a `width` x `width`
/// square, on a screen `height_px` rows tall: the pixels of `runs` that a
/// square of that size lying entirely in `runs` covers. Stretches narrower
/// than the square in either direction disappear; wider ones keep their
/// shape. A `width` of 0 or 1 changes nothing.
pub(super) fn open(runs: &[Run], width: u32, height_px: u32) -> Vec<Run> {
    let _clock = crate::clock::scope("traps.open");
    if width <= 1 || runs.is_empty() {
        return runs.to_vec();
    }
    let reach = width - 1;
    // Erode: the top-left corners of the squares that fit. A run keeps the
    // corners of the squares that fit in it on its row; `width` rows in a
    // row must all keep a corner.
    let corners: Vec<Run> = runs
        .iter()
        .filter(|run| run.len() >= width)
        .map(|run| Run::new(run.row, run.x0, run.x1 - reach))
        .collect();
    let rows = RowIndex::new(&corners, height_px);
    let mut eroded: Vec<Run> = Vec::with_capacity(corners.len());
    let (mut kept, mut scratch): (Vec<Run>, Vec<Run>) = (Vec::new(), Vec::new());
    for row in 0..height_px.saturating_sub(reach) {
        let first = rows.row(&corners, row);
        if first.is_empty() {
            continue;
        }
        kept.clear();
        kept.extend_from_slice(first);
        for next in row + 1..=row + reach {
            intersect_row(&kept, rows.row(&corners, next), &mut scratch);
            std::mem::swap(&mut kept, &mut scratch);
            if kept.is_empty() {
                break;
            }
        }
        eroded.extend_from_slice(&kept);
    }
    // Dilate: every square back from its corner.
    for run in &mut eroded {
        run.x1 += reach;
    }
    let spans = eroded;
    let rows = RowIndex::new(&spans, height_px);
    let mut opened: Vec<Run> = Vec::with_capacity(runs.len());
    let (mut covered, mut scratch): (Vec<Run>, Vec<Run>) = (Vec::new(), Vec::new());
    for row in 0..height_px {
        covered.clear();
        for from in row.saturating_sub(reach)..=row {
            union_row(&covered, rows.row(&spans, from), &mut scratch);
            std::mem::swap(&mut covered, &mut scratch);
        }
        opened.extend(covered.iter().map(|span| Run::new(row, span.x0, span.x1)));
    }
    opened
}

/// The runs of `a` (on one row) clipped to the runs of `b` (on another),
/// kept on `a`'s row, into `out`.
fn intersect_row(a: &[Run], b: &[Run], out: &mut Vec<Run>) {
    out.clear();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        let (lo, hi) = (a[i].x0.max(b[j].x0), a[i].x1.min(b[j].x1));
        if lo < hi {
            out.push(Run::new(a[i].row, lo, hi));
        }
        if a[i].x1 <= b[j].x1 {
            i += 1;
        } else {
            j += 1;
        }
    }
}

/// The union of the runs of `a` and `b` (each sorted, on any rows), as
/// sorted runs that neither overlap nor touch, into `out`. Their rows are
/// left as found: the caller sets the row.
fn union_row(a: &[Run], b: &[Run], out: &mut Vec<Run>) {
    out.clear();
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        let next = if j >= b.len() || (i < a.len() && a[i].x0 <= b[j].x0) {
            i += 1;
            a[i - 1]
        } else {
            j += 1;
            b[j - 1]
        };
        match out.last_mut() {
            Some(last) if next.x0 <= last.x1 => last.x1 = last.x1.max(next.x1),
            _ => out.push(next),
        }
    }
}

/// Where each row's runs start in a run list.
struct RowIndex {
    /// `starts[row]..starts[row + 1]` are the runs of `row`.
    starts: Vec<usize>,
}

impl RowIndex {
    fn new(runs: &[Run], height_px: u32) -> Self {
        let mut starts = Vec::with_capacity(height_px as usize + 1);
        let mut at = 0;
        for row in 0..=height_px {
            while at < runs.len() && runs[at].row < row {
                at += 1;
            }
            starts.push(at);
        }
        Self { starts }
    }

    fn row<'r>(&self, runs: &'r [Run], row: u32) -> &'r [Run] {
        let row = row as usize;
        &runs[self.starts[row]..self.starts[row + 1]]
    }
}

#[cfg(test)]
mod air_tests {
    use super::*;

    /// The run list of a picture: `#` is in the list.
    fn picture(rows: &[&str]) -> Vec<Run> {
        let mut runs = Vec::new();
        for (row, line) in rows.iter().enumerate() {
            let mut start = None;
            for (x, c) in line.chars().chain(std::iter::once('.')).enumerate() {
                match (c == '#', start) {
                    (true, None) => start = Some(x as u32),
                    (false, Some(x0)) => {
                        runs.push(Run::new(row as u32, x0, x as u32));
                        start = None;
                    }
                    _ => {}
                }
            }
        }
        runs
    }

    #[test]
    fn opening_drops_what_is_narrower_than_the_square() {
        let runs = picture(&[
            "####.......",
            "####.......",
            "#########..",
            "####.......",
            "##.....####",
            "##.....####",
            "##.....####",
        ]);
        // The 1-pixel spur and the 2-pixel column go; the blocks stay whole.
        assert_eq!(
            open(&runs, 3, 7),
            picture(&[
                "####.......",
                "####.......",
                "####.......",
                "####.......",
                ".......####",
                ".......####",
                ".......####",
            ])
        );
        assert_eq!(open(&runs, 1, 7), runs);
        assert_eq!(open(&runs, 5, 7), Vec::new());
    }

    #[test]
    fn opening_keeps_every_pixel_a_square_covers() {
        // A staircase of overlapping 3 x 3 squares stays whole; the pixel
        // that only touches it by a corner, and the 2 x 2 step, do not.
        let runs = picture(&[
            "###.......",
            "###.......",
            "######....",
            "...###....",
            "...######.",
            "......###.",
            "......###.",
            ".........#",
            "##.......#",
            "##........",
        ]);
        assert_eq!(
            open(&runs, 3, 10),
            picture(&[
                "###.......",
                "###.......",
                "######....",
                "...###....",
                "...######.",
                "......###.",
                "......###.",
                "..........",
                "..........",
                "..........",
            ])
        );
    }
}
