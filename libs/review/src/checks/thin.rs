/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Features thinner than a pixel (#92): walls, pins, blades, edges and necks
//! narrower than the printer can resolve.
//!
//! Reports [`CheckKind::ThinFeature`] findings, without a suggested fix:
//! only the model itself can be changed.
//!
//! # What is measured
//!
//! The **local thickness** of the solid at a point is the diameter of the
//! widest disc that fits in the solid and covers the point. The disc is
//! round in pixel units, so a wall is measured across X, across Y and on any
//! diagonal alike; on a screen with non-square pixels it is an ellipse in
//! millimetres, the same number of pixels across in every direction.
//!
//! It is measured on the vector contours, not on the printer's pixels: those
//! round every width to whole pixels and drop a feature under a pixel
//! altogether, and those are the worst ones. Exact polygon offsets would
//! need a robust offset and boolean library the review does not have.
//! Instead each layer is cut, one role at a time, along rows a
//! [`subpixels`](ThinFeatureOptions::subpixels)th of a pixel apart (a
//! quarter by default). Along a row the solid is known exactly: the
//! stretches between the contours' crossings. A disc is cut the same way, a
//! stretch of its exact half-width on every row it reaches. *Opening* the
//! rows with a disc as wide as the limit (the union of every placement that
//! fits) leaves out what is thinner than the limit.
//!
//! - **Candidates.** That first pass is exact across the rows. Across them
//!   it errs towards *thin*, by up to a row: for a horizontal wall, and for
//!   a pin whose centre falls between rows. The sub-pixels whose centres it
//!   leaves out are the candidates, kept as row runs of the sub-pixel grid,
//!   and joined into 8-connected pieces.
//! - **A first width.** Opening again with every narrower disc, a whole
//!   number of sub-pixels across, gives each candidate sub-pixel a first,
//!   coarse thickness: exact across the rows to a sub-pixel, and up to three
//!   sub-pixels short along them.
//! - **Measured again.** A piece whose coarse width is too close to the
//!   limit to tell is measured again at 8 of its sub-pixels, on rows and on
//!   columns a sixteenth of a pixel apart around each. That thickness is
//!   exact across X or Y, and within a sixteenth of a pixel for a round pin.
//!   A piece whose median reaches the limit was not thin after all: it is
//!   dropped, and backs its neighbours like any thick part. A layer with
//!   more than 64 such pieces (a lattice near the limit has thousands) opens
//!   them again on columns instead, exact across Y as the rows are across X,
//!   and drops those mostly covered: one pass, however many there are.
//! - **The reported width.** Every feature about to be reported is measured
//!   the same way at up to 16 of its unbacked sub-pixels. Its width is their
//!   median, or for a neck their narrowest, reported in pixels and in
//!   millimetres across its narrow axis.
//!
//! So the limits and the widths hold to a few hundredths of a pixel wherever
//! a feature falls on the pixel grid.
//!
//! # What is reported
//!
//! - **Model features under 2 pixels**
//!   ([`model_width_px`](ThinFeatureOptions::model_width_px)): a warning, or
//!   an error when the feature is under 1 pixel
//!   ([`error_width_px`](ThinFeatureOptions::error_width_px)).
//! - **Support parts under 1 pixel**
//!   ([`support_width_px`](ThinFeatureOptions::support_width_px)), an error,
//!   and only where no model covers them. Support tips are thin on purpose,
//!   so only a support too thin to be sure to print at all is reported. A
//!   round support up to 1.41 pixels across can still miss every pixel
//!   centre; the limit is an option.
//! - **Not the steps of a surface.** A thin sliver lying on thick solid in
//!   the layer below or above it is the crest of a slope or a ridge, or the
//!   rounding of a leaning edge, as the layers cut it: its pixels are
//!   *backed* and do not count. Layer 0 rests on the plate, which backs all
//!   of it. Only unbacked thin pixels make a feature: those of a wall, pin or
//!   blade that stays thin from one layer to the next, or that has nothing
//!   above and below.
//! - **Not the rounding of corners.** A disc cannot reach into a corner, so
//!   every corner leaves a few thin sub-pixels attached to thicker solid. A
//!   thin piece that touches thicker solid in one place is kept only when it
//!   sticks out at least [`min_length_px`](ThinFeatureOptions::min_length_px)
//!   (its area divided by its width): a fin or the edge of a blade, but not a
//!   corner blunter than about 25 degrees (measured: up to 1 pixel of
//!   residue for 60 degrees, 3 for 30, 10 for 10). A piece that touches it
//!   in two places or more, a **neck** joining two parts (two bodies that
//!   only touch included), and a piece that is a whole part of its layer on
//!   its own, such as the section of a pin, are always kept.
//! - **One finding per feature.** Thin pieces of consecutive layers that come
//!   within a layer height (and at least a pixel) of each other are joined
//!   with union-find: a wall 200 layers tall, or leaning up to 45 degrees,
//!   is one finding.
//! - **Large enough to matter.** A feature is reported when it measures
//!   [`min_size`](ThinFeatureOptions::min_size) of its own widths: each
//!   unbacked piece counts its length over its width, and at least 1. So a
//!   pin needs 3 unbacked layers, however thin, and a flat strand 3 times
//!   longer than wide is enough on a single layer. The sharp tip of a cone,
//!   thin over a layer or two, is not worth a finding.
//!
//! # Known limits
//!
//! - A neck a layer or two tall between thick parts is backed on both sides
//!   and goes unreported.
//! - A tip is reported where the layers cut it: a spike thin over several
//!   layers is, the last layer or two of a cone are not, and a sideways tip
//!   (a claw) that ends between two layer planes is only as long as the
//!   layer that cuts nearest to it.
//! - A thin part that touches a part just over the limit across the rows (a
//!   horizontal wall up to a quarter pixel wider than the limit) is one
//!   candidate with it, and is measured together with it.
//! - On a layer with more than 64 pieces near the limit, a round pin up to a
//!   quarter pixel over the limit may be kept as thin: the pass on columns
//!   is exact for walls, not for a pin centred between rows and columns.
//! - The width of a reported feature is measured at up to 16 points, and at
//!   most 4096 per role and plate; past that, a feature keeps its coarse
//!   width.
//! - A feature too small to cover the centre of a sub-pixel (under about a
//!   third of a pixel across, by default) is not seen.

use std::fmt::Write as _;

use rayon::prelude::*;
use sc_sl1cer::contour::ContourLayer;

use crate::finding::{CheckKind, Finding, RoleScope, Severity};
use crate::input::{ReviewInput, Role};
use crate::regions::{Connectivity, label_runs, measure_runs};
use crate::runner::ReviewContext;
use crate::runs::{self, Run};
use crate::union_find::{Groups, RegionRef, UnionFind};

#[path = "thin_lines.rs"]
mod lines;

use lines::{Axis, Disc, Edges, Lines};

/// Settings of the thin-feature check. Widths are in printer pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct ThinFeatureOptions {
    /// Model features narrower than this are reported; 0 skips models.
    pub model_width_px: f64,
    /// Support parts narrower than this are reported; 0 skips supports.
    /// Support tips are thin on purpose, so it is lower than the model's.
    pub support_width_px: f64,
    /// Features narrower than this are errors; wider ones, still under their
    /// role's limit, are warnings.
    pub error_width_px: f64,
    /// Thin pieces of one layer that touch thicker solid in one place and
    /// stick out less than this (their area divided by their width) are the
    /// corners of a wider shape, and are dropped.
    pub min_length_px: f64,
    /// The smallest feature reported, measured in its own widths: each of
    /// its pieces with unbacked thin pixels counts its length divided by its
    /// width, and at least 1 (scaled by the share of it that is unbacked).
    /// So a pin counts its layers and a flat strand its length, and a pin
    /// needs as many layers to be reported whether it is 1.5 or 0.25 pixels
    /// across.
    pub min_size: f64,
    /// How many times finer than the screen, along each axis, the contours
    /// are measured, from 2 to 16. A finer grid sees smaller features and
    /// costs more.
    pub subpixels: u32,
}

