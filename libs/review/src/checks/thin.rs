/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Features thinner than a pixel (#92): walls, pins, blades and tips
//! narrower than the printer can resolve.
//!
//! Reports [`CheckKind::ThinFeature`] findings, without a suggested fix:
//! only the model itself can be changed.
//!
//! # What is measured
//!
//! Each layer's contours are rasterised again, one role at a time, on a grid
//! [`subpixels`](ThinFeatureOptions::subpixels) times finer than the screen
//! along each axis (a quarter pixel by default), and kept as row runs. On
//! that grid the role is *opened* with a disc as wide as its limit: the
//! union of every such disc that fits inside it. What the opening removes is
//! thinner than the limit. The disc is round in pixel units, so a wall is
//! caught across X, across Y and on any diagonal alike; on a screen with
//! non-square pixels it is an ellipse in millimetres, the same number of
//! pixels across in every direction.
//!
//! Why runs of a finer grid, rather than the printer's own pixels or exact
//! polygon offsets: the printer's raster rounds every width to whole pixels
//! and drops a feature under a pixel altogether, and those are the worst
//! ones; exact offsetting needs a robust polygon offset and boolean library
//! the review does not have. A quarter-pixel grid measures the vector
//! contours to a quarter pixel with exact, simple morphology on runs, through
//! the same rasteriser the printer file uses, and never holds a bitmap.
//!
//! The **width** of a thin pixel is the diameter of the largest disc that
//! fits in the solid and covers it (its local thickness, in quarter pixels).
//! A feature's width is the median over its thin pixels: the thickness of a
//! wall or a pin, unaffected by its corners or by the root where it meets
//! thicker solid.
//!
//! # What is reported
//!
//! - **Model features under 2 pixels**
//!   ([`model_width_px`](ThinFeatureOptions::model_width_px)): a warning, or
//!   an error when the feature is under 1 pixel
//!   ([`error_width_px`](ThinFeatureOptions::error_width_px)).
//! - **Support parts under 1 pixel**
//!   ([`support_width_px`](ThinFeatureOptions::support_width_px)): support
//!   tips are thin on purpose, so only a support that would not print at
//!   all is reported, and only where no model covers it.
//! - **Not the steps of a surface.** A thin sliver lying on thick solid in
//!   the layer below or above it is the crest of a slope or a ridge, or the
//!   rounding of a leaning edge, as the layers cut it: its pixels are
//!   *backed* and do not count. Layer 0 rests on the plate, which backs all
//!   of it. Only unbacked thin pixels make a feature: those of a wall, pin or
//!   blade that stays thin from one layer to the next, or that has nothing
//!   above and below.
//! - **Not the rounding of corners.** A disc cannot reach into a corner, so
//!   every corner leaves a few thin sub-pixels attached to thicker solid. A
//!   thin piece attached to thicker solid is kept only when it sticks out at
//!   least [`min_length_px`](ThinFeatureOptions::min_length_px) (its area
//!   divided by its width): a fin, a neck or the edge of a blade, but not a
//!   corner blunter than about 25 degrees (measured: up to 1 pixel of
//!   residue for 60 degrees, 3 for 30, 10 for 10). A piece that is a whole
//!   part of its layer on its own, such as the section of a pin, is always
//!   kept, however small.
//! - **One finding per feature.** Thin pieces of consecutive layers that come
//!   within a layer height (and at least a pixel) of each other are joined
//!   with union-find: a wall 200 layers tall, or leaning up to 45 degrees,
//!   is one finding.
//! - **Large enough to matter.** A feature is reported when its unbacked
//!   thin pixels, summed over its layers, reach
//!   [`min_area_px`](ThinFeatureOptions::min_area_px): the sharp tip of a
//!   cone, thin over a layer or two, is not worth a finding.
//!
//! Known limit: a neck a layer or two tall between thick parts is backed on
//! both sides and goes unreported.

use std::fmt::Write as _;

use rayon::prelude::*;
use sc_print_primitives::PixelResolution;
use sc_sl1cer::contour::{ContourLayer, Polygon};
use sc_sl1cer::raster::pixel_map::PixelMap;
use sc_sl1cer::raster::rasterize_layer_binary;

use crate::finding::{CheckKind, Finding, RoleScope, Severity};
use crate::input::{ReviewInput, Role};
use crate::regions::{Connectivity, label_runs, measure_runs};
use crate::runner::ReviewContext;
use crate::runs::{self, Run};
use crate::union_find::{Groups, RegionRef, UnionFind};

/// Settings of the thin-feature check. Widths are in printer pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct ThinFeatureOptions {
    /// Model features narrower than this are reported.
    pub model_width_px: f64,
    /// Support parts narrower than this are reported; 0 skips supports.
    /// Support tips are thin on purpose, so it is lower than the model's.
    pub support_width_px: f64,
    /// Features narrower than this are errors; wider ones, still under their
    /// role's limit, are warnings.
    pub error_width_px: f64,
    /// Thin pieces of one layer that touch thicker solid and stick out less
    /// than this (their area divided by their width) are the corners of a
    /// wider shape, and are dropped.
    pub min_length_px: f64,
    /// The smallest feature reported: its unbacked thin pixels, summed over
    /// its layers.
    pub min_area_px: f64,
    /// How many times finer than the screen, along each axis, the contours
    /// are measured.
    pub subpixels: u32,
}