impl Default for ThinFeatureOptions {
    fn default() -> Self {
        Self {
            model_width_px: 2.0,
            support_width_px: 1.0,
            error_width_px: 1.0,
            min_length_px: 3.0,
            min_size: 3.0,
            subpixels: 4,
        }
    }
}

/// The range [`ThinFeatureOptions::subpixels`] is held to: at least 2, so a
/// feature under a pixel still covers sub-pixel centres, and at most 16.
const SUBPIXELS: (u32, u32) = (2, 16);

/// Lines per pixel of the second measurement, around each probe.
const PROBE_LINES_PER_PX: f64 = 16.0;

/// Bisection steps of each measurement: the width to 1/4096 of the limit.
const PROBE_STEPS: u32 = 12;

/// How many sub-pixels of a piece near the limit are measured again.
const PROBES: u64 = 8;

/// Up to this many pieces near the limit, per layer and role, are measured
/// again one by one; more are decided together on columns.
const REFINED_PIECES: usize = 64;

/// How many sub-pixels of each piece are kept, spread over it, to measure
/// its feature again when it is reported.
const SAMPLES: u64 = 4;

/// How many sub-pixels of a reported feature are measured again.
const FEATURE_PROBES: usize = 16;

/// At most this many sub-pixels are measured again at report time per role;
/// the features after them keep their coarse width.
const REPORT_PROBES: usize = 4096;

/// Finds the features thinner than the printer resolves.
pub fn run(ctx: &ReviewContext<'_>, options: &ThinFeatureOptions) -> Vec<Finding> {
    let input = ctx.input;
    let subpixels = options.subpixels.clamp(SUBPIXELS.0, SUBPIXELS.1);
    let Some(grid) = FineGrid::new(input, subpixels) else {
        return Vec::new();
    };
    let scale = grid.scale();
    let passes: Vec<Pass> = [
        (Role::Model, options.model_width_px),
        (Role::Support, options.support_width_px),
    ]
    .into_iter()
    .filter(|&(role, width_px)| width_px > 0.0 && input.leaves().iter().any(|l| l.role == role))
    .map(|(role, width_px)| Pass {
        role,
        width_px,
        width: width_px * scale,
        min_length: options.min_length_px.max(0.0) * scale,
    })
    .collect();
    if passes.is_empty() {
        return Vec::new();
    }
    let layers = sweep(input, &grid, &passes);
    let rules = Rules {
        error_width_px: options.error_width_px,
        min_size: options.min_size.max(0.0),
    };
    passes
        .iter()
        .zip(&layers)
        .flat_map(|(pass, pass_layers)| report(input, &grid, pass, pass_layers, &rules))
        .collect()
}