impl Default for ThinFeatureOptions {
    fn default() -> Self {
        Self {
            model_width_px: 2.0,
            support_width_px: 1.0,
            error_width_px: 1.0,
            min_length_px: 3.0,
            min_area_px: 4.0,
            subpixels: 4,
        }
    }
}

/// Finds the features thinner than the printer resolves.
pub fn run(ctx: &ReviewContext<'_>, options: &ThinFeatureOptions) -> Vec<Finding> {
    let input = ctx.input;
    let subpixels = options.subpixels.max(1);
    let Some(grid) = FineGrid::new(input, subpixels) else {
        return Vec::new();
    };
    let to_sub = |px: f64| (px * f64::from(subpixels)).round().max(0.0) as u32;
    let min_length = options.min_length_px.max(0.0) * f64::from(subpixels);
    let passes: Vec<Pass> = [
        (Role::Model, options.model_width_px),
        (Role::Support, options.support_width_px),
    ]
    .into_iter()
    .map(|(role, width_px)| Pass {
        role,
        width: to_sub(width_px),
        min_length,
    })
    // A disc one sub-pixel wide fits everywhere: nothing is thinner.
    .filter(|pass| pass.width >= 2 && input.leaves().iter().any(|l| l.role == pass.role))
    .collect();
    if passes.is_empty() {
        return Vec::new();
    }
    let layers = sweep(input, &grid, &passes);
    let rules = Rules {
        error_width: to_sub(options.error_width_px),
        min_area_px: options.min_area_px,
    };
    passes
        .iter()
        .zip(&layers)
        .flat_map(|(pass, pass_layers)| report(input, &grid, pass, pass_layers, &rules))
        .collect()
}

/// The fine grid the contours are measured on.
struct FineGrid {
    map: PixelMap,
    /// Sub-pixels per printer pixel along each axis.
    scale: u32,
    width: u32,
    height: u32,
}

impl FineGrid {
    fn new(input: &ReviewInput<'_>, scale: u32) -> Option<Self> {
        let width = input.width_px().checked_mul(scale)?;
        let height = input.height_px().checked_mul(scale)?;
        let screen = input.pixel_map();
        let map = PixelMap::new(
            screen.plate_x(),
            screen.plate_y(),
            PixelResolution::new(width, height).ok()?,
            screen.mirror(),
        );
        Some(Self {
            map,
            scale,
            width,
            height,
        })
    }

    /// Sub-pixels per printer pixel.
    fn sub_area(&self) -> f64 {
        f64::from(self.scale) * f64::from(self.scale)
    }

    /// The raster point (printer pixels) at the centre of sub-pixel `p`.
    fn to_raster(&self, p: [u32; 2]) -> [f64; 2] {
        let s = f64::from(self.scale);
        [(f64::from(p[0]) + 0.5) / s, (f64::from(p[1]) + 0.5) / s]
    }
}

/// One role's check.
struct Pass {
    role: Role,
    /// The width limit in sub-pixels: the diameter of the opening disc.
    width: u32,
    /// How far, in sub-pixels, a piece attached to thicker solid must stick
    /// out to be kept.
    min_length: f64,
}

/// What turns features into findings.
struct Rules {
    /// Features narrower than this, in sub-pixels, are errors.
    error_width: u32,
    min_area_px: f64,
}

/// One thin piece of one layer: an 8-connected part of what the opening
/// removed.
#[derive(Clone, Debug)]
struct Piece {
    /// Sub-pixels in the piece.
    area: u64,
    /// Sub-pixels with nothing thick straight below or above.
    unbacked: u64,
    /// A sub-pixel of the piece near its centre.
    inside: [u32; 2],
    /// Sub-pixels by width: `widths[d]` of them are covered by a disc `d`
    /// sub-pixels across that fits in the solid, and by none wider.
    widths: Vec<u64>,
    /// Second moments of the piece, for its direction.
    moments: Moments,
    /// Whether it touches the role's thick part on its layer.
    attached: bool,
}

/// What a pass found on one layer, once its neighbours are known.
#[derive(Clone, Debug, Default)]
struct LayerPieces {
    pieces: Vec<Piece>,
    /// Pairs `(lower, upper)` of a piece of the layer below and one of this
    /// layer that belong to the same feature.
    links: Vec<(u32, u32)>,
}

/// One role on one layer, before its neighbours are known.
#[derive(Clone, Debug, Default)]
struct RoleLayer {
    /// What discs of the role's width cover.
    thick: Vec<Run>,
    /// The kept thin pieces, as runs, with the piece of every run.
    thin: Vec<Run>,
    labels: Vec<u32>,
    pieces: Vec<Piece>,
}

/// Rasterises, opens and joins every layer, a batch of layers at a time so
/// only a batch of sub-pixel runs is held at once, and keeps the thin pieces
/// of each pass.
fn sweep(input: &ReviewInput<'_>, grid: &FineGrid, passes: &[Pass]) -> Vec<Vec<LayerPieces>> {
    let count = input.layer_count();
    let batch = (rayon::current_num_threads() * 2).max(4);
    let mut done: Vec<Vec<LayerPieces>> = vec![Vec::with_capacity(count); passes.len()];
    // The last two layers of the previous batch: the next one needs them as
    // neighbours.
    let mut carried: Vec<Vec<RoleLayer>> = Vec::new();
    let mut start = 0;
    while start < count {
        let end = (start + batch).min(count);
        let fresh: Vec<Vec<RoleLayer>> = (start..end)
            .into_par_iter()
            .map(|layer| measure_layer(input, grid, passes, layer))
            .collect();
        let base = start - carried.len();
        let mut window = std::mem::take(&mut carried);
        window.extend(fresh);
        // A layer is finished once the layer above it is known.
        let first = start.saturating_sub(1);
        let last = if end == count { end } else { end - 1 };
        let finished: Vec<Vec<LayerPieces>> = (first..last)
            .into_par_iter()
            .map(|layer| {
                let at = |k: usize| &window[k - base];
                let below = (layer > 0).then(|| at(layer - 1));
                let above = (layer + 1 < end).then(|| at(layer + 1));
                let reach = link_reach(input, grid, layer);
                (0..passes.len())
                    .map(|p| {
                        finish_layer(
                            &at(layer)[p],
                            below.map(|b| &b[p]),
                            above.map(|a| &a[p]),
                            reach,
                        )
                    })
                    .collect()
            })
            .collect();
        for layer in finished {
            for (p, pieces) in layer.into_iter().enumerate() {
                done[p].push(pieces);
            }
        }
        let keep = window.len().min(2);
        carried = window.split_off(window.len() - keep);
        start = end;
    }
    done
}

/// How far apart, in sub-pixels, pieces of layer `layer` and the layer below
/// may be and still be one feature: the layer's height (a feature leaning up
/// to 45 degrees), and at least a pixel.
fn link_reach(input: &ReviewInput<'_>, grid: &FineGrid, layer: usize) -> i64 {
    let [x, y] = input.pixel_size_mm();
    let pixels = (input.layers()[layer].height_mm() / x.min(y)).max(1.0);
    (pixels * f64::from(grid.scale)).ceil() as i64
}

/// Rasterises one layer on the fine grid and finds the thin pieces of every
/// pass.
fn measure_layer(
    input: &ReviewInput<'_>,
    grid: &FineGrid,
    passes: &[Pass],
    layer: usize,
) -> Vec<RoleLayer> {
    let model = fine_role_runs(input, grid, layer, Role::Model);
    passes
        .iter()
        .map(|pass| match pass.role {
            Role::Model => thin_pieces(grid, pass, &model, None),
            Role::Support => {
                let support = fine_role_runs(input, grid, layer, Role::Support);
                thin_pieces(grid, pass, &support, Some(&model))
            }
        })
        .collect()
}

/// The sub-pixels of every `role` leaf on `layer`, the leaves rasterised
/// together as the printer file merges them.
fn fine_role_runs(input: &ReviewInput<'_>, grid: &FineGrid, layer: usize, role: Role) -> Vec<Run> {
    let contours: Vec<&ContourLayer> = input
        .leaves()
        .iter()
        .filter(|leaf| leaf.role == role)
        .map(|leaf| &leaf.stack.layers()[layer])
        .filter(|contour| !contour.polygons().is_empty())
        .collect();
    match contours.as_slice() {
        [] => Vec::new(),
        [only] => runs::runs_from_raster(&rasterize_layer_binary(only, &grid.map)),
        many => {
            let polygons: Vec<Polygon> = many
                .iter()
                .flat_map(|contour| contour.polygons().iter().cloned())
                .collect();
            let merged = ContourLayer::new(many[0].z_mm(), polygons);
            runs::runs_from_raster(&rasterize_layer_binary(&merged, &grid.map))
        }
    }
}