/// The fine grid the contours are measured on: sub-pixels `scale` times
/// smaller than the screen's pixels along each axis.
struct FineGrid {
    scale: u32,
    width: u32,
    height: u32,
}

impl FineGrid {
    fn new(input: &ReviewInput<'_>, scale: u32) -> Option<Self> {
        Some(Self {
            scale,
            width: input.width_px().checked_mul(scale)?,
            height: input.height_px().checked_mul(scale)?,
        })
    }

    /// Sub-pixels per printer pixel along each axis.
    fn scale(&self) -> f64 {
        f64::from(self.scale)
    }

    /// The edges of `contours` on this grid, for lines along `axis`.
    fn edges(&self, input: &ReviewInput<'_>, contours: &[&ContourLayer], axis: Axis) -> Edges {
        Edges::new(
            contours,
            input.pixel_map(),
            self.scale(),
            axis,
            [self.width, self.height],
        )
    }

    /// The raster point (printer pixels) at the centre of sub-pixel `p`.
    fn to_raster(&self, p: [u32; 2]) -> [f64; 2] {
        let s = self.scale();
        [(f64::from(p[0]) + 0.5) / s, (f64::from(p[1]) + 0.5) / s]
    }
}

/// One role's check.
struct Pass {
    role: Role,
    /// The width limit in pixels.
    width_px: f64,
    /// The width limit in sub-pixels: the diameter of the opening disc.
    width: f64,
    /// How far, in sub-pixels, a piece attached to thicker solid in one
    /// place must stick out to be kept.
    min_length: f64,
}

/// What turns features into findings.
struct Rules {
    /// Features narrower than this, in pixels, are errors.
    error_width_px: f64,
    min_size: f64,
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
    /// Sub-pixels by coarse thickness: `widths[d]` of them are covered by a
    /// disc `d` sub-pixels across that fits in the solid on the rows, and by
    /// none wider (`widths[0]`: by none at all).
    widths: Vec<u64>,
    /// The thickness, in sub-pixels, measured again at a few of its
    /// sub-pixels: only for a piece near the limit.
    probes: Vec<f64>,
    /// Sub-pixels spread over it, to measure its feature again.
    samples: Vec<[u32; 2]>,
    /// Its sub-pixel of the least coarse thickness: where a neck breaks.
    narrowest: [u32; 2],
    /// Second moments of the piece, for its direction.
    moments: Moments,
    /// How many separate stretches of the role's thick part it touches,
    /// counting to 2: none for a piece standing alone, one for a fin, an
    /// edge or a corner, two for a neck.
    contacts: u8,
}

impl Piece {
    /// Whether it touches the role's thick part.
    fn attached(&self) -> bool {
        self.contacts > 0
    }

    /// Whether it joins thick parts (or one thick part) in two places.
    fn neck(&self) -> bool {
        self.contacts > 1
    }

    /// Its median thickness in sub-pixels: measured again if it was, coarse
    /// otherwise, and at least one sub-pixel.
    fn width(&self) -> f64 {
        if self.probes.is_empty() {
            f64::from(median_count(&self.widths)).max(1.0)
        } else {
            median(&self.probes)
        }
    }
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
    /// The role's sub-pixels at least as thick as its limit.
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
    (pixels * grid.scale()).ceil() as i64
}

/// Cuts one layer on the fine grid and finds the thin pieces of every pass.
fn measure_layer(
    input: &ReviewInput<'_>,
    grid: &FineGrid,
    passes: &[Pass],
    layer: usize,
) -> Vec<RoleLayer> {
    let model = RoleCut::new(input, grid, layer, Role::Model);
    passes
        .iter()
        .map(|pass| match pass.role {
            Role::Model => thin_pieces(input, grid, pass, &model, None),
            Role::Support => {
                let support = RoleCut::new(input, grid, layer, Role::Support);
                thin_pieces(input, grid, pass, &support, Some(&model.solid))
            }
        })
        .collect()
}

/// One role on one layer, cut along the rows through the sub-pixel centres.
struct RoleCut<'a> {
    /// Every contour of the role on the layer: its leaves together, as the
    /// printer file merges them.
    contours: Vec<&'a ContourLayer>,
    /// The role's edges, to cut rows anywhere.
    rows: Edges,
    /// The solid on the rows through the sub-pixel centres.
    lines: Lines,
    /// Its sub-pixels.
    solid: Vec<Run>,
}

impl<'a> RoleCut<'a> {
    fn new(input: &ReviewInput<'a>, grid: &FineGrid, layer: usize, role: Role) -> Self {
        let contours = role_contours(input, layer, role);
        let rows = grid.edges(input, &contours, Axis::Rows);
        let lines = rows.lines(0.5, 1.0, 0, grid.height as usize);
        let solid = lines.cells(grid.width, false);
        Self {
            contours,
            rows,
            lines,
            solid,
        }
    }
}