/// Opens `solid` with the pass's disc and measures the pieces the opening
/// removes, without the pixels of `mask` (which another role prints anyway).
fn thin_pieces(grid: &FineGrid, pass: &Pass, solid: &[Run], mask: Option<&[Run]>) -> RoleLayer {
    let disc = Kernel::disc(pass.width);
    let thick = open(solid, &disc, grid);
    let mut thin = difference(solid, &thick);
    if let Some(mask) = mask {
        thin = difference(&thin, mask);
    }
    let (labels, count) = label_runs(&thin, Connectivity::Eight);
    let regions = measure_runs(&thin, &labels, count, grid.width, grid.height);
    let thick_rows = Rows::new(&thick);
    let mut attached = vec![false; count];
    for (run, &label) in thin.iter().zip(&labels) {
        let slot = &mut attached[label as usize];
        *slot = *slot || touches(&thick_rows, run, 1);
    }
    let pieces: Vec<Piece> = regions
        .iter()
        .zip(&attached)
        .map(|(region, &attached)| Piece {
            area: region.area_px,
            unbacked: 0,
            inside: region.inside_px,
            widths: vec![0; pass.width as usize],
            moments: Moments::default(),
            attached,
        })
        .collect();
    // An attached piece sticks out at most its area over one sub-pixel: the
    // smaller ones are corners whatever their width.
    let keep: Vec<bool> = pieces
        .iter()
        .map(|p| !p.attached || p.area as f64 >= pass.min_length)
        .collect();
    let (thin, labels, mut pieces) = retain(&thin, &labels, pieces, &keep);
    measure_widths(grid, pass, solid, &thin, &labels, &mut pieces);
    let keep: Vec<bool> = pieces
        .iter()
        .map(|p| !p.attached || p.area as f64 >= pass.min_length * f64::from(median(&p.widths)))
        .collect();
    let (thin, labels, mut pieces) = retain(&thin, &labels, pieces, &keep);
    for (run, &label) in thin.iter().zip(&labels) {
        pieces[label as usize].moments.add(run);
    }
    RoleLayer {
        thick,
        thin,
        labels,
        pieces,
    }
}

/// The pieces marked in `keep`, renumbered in order, with their runs.
fn retain(
    runs: &[Run],
    labels: &[u32],
    pieces: Vec<Piece>,
    keep: &[bool],
) -> (Vec<Run>, Vec<u32>, Vec<Piece>) {
    let mut renumber = vec![u32::MAX; pieces.len()];
    let mut kept_pieces = Vec::new();
    for (old, piece) in pieces.into_iter().enumerate() {
        if keep[old] {
            renumber[old] = kept_pieces.len() as u32;
            kept_pieces.push(piece);
        }
    }
    let mut kept_runs = Vec::new();
    let mut kept_labels = Vec::new();
    for (run, &label) in runs.iter().zip(labels) {
        let new = renumber[label as usize];
        if new != u32::MAX {
            kept_runs.push(*run);
            kept_labels.push(new);
        }
    }
    (kept_runs, kept_labels, kept_pieces)
}

/// Fills every piece's `widths`: for each thin sub-pixel, the widest disc
/// under the pass's width that fits in `solid` and covers it.
fn measure_widths(
    grid: &FineGrid,
    pass: &Pass,
    solid: &[Run],
    thin: &[Run],
    labels: &[u32],
    pieces: &mut [Piece],
) {
    if thin.is_empty() {
        return;
    }
    // A disc that covers a thin sub-pixel lies within the pass's width of
    // it, so the solid around the thin pieces is all the openings need.
    let reach = Kernel::square(i64::from(pass.width));
    let near = intersection(solid, &dilate(thin, &reach, grid));
    let thin_rows = Rows::new(thin);
    let mut assigned: Vec<Run> = Vec::new();
    for width in (2..pass.width).rev() {
        let covered = intersection(&open(&near, &Kernel::disc(width), grid), thin);
        for run in difference(&covered, &assigned) {
            let label = thin_rows.label_at(labels, &run);
            pieces[label as usize].widths[width as usize] += u64::from(run.len());
        }
        assigned = runs::union(&assigned, &covered);
    }
    // A single sub-pixel is a disc that fits everywhere.
    for piece in pieces.iter_mut() {
        let wider: u64 = piece.widths.iter().sum();
        piece.widths[1] = piece.area - wider;
    }
}

/// Counts the piece's unbacked sub-pixels and links them to the pieces of
/// the layer below.
fn finish_layer(
    this: &RoleLayer,
    below: Option<&RoleLayer>,
    above: Option<&RoleLayer>,
    reach: i64,
) -> LayerPieces {
    let mut pieces = this.pieces.clone();
    // Layer 0 rests on the plate, which backs it all.
    if let Some(below) = below {
        let below_rows = Rows::new(&below.thick);
        let above_rows = above.map(|a| Rows::new(&a.thick));
        let mut backed = Vec::new();
        for (run, &label) in this.thin.iter().zip(&this.labels) {
            backed.clear();
            below_rows.overlaps(run, &mut backed);
            if let Some(rows) = &above_rows {
                rows.overlaps(run, &mut backed);
            }
            pieces[label as usize].unbacked += u64::from(run.len()) - covered_len(&mut backed);
        }
    }
    let links = below.map_or_else(Vec::new, |below| link_pieces(below, this, reach));
    LayerPieces { pieces, links }
}

/// Pairs of pieces, one in `lower` and one in `upper`, that come within
/// `reach` sub-pixels of each other (Chebyshev distance).
fn link_pieces(lower: &RoleLayer, upper: &RoleLayer, reach: i64) -> Vec<(u32, u32)> {
    let mut links = Vec::new();
    if lower.thin.is_empty() || upper.thin.is_empty() {
        return links;
    }
    let rows = Rows::new(&lower.thin);
    for (run, &up) in upper.thin.iter().zip(&upper.labels) {
        let y = i64::from(run.row);
        let (x0, x1) = (i64::from(run.x0) - reach, i64::from(run.x1) + reach);
        for row in (y - reach)..=(y + reach) {
            let (offset, found) = rows.get_with_offset(row);
            let from = found.partition_point(|r| i64::from(r.x1) <= x0);
            for (i, other) in found[from..].iter().enumerate() {
                if i64::from(other.x0) >= x1 {
                    break;
                }
                links.push((lower.labels[offset + from + i], up));
            }
        }
    }
    links.sort_unstable();
    links.dedup();
    links
}

/// Joins the pieces of a pass across layers and turns every feature large
/// enough into a finding.
fn report(
    input: &ReviewInput<'_>,
    grid: &FineGrid,
    pass: &Pass,
    layers: &[LayerPieces],
    rules: &Rules,
) -> Vec<Finding> {
    let mut offsets = Vec::with_capacity(layers.len() + 1);
    let mut total = 0;
    offsets.push(0);
    for layer in layers {
        total += layer.pieces.len();
        offsets.push(total);
    }
    let mut sets = UnionFind::new(total);
    for (layer, pieces) in layers.iter().enumerate().skip(1) {
        for &(lower, upper) in &pieces.links {
            sets.union(
                offsets[layer - 1] + lower as usize,
                offsets[layer] + upper as usize,
            );
        }
    }
    let groups = Groups::from_union_find(offsets, &mut sets);
    let mut findings = Vec::new();
    for members in groups.iter() {
        let piece = |m: &RegionRef| &layers[m.layer].pieces[m.index as usize];
        let unbacked: u64 = members.iter().map(|m| piece(m).unbacked).sum();
        if (unbacked as f64) < rules.min_area_px * grid.sub_area() || unbacked == 0 {
            continue;
        }
        let mut widths = vec![0u64; pass.width as usize];
        for m in members.iter().filter(|m| piece(m).unbacked > 0) {
            for (sum, count) in widths.iter_mut().zip(&piece(m).widths) {
                *sum += count;
            }
        }
        let focus = members
            .iter()
            .filter(|m| piece(m).unbacked > 0)
            .max_by(|a, b| {
                piece(a)
                    .unbacked
                    .cmp(&piece(b).unbacked)
                    .then(b.layer.cmp(&a.layer))
            })
            .expect("a feature with unbacked pixels");
        let feature = Feature {
            first: members[0].layer,
            last: members
                .iter()
                .map(|m| m.layer)
                .max()
                .unwrap_or(members[0].layer),
            width: median(&widths),
            focus_layer: focus.layer,
            focus: piece(focus),
        };
        findings.push(feature.finding(input, grid, pass, rules));
    }
    findings
}

/// One feature, ready to report.
struct Feature<'a> {
    first: usize,
    last: usize,
    /// Its width in sub-pixels.
    width: u32,
    focus_layer: usize,
    /// Its piece with the most unbacked sub-pixels, where the camera goes.
    focus: &'a Piece,
}

/// How many times longer than wide a free-standing piece is at least to be
/// called a wall rather than a pin.
const WALL_LENGTH: f64 = 3.0;