/// Every contour of `role` on `layer`.
fn role_contours<'a>(input: &ReviewInput<'a>, layer: usize, role: Role) -> Vec<&'a ContourLayer> {
    input
        .leaves()
        .iter()
        .filter(|leaf| leaf.role == role)
        .map(|leaf| &leaf.stack.layers()[layer])
        .filter(|contour| !contour.polygons().is_empty())
        .collect()
}

/// Opens the role's solid with the pass's disc, measures the pieces the
/// opening removes, without the sub-pixels of `mask` (which another role
/// prints anyway), and keeps those that are thin features.
fn thin_pieces(
    input: &ReviewInput<'_>,
    grid: &FineGrid,
    pass: &Pass,
    cut: &RoleCut<'_>,
    mask: Option<&[Run]>,
) -> RoleLayer {
    let opened = lines::open(&cut.lines, &Disc::new(0.5 * pass.width, 1.0));
    let thick = opened.cells(grid.width, true);
    let mut thin = difference(&cut.solid, &thick);
    if let Some(mask) = mask {
        thin = difference(&thin, mask);
    }
    if thin.is_empty() {
        return RoleLayer {
            thick,
            ..RoleLayer::default()
        };
    }
    let (labels, count) = label_runs(&thin, Connectivity::Eight);
    let regions = measure_runs(&thin, &labels, count, grid.width, grid.height);
    let levels = pass.width.ceil() as usize;
    let pieces: Vec<Piece> = regions
        .iter()
        .zip(contacts(&thin, &labels, count, &thick))
        .map(|(region, contacts)| Piece {
            area: region.area_px,
            unbacked: 0,
            inside: region.inside_px,
            widths: vec![0; levels],
            probes: Vec::new(),
            samples: Vec::new(),
            narrowest: region.inside_px,
            moments: Moments::default(),
            contacts,
        })
        .collect();
    // A piece attached in one place sticks out at most its area over one
    // sub-pixel: the smaller ones are corners whatever their width.
    let keep: Vec<bool> = pieces
        .iter()
        .map(|p| p.contacts != 1 || p.area as f64 >= pass.min_length)
        .collect();
    let (thin, labels, mut pieces) = retain(&thin, &labels, pieces, &keep);
    coarse_widths(grid, cut, &thin, &labels, &mut pieces);
    for (run, &label) in thin.iter().zip(&labels) {
        pieces[label as usize].moments.add(run);
    }
    let samples = spread(&thin, &labels, &pieces_areas(&pieces), |_| SAMPLES);
    for (piece, samples) in pieces.iter_mut().zip(samples) {
        piece.samples = samples;
    }
    // Measured again, a piece as thick as the limit is not a thin feature,
    // and backs its neighbours as the rest of the thick part does.
    let thick_after_all = decide_near_limit(input, grid, pass, cut, &thin, &labels, &mut pieces);
    let dropped: Vec<Run> = thin
        .iter()
        .zip(&labels)
        .filter(|&(_, &label)| thick_after_all[label as usize])
        .map(|(run, _)| *run)
        .collect();
    let thick = if dropped.is_empty() {
        thick
    } else {
        runs::union(&thick, &dropped)
    };
    let keep: Vec<bool> = pieces
        .iter()
        .zip(&thick_after_all)
        .map(|(p, &thick)| {
            !thick && (p.contacts != 1 || p.area as f64 >= pass.min_length * p.width())
        })
        .collect();
    let (thin, labels, pieces) = retain(&thin, &labels, pieces, &keep);
    RoleLayer {
        thick,
        thin,
        labels,
        pieces,
    }
}

/// The area of every piece.
fn pieces_areas(pieces: &[Piece]) -> Vec<u64> {
    pieces.iter().map(|p| p.area).collect()
}

/// Gives every sub-pixel of the pieces a coarse thickness, on the rows: the
/// widest disc of a whole number of sub-pixels across, under the limit, that
/// fits in the solid and covers it. Fills each piece's `widths` and
/// `narrowest`.
fn coarse_widths(
    grid: &FineGrid,
    cut: &RoleCut<'_>,
    thin: &[Run],
    labels: &[u32],
    pieces: &mut [Piece],
) {
    let Some(levels) = pieces.first().map(|p| p.widths.len()) else {
        return;
    };
    // A disc covering a thin sub-pixel lies within the limit of it: the
    // solid around the thin pieces is all the openings need. The region is
    // cut a sub-pixel wider, so that its own edge is never the solid's.
    let around = dilate_square(thin, levels as u32 + 1, grid.width, grid.height);
    let near = cut.lines.clipped_to(&around);
    let thin_rows = Rows::new(thin);
    let mut assigned: Vec<Run> = Vec::new();
    for d in (1..levels).rev() {
        let opened = lines::open(&near, &Disc::new(0.5 * d as f64, 1.0));
        let covered = intersection(&opened.cells(grid.width, true), thin);
        for run in difference(&covered, &assigned) {
            let piece = &mut pieces[thin_rows.label_at(labels, &run) as usize];
            piece.widths[d] += u64::from(run.len());
            // Levels go down: the last one written is the narrowest.
            piece.narrowest = [run.x0, run.row];
        }
        assigned = runs::union(&assigned, &covered);
    }
    for run in difference(thin, &assigned) {
        pieces[thin_rows.label_at(labels, &run) as usize].narrowest = [run.x0, run.row];
    }
    for piece in pieces.iter_mut() {
        let wider: u64 = piece.widths.iter().sum();
        piece.widths[0] = piece.area - wider;
    }
}