impl Feature<'_> {
    fn finding(
        &self,
        input: &ReviewInput<'_>,
        grid: &FineGrid,
        pass: &Pass,
        rules: &Rules,
    ) -> Finding {
        let scale = f64::from(grid.scale);
        let width_px = f64::from(self.width) / scale;
        let (severity, limit) = if self.width < rules.error_width {
            (Severity::Error, rules.error_width)
        } else {
            (Severity::Warning, pass.width)
        };
        let width_mm = width_px * self.focus.moments.mm_per_px_across(input.pixel_size_mm());
        let focus_width = f64::from(median(&self.focus.widths));
        let long = self.focus.area as f64 >= WALL_LENGTH * focus_width * focus_width;
        let (noun, scope) = match pass.role {
            Role::Support => ("support", RoleScope::Support),
            // Thinning out of thicker solid: the edge of a blade, or a tip.
            Role::Model if self.focus.attached && !is_even(&self.focus.widths) => {
                ("edge", RoleScope::Model)
            }
            Role::Model if long => ("wall", RoleScope::Model),
            Role::Model => ("pin", RoleScope::Model),
        };
        let limit = pixels(f64::from(limit) / scale);
        let title = format!("{} thinner than {limit}", capitalised(noun));
        let detail = format!(
            "This {noun} is {width_mm:.3} mm ({} px) wide, under {limit}: it will not print.",
            number(width_px)
        );
        Finding::new(
            CheckKind::ThinFeature,
            severity,
            self.first,
            self.last,
            input.focus(self.focus_layer, grid.to_raster(self.focus.inside)),
            scope,
            title,
            detail,
        )
    }
}

/// The width at least half the sub-pixels counted in `widths` reach: the
/// smallest `d` with half of them at `d` or narrower.
fn median(widths: &[u64]) -> u32 {
    let total: u64 = widths.iter().sum();
    let mut seen = 0;
    for (width, &count) in widths.iter().enumerate() {
        seen += count;
        if 2 * seen >= total && seen > 0 {
            return width as u32;
        }
    }
    1
}

/// Whether three quarters of the sub-pixels counted in `widths` are within
/// a sub-pixel of the median: a wall, a fin or a neck of even thickness,
/// rather than an edge that tapers out of thicker solid.
fn is_even(widths: &[u64]) -> bool {
    let total: u64 = widths.iter().sum();
    let middle = median(widths) as usize;
    let near: u64 = widths[middle.saturating_sub(1)..(middle + 2).min(widths.len())]
        .iter()
        .sum();
    4 * near >= 3 * total
}

fn capitalised(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// `"2 pixels"`, `"1 pixel"`, `"1.5 pixels"`.
fn pixels(count: f64) -> String {
    let unit = if (count - 1.0).abs() < 1e-9 {
        "pixel"
    } else {
        "pixels"
    };
    format!("{} {unit}", number(count))
}

/// A number with at most two decimals and no trailing zeros.
fn number(value: f64) -> String {
    let mut text = String::new();
    let _ = write!(text, "{value:.2}");
    while text.contains('.') && (text.ends_with('0') || text.ends_with('.')) {
        text.pop();
    }
    text
}

/// Sums over the sub-pixel centres of a piece, for its direction.
#[derive(Clone, Copy, Debug, Default)]
struct Moments {
    n: f64,
    x: f64,
    y: f64,
    xx: f64,
    yy: f64,
    xy: f64,
}

impl Moments {
    fn add(&mut self, run: &Run) {
        let len = f64::from(run.len());
        let mid = 0.5 * (f64::from(run.x0) + f64::from(run.x1));
        let cy = f64::from(run.row) + 0.5;
        self.n += len;
        self.x += len * mid;
        self.y += len * cy;
        // The centres of `len` consecutive sub-pixels have mean `mid` and
        // variance (len^2 - 1) / 12.
        self.xx += len * mid.mul_add(mid, (len * len - 1.0) / 12.0);
        self.yy += len * cy * cy;
        self.xy += len * mid * cy;
    }

    /// Millimetres per pixel across the piece, along its narrow axis: on a
    /// screen with non-square pixels a pixel's size depends on the
    /// direction.
    fn mm_per_px_across(&self, pixel_mm: [f64; 2]) -> f64 {
        if self.n == 0.0 {
            return pixel_mm[0].min(pixel_mm[1]);
        }
        let (mx, my) = (self.x / self.n, self.y / self.n);
        let a = self.xx / self.n - mx * mx;
        let c = self.yy / self.n - my * my;
        let b = self.xy / self.n - mx * my;
        // The long axis is at `angle`; the narrow one is perpendicular.
        let angle = 0.5 * (2.0 * b).atan2(a - c);
        let (ux, uy) = (-angle.sin(), angle.cos());
        (ux * pixel_mm[0]).hypot(uy * pixel_mm[1])
    }
}

/// A structuring element: for each row offset `dy` from its anchor, the
/// columns `l..=r` it covers.
#[derive(Clone, Debug)]
struct Kernel {
    rows: Vec<(i64, i64, i64)>,
}

impl Kernel {
    /// A disc `d` sub-pixels across: the cells of a `d` x `d` box whose
    /// centres lie within `d / 2` of the box's centre, anchored on its middle
    /// row, which is full.
    fn disc(d: u32) -> Self {
        let centre = 0.5 * f64::from(d);
        let anchor = i64::from(d / 2);
        let rows = (0..i64::from(d))
            .filter_map(|j| {
                let dy = j as f64 + 0.5 - centre;
                let l = (0..i64::from(d)).find(|&i| {
                    let dx = i as f64 + 0.5 - centre;
                    dx.mul_add(dx, dy * dy) <= centre * centre
                })?;
                // Each row is symmetric about the centre.
                let r = i64::from(d) - 1 - l;
                Some((j - anchor, l - anchor, r - anchor))
            })
            .collect();
        Self { rows }
    }

    /// The square `-radius..=radius` along both axes.
    fn square(radius: i64) -> Self {
        Self {
            rows: (-radius..=radius).map(|dy| (dy, -radius, radius)).collect(),
        }
    }

    fn dy_range(&self) -> (i64, i64) {
        let min = self.rows.iter().map(|r| r.0).min().unwrap_or(0);
        let max = self.rows.iter().map(|r| r.0).max().unwrap_or(0);
        (min, max)
    }
}

/// A run list indexed by row.
struct Rows<'a> {
    runs: &'a [Run],
    first: i64,
    /// Start of each row's runs, from row `first`, plus the end.
    starts: Vec<usize>,
}

impl<'a> Rows<'a> {
    fn new(runs: &'a [Run]) -> Self {
        let (Some(first), Some(last)) = (runs.first(), runs.last()) else {
            return Self {
                runs,
                first: 0,
                starts: vec![0],
            };
        };
        let first = first.row;
        let mut starts = vec![0usize; (last.row - first) as usize + 2];
        for run in runs {
            starts[(run.row - first) as usize + 1] += 1;
        }
        for i in 1..starts.len() {
            starts[i] += starts[i - 1];
        }
        Self {
            runs,
            first: i64::from(first),
            starts,
        }
    }

    /// The first and last row holding runs.
    fn span(&self) -> Option<(i64, i64)> {
        let rows = self.starts.len() as i64 - 1;
        (!self.runs.is_empty()).then_some((self.first, self.first + rows - 1))
    }

    fn get(&self, row: i64) -> &'a [Run] {
        self.get_with_offset(row).1
    }

    /// The runs of `row`, and the index of the first of them in the list.
    fn get_with_offset(&self, row: i64) -> (usize, &'a [Run]) {
        let i = row - self.first;
        if i < 0 || i as usize + 1 >= self.starts.len() {
            return (0, &[]);
        }
        let (from, to) = (self.starts[i as usize], self.starts[i as usize + 1]);
        (from, &self.runs[from..to])
    }

    /// The label of the run that holds `run`, which lies inside one.
    fn label_at(&self, labels: &[u32], run: &Run) -> u32 {
        let (offset, found) = self.get_with_offset(i64::from(run.row));
        let i = found.partition_point(|r| r.x1 <= run.x0);
        labels[offset + i]
    }

    /// Pushes the stretches of `run`'s row covered by these runs, clipped to
    /// `run`.
    fn overlaps(&self, run: &Run, out: &mut Vec<(u32, u32)>) {
        let found = self.get(i64::from(run.row));
        let from = found.partition_point(|r| r.x1 <= run.x0);
        for other in &found[from..] {
            if other.x0 >= run.x1 {
                break;
            }
            out.push((other.x0.max(run.x0), other.x1.min(run.x1)));
        }
    }
}

/// Whether any of `rows` lies within `reach` sub-pixels of `run`, across
/// rows too (8-adjacency for a reach of 1).
fn touches(rows: &Rows<'_>, run: &Run, reach: i64) -> bool {
    let y = i64::from(run.row);
    let (x0, x1) = (i64::from(run.x0) - reach, i64::from(run.x1) + reach);
    ((y - reach)..=(y + reach)).any(|row| {
        let found = rows.get(row);
        let from = found.partition_point(|r| i64::from(r.x1) <= x0);
        found.get(from).is_some_and(|r| i64::from(r.x0) < x1)
    })
}

/// The total length of stretches that may overlap.
fn covered_len(stretches: &mut [(u32, u32)]) -> u64 {
    stretches.sort_unstable();
    let mut total = 0u64;
    let mut end = 0u32;
    for &(x0, x1) in stretches.iter() {
        let from = x0.max(end);
        if x1 > from {
            total += u64::from(x1 - from);
        }
        end = end.max(x1);
    }
    total
}

/// Opening: the union of every placement of `kernel` that fits in `runs`.
fn open(runs: &[Run], kernel: &Kernel, grid: &FineGrid) -> Vec<Run> {
    dilate(&erode(runs, kernel), kernel, grid)
}