/// Decides whether the pieces whose coarse width is too close to the limit
/// to tell are thick after all, and returns it for every piece. When there
/// are at most [`REFINED_PIECES`] of them, each is measured again at
/// [`PROBES`] of its sub-pixels, and its median decides. When there are
/// more, as on a lattice, they are opened again on columns, exact across Y
/// as the rows are across X, and a piece mostly covered is thick.
fn decide_near_limit(
    input: &ReviewInput<'_>,
    grid: &FineGrid,
    pass: &Pass,
    cut: &RoleCut<'_>,
    thin: &[Run],
    labels: &[u32],
    pieces: &mut [Piece],
) -> Vec<bool> {
    let near: Vec<bool> = pieces
        .iter()
        .map(|p| {
            // The coarse width is short by under a sub-pixel across the rows,
            // and by up to three along them.
            let slack = if p.moments.thin_across_rows() {
                1.0
            } else {
                3.0
            };
            f64::from(median_count(&p.widths)) + slack >= pass.width
        })
        .collect();
    let count = near.iter().filter(|&&n| n).count();
    if count == 0 {
        return vec![false; pieces.len()];
    }
    let columns = grid.edges(input, &cut.contours, Axis::Columns);
    if count <= REFINED_PIECES {
        let measure = Measure::new(grid, pass, &cut.rows, &columns);
        let cells = spread(thin, labels, &pieces_areas(pieces), |p| {
            if near[p] { PROBES } else { 0 }
        });
        for (piece, cells) in pieces.iter_mut().zip(cells) {
            piece.probes = cells.into_iter().map(|cell| measure.at(cell)).collect();
        }
        return pieces
            .iter()
            .map(|p| !p.probes.is_empty() && median(&p.probes) >= pass.width)
            .collect();
    }
    let covered = covered_on_columns(grid, pass, &columns, thin, labels, &near);
    pieces
        .iter()
        .zip(covered)
        .zip(&near)
        .map(|((p, covered), &near)| near && 2 * covered >= p.area)
        .collect()
}