/// Erosion: the anchors where `kernel` fits inside `runs`.
fn erode(runs: &[Run], kernel: &Kernel) -> Vec<Run> {
    let rows = Rows::new(runs);
    let Some((lo, hi)) = rows.span() else {
        return Vec::new();
    };
    let (dy_min, dy_max) = kernel.dy_range();
    let mut out = Vec::new();
    let mut acc: Vec<(i64, i64)> = Vec::new();
    let mut shrunk: Vec<(i64, i64)> = Vec::new();
    let mut next: Vec<(i64, i64)> = Vec::new();
    for y in (lo - dy_min)..=(hi - dy_max) {
        for (i, &(dy, l, r)) in kernel.rows.iter().enumerate() {
            // Anchor x fits this row of the kernel iff x + l..=x + r lies in
            // one run.
            shrunk.clear();
            shrunk.extend(rows.get(y + dy).iter().filter_map(|run| {
                let (x0, x1) = (i64::from(run.x0) - l, i64::from(run.x1) - r);
                (x0 < x1).then_some((x0, x1))
            }));
            if i == 0 {
                std::mem::swap(&mut acc, &mut shrunk);
            } else {
                intersect_into(&acc, &shrunk, &mut next);
                std::mem::swap(&mut acc, &mut next);
            }
            if acc.is_empty() {
                break;
            }
        }
        // The anchor row spans the kernel's full width (`l <= 0 <= r`), so
        // every anchor lies inside a run: never negative.
        out.extend(
            acc.iter()
                .map(|&(x0, x1)| Run::new(y as u32, x0 as u32, x1 as u32)),
        );
        acc.clear();
    }
    out
}

/// Dilation: every sub-pixel `kernel` covers when anchored on `runs`,
/// clipped to the grid.
fn dilate(runs: &[Run], kernel: &Kernel, grid: &FineGrid) -> Vec<Run> {
    let rows = Rows::new(runs);
    let Some((lo, hi)) = rows.span() else {
        return Vec::new();
    };
    let (dy_min, dy_max) = kernel.dy_range();
    let (width, height) = (i64::from(grid.width), i64::from(grid.height));
    let mut out = Vec::new();
    let mut spans: Vec<(i64, i64)> = Vec::new();
    for y in (lo + dy_min).max(0)..=(hi + dy_max).min(height - 1) {
        spans.clear();
        for &(dy, l, r) in &kernel.rows {
            spans.extend(rows.get(y - dy).iter().filter_map(|run| {
                let x0 = (i64::from(run.x0) + l).max(0);
                let x1 = (i64::from(run.x1) + r).min(width);
                (x0 < x1).then_some((x0, x1))
            }));
        }
        spans.sort_unstable();
        let mut spans = spans.iter();
        let Some(&(mut x0, mut x1)) = spans.next() else {
            continue;
        };
        for &(next0, next1) in spans {
            if next0 <= x1 {
                x1 = x1.max(next1);
            } else {
                out.push(Run::new(y as u32, x0 as u32, x1 as u32));
                (x0, x1) = (next0, next1);
            }
        }
        out.push(Run::new(y as u32, x0 as u32, x1 as u32));
    }
    out
}

/// The intersection of two sorted lists of disjoint stretches.
fn intersect_into(a: &[(i64, i64)], b: &[(i64, i64)], out: &mut Vec<(i64, i64)>) {
    out.clear();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        let lo = a[i].0.max(b[j].0);
        let hi = a[i].1.min(b[j].1);
        if lo < hi {
            out.push((lo, hi));
        }
        if a[i].1 <= b[j].1 {
            i += 1;
        } else {
            j += 1;
        }
    }
}

/// The pixels of run list `a` that are not in run list `b`.
fn difference(a: &[Run], b: &[Run]) -> Vec<Run> {
    let mut out = Vec::with_capacity(a.len());
    let mut j = 0;
    for run in a {
        while j < b.len() && (b[j].row, b[j].x1) <= (run.row, run.x0) {
            j += 1;
        }
        let mut x = run.x0;
        for cut in b[j..]
            .iter()
            .take_while(|c| c.row == run.row && c.x0 < run.x1)
        {
            if cut.x0 > x {
                out.push(Run::new(run.row, x, cut.x0));
            }
            x = x.max(cut.x1);
        }
        if x < run.x1 {
            out.push(Run::new(run.row, x, run.x1));
        }
    }
    out
}

/// The pixels in both run lists.
fn intersection(a: &[Run], b: &[Run]) -> Vec<Run> {
    let mut out = Vec::new();
    runs::for_each_overlap(a, b, |i, j, _| {
        out.push(Run::new(
            a[i].row,
            a[i].x0.max(b[j].x0),
            a[i].x1.min(b[j].x1),
        ));
    });
    out
}

#[cfg(test)]
#[path = "tests_thin.rs"]
mod tests;