/// For every piece marked in `near`, how many of its sub-pixels the
/// opening with the pass's disc covers on the columns: only the columns
/// those pieces span, and the limit around them, are cut.
fn covered_on_columns(
    grid: &FineGrid,
    pass: &Pass,
    columns: &Edges,
    thin: &[Run],
    labels: &[u32],
    near: &[bool],
) -> Vec<u64> {
    let margin = pass.width.ceil() as u32 + 2;
    let mut spans: Vec<(u32, u32)> = thin
        .iter()
        .zip(labels)
        .filter(|&(_, &label)| near[label as usize])
        .map(|(run, _)| {
            (
                run.x0.saturating_sub(margin),
                (run.x1 + margin).min(grid.width),
            )
        })
        .collect();
    spans.sort_unstable();
    let mut merged: Vec<(u32, u32)> = Vec::new();
    for (a, b) in spans {
        match merged.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    let disc = Disc::new(0.5 * pass.width, 1.0);
    let opened: Vec<Lines> = merged
        .iter()
        .map(|&(a, b)| {
            let cut = columns.lines(0.5, 1.0, i64::from(a), (b - a) as usize);
            lines::open(&cut, &disc)
        })
        .collect();
    let mut covered = vec![0u64; near.len()];
    for (run, &label) in thin.iter().zip(labels) {
        if !near[label as usize] {
            continue;
        }
        let y = f64::from(run.row) + 0.5;
        for x in run.x0..run.x1 {
            let span = merged.partition_point(|&(a, _)| a <= x) - 1;
            let line = opened[span].line(i64::from(x));
            let at = line.partition_point(|s| s.1 + lines::EPS < y);
            if line.get(at).is_some_and(|s| s.0 - lines::EPS <= y) {
                covered[label as usize] += 1;
            }
        }
    }
    covered
}

/// For every piece, up to `wanted(piece)` of its sub-pixels, spread evenly
/// over it in scan order: the middle one of each of as many equal shares.
fn spread(
    thin: &[Run],
    labels: &[u32],
    areas: &[u64],
    wanted: impl Fn(usize) -> u64,
) -> Vec<Vec<[u32; 2]>> {
    let wanted: Vec<u64> = (0..areas.len()).map(|p| wanted(p).min(areas[p])).collect();
    let mut out = vec![Vec::new(); areas.len()];
    let mut seen = vec![0u64; areas.len()];
    for (run, &label) in thin.iter().zip(labels) {
        let p = label as usize;
        let (start, end) = (seen[p], seen[p] + u64::from(run.len()));
        while (out[p].len() as u64) < wanted[p] {
            let j = out[p].len() as u64;
            let target = (2 * j + 1) * areas[p] / (2 * wanted[p]);
            if target >= end {
                break;
            }
            out[p].push([run.x0 + (target - start) as u32, run.row]);
        }
        seen[p] = end;
    }
    out
}

/// For every piece, how many separate stretches of `thick` it touches
/// (8-adjacency), counting to 2: the thick sub-pixels next to it, grouped
/// by their own connectivity.
fn contacts(thin: &[Run], labels: &[u32], count: usize, thick: &[Run]) -> Vec<u8> {
    let rows = Rows::new(thick);
    let mut touched: Vec<(u32, Run)> = Vec::new();
    for (run, &label) in thin.iter().zip(labels) {
        let (x0, x1) = (run.x0.saturating_sub(1), run.x1 + 1);
        for row in (i64::from(run.row) - 1)..=(i64::from(run.row) + 1) {
            let found = rows.get(row);
            let from = found.partition_point(|r| r.x1 <= x0);
            for other in found[from..].iter().take_while(|r| r.x0 < x1) {
                touched.push((
                    label,
                    Run::new(row as u32, other.x0.max(x0), other.x1.min(x1)),
                ));
            }
        }
    }
    touched.sort_unstable();
    let mut out = vec![0u8; count];
    let mut merged: Vec<Run> = Vec::new();
    for group in touched.chunk_by(|a, b| a.0 == b.0) {
        merged.clear();
        for &(_, run) in group {
            match merged.last_mut() {
                Some(last) if last.row == run.row && run.x0 <= last.x1 => {
                    last.x1 = last.x1.max(run.x1);
                }
                _ => merged.push(run),
            }
        }
        let (_, stretches) = label_runs(&merged, Connectivity::Eight);
        out[group[0].0 as usize] = stretches.min(2) as u8;
    }
    out
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

/// The second, local measurement of the thickness.
struct Measure<'a> {
    rows: &'a Edges,
    columns: &'a Edges,
    /// The spacing of its lines, in sub-pixels.
    step: f64,
    /// How far around the point the solid matters, in sub-pixels.
    reach: f64,
    /// The widest thickness measured: the limit.
    max: f64,
}

impl<'a> Measure<'a> {
    /// The measurement of `pass` on `grid`, with the role's edges laid out
    /// for rows and for columns.
    fn new(grid: &FineGrid, pass: &Pass, rows: &'a Edges, columns: &'a Edges) -> Self {
        Self {
            rows,
            columns,
            step: grid.scale() / PROBE_LINES_PER_PX,
            // A disc covering the point lies within its diameter of it.
            reach: pass.width + 1.0,
            max: pass.width,
        }
    }

    /// The local thickness at the centre of sub-pixel `cell`, in
    /// sub-pixels: the larger of the measures on rows and on columns, each
    /// exact across its lines' direction.
    fn at(&self, cell: [u32; 2]) -> f64 {
        let [x, y] = [f64::from(cell[0]) + 0.5, f64::from(cell[1]) + 0.5];
        let on_rows = self.rows.window(x, y, self.reach, self.step);
        let t = on_rows.thickness(self.max, PROBE_STEPS);
        if t >= self.max {
            return t;
        }
        let on_columns = self.columns.window(y, x, self.reach, self.step);
        t.max(on_columns.thickness(self.max, PROBE_STEPS))
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
    let mut budget = REPORT_PROBES;
    let mut findings = Vec::new();
    for members in groups.iter() {
        let piece = |m: &RegionRef| &layers[m.layer].pieces[m.index as usize];
        let unbacked: Vec<&RegionRef> = members.iter().filter(|m| piece(m).unbacked > 0).collect();
        if unbacked.is_empty() {
            continue;
        }
        let focus = *unbacked
            .iter()
            .max_by(|a, b| {
                piece(a)
                    .unbacked
                    .cmp(&piece(b).unbacked)
                    .then(b.layer.cmp(&a.layer))
            })
            .expect("a feature with unbacked pixels");
        let neck = piece(focus).neck();
        let measured = measure_feature(input, grid, pass, layers, &unbacked, neck, &mut budget);
        let width = if measured.is_empty() {
            // Over the budget: the coarse widths.
            let coarse: Vec<(f64, f64)> = unbacked
                .iter()
                .map(|m| (piece(m).width(), piece(m).unbacked as f64))
                .collect();
            weighted_median(&coarse)
        } else if neck {
            // A neck breaks where it is narrowest.
            measured
                .iter()
                .map(|&(t, _)| t)
                .fold(f64::INFINITY, f64::min)
        } else {
            weighted_median(&measured)
        };
        let width_px = hundredths(width / grid.scale());
        // Measured to the limit: as reported, it would not be under it.
        if width_px.is_nan() || width_px >= pass.width_px {
            continue;
        }
        // Each unbacked piece counts its length in widths, at least 1: a pin
        // counts its layers, a strand its length.
        let size: f64 = unbacked
            .iter()
            .map(|m| {
                let p = piece(m);
                let widths = p.moments.length() / width.max(1.0);
                p.unbacked as f64 / p.area as f64 * widths.max(1.0)
            })
            .sum();
        if size < rules.min_size {
            continue;
        }
        let feature = Feature {
            first: members[0].layer,
            last: members
                .iter()
                .map(|m| m.layer)
                .max()
                .unwrap_or(members[0].layer),
            width,
            width_px,
            neck,
            focus_layer: focus.layer,
            focus: piece(focus),
        };
        findings.push(feature.finding(input, grid, pass, rules));
    }
    findings
}

/// The thickness of a feature at up to [`FEATURE_PROBES`] of the sub-pixels
/// of its `unbacked` pieces, each with the share of their unbacked
/// sub-pixels it stands for: those of pieces measured again already, and
/// samples of the others measured now, while `budget` lasts. For a neck,
/// the samples are its pieces' narrowest sub-pixels. Empty when nothing
/// could be measured.
fn measure_feature(
    input: &ReviewInput<'_>,
    grid: &FineGrid,
    pass: &Pass,
    layers: &[LayerPieces],
    unbacked: &[&RegionRef],
    neck: bool,
    budget: &mut usize,
) -> Vec<(f64, f64)> {
    let piece = |m: &RegionRef| &layers[m.layer].pieces[m.index as usize];
    let mut out: Vec<(f64, f64)> = Vec::new();
    // (layer, sub-pixel, share) of the samples still to measure.
    let mut samples: Vec<(usize, [u32; 2], f64)> = Vec::new();
    for m in unbacked {
        let p = piece(m);
        if p.probes.is_empty() {
            let cells = if neck {
                vec![p.narrowest]
            } else {
                p.samples.clone()
            };
            let share = p.unbacked as f64 / cells.len() as f64;
            samples.extend(cells.into_iter().map(|cell| (m.layer, cell, share)));
        } else {
            let share = p.unbacked as f64 / p.probes.len() as f64;
            out.extend(p.probes.iter().map(|&t| (t, share)));
        }
    }
    let wanted = FEATURE_PROBES
        .saturating_sub(out.len())
        .min(samples.len())
        .min(*budget);
    if wanted == 0 {
        if out.is_empty() || samples.is_empty() {
            return out;
        }
        // The samples left unmeasured stand with the probes taken.
        let total: f64 =
            samples.iter().map(|s| s.2).sum::<f64>() + out.iter().map(|o| o.1).sum::<f64>();
        let taken: f64 = out.iter().map(|o| o.1).sum();
        for o in &mut out {
            o.1 *= total / taken;
        }
        return out;
    }
    *budget -= wanted;
    // The samples measured stand for all of them.
    let stand = samples.len() as f64 / wanted as f64;
    let mut cuts: Vec<(usize, Edges, Edges)> = Vec::new();
    for j in 0..wanted {
        let (layer, cell, share) = samples[(2 * j + 1) * samples.len() / (2 * wanted)];
        let at = match cuts.iter().position(|c| c.0 == layer) {
            Some(at) => at,
            None => {
                let contours = role_contours(input, layer, pass.role);
                cuts.push((
                    layer,
                    grid.edges(input, &contours, Axis::Rows),
                    grid.edges(input, &contours, Axis::Columns),
                ));
                cuts.len() - 1
            }
        };
        let (_, rows, columns) = &cuts[at];
        let t = Measure::new(grid, pass, rows, columns).at(cell);
        out.push((t, share * stand));
    }
    out
}

/// One feature, ready to report.
struct Feature<'a> {
    first: usize,
    last: usize,
    /// Its width in sub-pixels.
    width: f64,
    /// Its width in pixels, to a hundredth.
    width_px: f64,
    /// Whether it joins thick parts in two places.
    neck: bool,
    focus_layer: usize,
    /// Its piece with the most unbacked sub-pixels, where the camera goes.
    focus: &'a Piece,
}

/// How many times longer than wide a piece is at least to be called a wall
/// rather than a pin or a neck.
const WALL_LENGTH: f64 = 3.0;

impl Feature<'_> {
    fn finding(
        &self,
        input: &ReviewInput<'_>,
        grid: &FineGrid,
        pass: &Pass,
        rules: &Rules,
    ) -> Finding {
        let (severity, limit) = if self.width_px < rules.error_width_px {
            (Severity::Error, rules.error_width_px)
        } else {
            (Severity::Warning, pass.width_px)
        };
        let width_mm = self.width_px * self.focus.moments.mm_per_px_across(input.pixel_size_mm());
        let long = self.focus.area as f64 >= WALL_LENGTH * self.width * self.width;
        let (noun, scope) = match pass.role {
            Role::Support => ("support", RoleScope::Support),
            Role::Model if self.neck && !long => ("neck", RoleScope::Model),
            // Thinning out of thicker solid: the edge of a blade, or a tip.
            Role::Model if self.focus.attached() && !self.neck && !is_even(&self.focus.widths) => {
                ("edge", RoleScope::Model)
            }
            Role::Model if long => ("wall", RoleScope::Model),
            Role::Model => ("pin", RoleScope::Model),
        };
        let limit = pixels(limit);
        let title = format!("{} thinner than {limit}", capitalised(noun));
        let detail = format!(
            "This {noun} is {width_mm:.3} mm ({} px) wide, under {limit}: it will not print.",
            number(self.width_px)
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

/// The lower median of `values`, or 0 when there are none.
fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[(sorted.len() - 1) / 2]
}

/// The smallest value with at least half of the total weight at or below
/// it, from `(value, weight)` pairs; 0 when there are none.
fn weighted_median(entries: &[(f64, f64)]) -> f64 {
    let mut sorted = entries.to_vec();
    sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total: f64 = sorted.iter().map(|&(_, w)| w).sum();
    let mut seen = 0.0;
    for &(value, weight) in &sorted {
        seen += weight;
        if 2.0 * seen >= total {
            return value;
        }
    }
    sorted.last().map_or(0.0, |&(value, _)| value)
}

/// `value` rounded to a hundredth, as it is reported.
fn hundredths(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// The coarse thickness at least half the sub-pixels counted in `widths`
/// reach: the smallest `d` with half of them at `d` or narrower.
fn median_count(widths: &[u64]) -> u32 {
    let total: u64 = widths.iter().sum();
    let mut seen = 0;
    for (width, &count) in widths.iter().enumerate() {
        seen += count;
        if 2 * seen >= total && seen > 0 {
            return width as u32;
        }
    }
    0
}

/// Whether three quarters of the sub-pixels counted in `widths` are within
/// a sub-pixel of their median: a wall, a fin or a neck of even thickness,
/// rather than an edge that tapers out of thicker solid.
fn is_even(widths: &[u64]) -> bool {
    let total: u64 = widths.iter().sum();
    let middle = median_count(widths) as usize;
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

    /// The length of the piece along its long axis, in sub-pixels: that of
    /// a rectangle with the same spread, so a run of `n` sub-pixels is `n`
    /// long.
    fn length(&self) -> f64 {
        if self.n == 0.0 {
            return 0.0;
        }
        let (mx, my) = (self.x / self.n, self.y / self.n);
        let a = self.xx / self.n - mx * mx;
        let c = self.yy / self.n - my * my;
        let b = self.xy / self.n - mx * my;
        let spread = 0.5 * (a + c) + (0.5 * (a - c)).hypot(b);
        // The centres' spread, plus that of a sub-pixel's own extent.
        12.0f64.mul_add(spread, 1.0).sqrt()
    }

    /// Whether the piece is longer than wide with its narrow axis within 45
    /// degrees of the rows: a width measured along the rows is exact for it.
    fn thin_across_rows(&self) -> bool {
        if self.n == 0.0 {
            return false;
        }
        let (mx, my) = (self.x / self.n, self.y / self.n);
        let a = self.xx / self.n - mx * mx;
        let c = self.yy / self.n - my * my;
        let b = self.xy / self.n - mx * my;
        let half = 0.5 * (a - c);
        let root = half.hypot(b);
        let (long, narrow) = (0.5 * (a + c) + root, 0.5 * (a + c) - root);
        // Along the narrow axis the spread is `narrow`; across X it is `a`.
        long >= 4.0 * narrow && a <= 0.5 * (long + narrow)
    }

    /// Millimetres per pixel of thickness across the piece, along its narrow
    /// axis. On a screen with non-square pixels it depends on the direction:
    /// a strip whose normal is `n` in pixel space, with pixels `a` by `b`
    /// millimetres, is `1 / |(n.x / a, n.y / b)|` millimetres thick per pixel
    /// of thickness.
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
        let (nx, ny) = (-angle.sin(), angle.cos());
        1.0 / (nx / pixel_mm[0]).hypot(ny / pixel_mm[1])
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

    /// The label of the run that holds `run`, which lies inside one.
    fn label_at(&self, labels: &[u32], run: &Run) -> u32 {
        let (offset, found) = self.get_with_offset(i64::from(run.row));
        let i = found.partition_point(|r| r.x1 <= run.x0);
        labels[offset + i]
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

/// Every sub-pixel within `r` of `runs` along both axes (a square), clipped
/// to a grid `width` by `height`.
fn dilate_square(runs: &[Run], r: u32, width: u32, height: u32) -> Vec<Run> {
    let rows = Rows::new(runs);
    let Some((first, last)) = rows.span() else {
        return Vec::new();
    };
    let r = i64::from(r);
    let mut out = Vec::new();
    let mut spans: Vec<(u32, u32)> = Vec::new();
    for y in (first - r).max(0)..=(last + r).min(i64::from(height) - 1) {
        spans.clear();
        for source in (y - r)..=(y + r) {
            spans.extend(rows.get(source).iter().map(|run| {
                (
                    (i64::from(run.x0) - r).max(0) as u32,
                    (i64::from(run.x1) + r).min(i64::from(width)) as u32,
                )
            }));
        }
        spans.sort_unstable();
        let mut spans = spans.iter();
        let Some(&(mut x0, mut x1)) = spans.next() else {
            continue;
        };
        for &(a, b) in spans {
            if a <= x1 {
                x1 = x1.max(b);
            } else {
                out.push(Run::new(y as u32, x0, x1));
                (x0, x1) = (a, b);
            }
        }
        out.push(Run::new(y as u32, x0, x1));
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

#[cfg(test)]
#[path = "tests_thin.rs"]
mod tests;
