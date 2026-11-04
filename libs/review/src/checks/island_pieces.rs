/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The pieces the island check works on: every layer's solid cut at its weak
//! joints, and how the pieces of consecutive layers rest on one another.
//!
//! A *weak joint* is a place where the solid of a layer is narrower than
//! [`MIN_JOINT_PX`] pixels between two parts much thicker than it: a contact
//! through a corner pixel, or a neck one or two pixels wide. It holds next to
//! nothing of the peel force on what hangs from it, so the parts it joins
//! are judged apart. The cut is an opening with a 3 x 3 square:
//!
//! 1. The *cores* are the 8-connected parts of the layer's erosion: the solid
//!    pixels whose 8 neighbours are all solid. Two parts joined through less
//!    than 3 pixels have separate cores.
//! 2. Every solid pixel within the 3 x 3 neighbourhood of a core pixel belongs
//!    to that core. A pixel next to two cores, which only happens at a weak
//!    joint, goes to the core whose stretch of the row starts first.
//! 3. The rest is *thin*: features under 3 pixels wide, in 8-connected thin
//!    parts.
//! 4. A core stands as a piece of its own, an *anchor*, only when its part
//!    is at least [`MIN_ANCHOR_PX`] pixels thick somewhere (it holds a 5 x 5
//!    square). A joint is weak only next to what is clearly thicker than it:
//!    a strip or a diagonal brace 3 or 4 pixels wide that narrows here and
//!    there is thin all along, not a chain of parts on weak joints. Every
//!    other part (thin, or a core of a part under 5 pixels thick) clusters
//!    with the non-anchor parts it touches, and the cluster joins the lowest
//!    anchor it touches, or is a piece of its own when it touches none.
//!
//! So two thick parts that meet through less than 3 pixels are two pieces;
//! a thin feature stays with the piece it hangs from, and a part that is thin
//! all over (a support stem, a stray pixel, a narrow strip) is one piece.
//!
//! 5. A piece that the cut leaves no larger than the given size (the tip's
//!    contact, [`WEAK_JOINT_KEEP_MM2`](super::island_policy::WEAK_JOINT_KEEP_MM2),
//!    #404) joins the largest piece it touches: pulled apart by a joint of a
//!    pixel or two, a speck that small is part of what it touches.
//!
//! # Contacts
//!
//! A piece rests on the pieces of the layer below through the pixels they
//! share straight above one another. Where a pixel has nothing below it, the
//! exposure of its layer cures through the liquid down to solid a layer or
//! more below, as deep as the cure depth reaches (#403): it rests on that
//! solid as well, through a contact that names how many layers below it is.
//!
//! Piece indices are deterministic: anchors in scan order, then the clusters
//! that touch no anchor, by their first part (cores before thin parts, each
//! in scan order). [`LayerPieces::compute`] gives the same pieces every
//! time, so a layer can be cut again to look at its pixels.

use rayon::prelude::*;

use super::island_policy::{HEIGHT_TOLERANCE_MM, MAX_CURED_GAP_LAYERS};
use crate::input::ReviewInput;
use crate::regions::{Connectivity, LayerPixels, label_runs};
use crate::runs::{Run, for_each_overlap};
use crate::union_find::UnionFind;

/// The narrowest joint, in pixels, that holds two parts of a layer together:
/// 3. A corner contact or a neck of 1 or 2 pixels is a weak joint.
///
/// One pixel is 19 to 50 µm on current resin printers, so a weak joint is at
/// most a tenth of a millimetre wide: it carries next to nothing of the peel
/// force on what hangs from it. Rasterising a contour also lights such
/// joints where two parts merely graze each other.
pub const MIN_JOINT_PX: u32 = 3;

/// How thick, in pixels, a part must be somewhere to stand alone across a
/// weak joint: 5, so it holds a 5 x 5 square and is at least two pixels
/// thicker than the widest weak joint on each side.
///
/// A diagonal brace or a strip 3 or 4 pixels wide narrows below 3 pixels
/// here and there from rasterisation alone; cut there, it would fall apart
/// into specks of a few pixels, each resting on little. On a 1.9 M-triangle
/// miniature with generated supports, cutting such features gave islands of
/// 7 to 123 pixels along braces and thin strips that are held as a whole.
pub const MIN_ANCHOR_PX: u32 = 5;

/// A layer's runs split where model starts and stops, each with its piece
/// and whether it is model.
type RoleSplit = (Vec<Run>, Vec<(u32, bool)>);

/// The statistics of one piece.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Piece {
    /// Pixels in the piece.
    pub area_px: u64,
    /// Pixels of it that are model.
    pub model_px: u64,
    /// Pixels of it that are support. Model and support may share pixels.
    pub support_px: u64,
}

/// How a piece of one layer rests on a piece of a layer below: the pixels
/// they share straight above one another, with nothing solid between them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Contact {
    /// How many layers below the piece of the upper layer it rests on: 1
    /// for the layer right below, more through cured liquid (#403).
    pub below: u8,
    /// The piece of the lower layer.
    pub lower: u32,
    /// The piece of the upper layer.
    pub upper: u32,
    /// Shared pixels where the lower pixel is model.
    pub model_px: u64,
    /// Shared pixels where the lower pixel is support and not model.
    pub support_px: u64,
}

/// One layer's solid cut into pieces.
#[derive(Clone, Debug, Default)]
pub(super) struct LayerPieces {
    /// The solid pixels, sorted by `(row, x0)`. Every run lies in one piece;
    /// runs of different pieces may touch.
    pub runs: Vec<Run>,
    /// The piece of each run.
    pub labels: Vec<u32>,
    /// Every piece's statistics, by index.
    pub pieces: Vec<Piece>,
    /// The model pixels of the layer.
    pub model: Vec<Run>,
    /// The runs split where model starts and stops, each with its piece and
    /// whether it is model: what the layers above rest on, made once.
    split: std::sync::OnceLock<RoleSplit>,
}

impl LayerPieces {
    /// Rasterises layer `layer` of `input` and cuts it into pieces, a piece
    /// cut off with no more than `keep_px` pixels joining the largest piece
    /// it touches.
    pub(super) fn compute(input: &ReviewInput<'_>, layer: usize, keep_px: u64) -> Self {
        let _clock = crate::clock::scope("pieces.compute");
        Self::cut(LayerPixels::compute(input, layer), keep_px)
    }

    /// Cuts the solid of `pixels` into pieces (see [`Self::compute`]).
    pub(super) fn cut(pixels: LayerPixels, keep_px: u64) -> Self {
        let _clock = crate::clock::scope("pieces.cut");
        let (runs, labels, count) = cut(
            &pixels.solid,
            &pixels.solid_labels,
            pixels.solid_count,
            keep_px,
        );
        let mut pieces = vec![Piece::default(); count];
        for (run, &label) in runs.iter().zip(&labels) {
            pieces[label as usize].area_px += u64::from(run.len());
        }
        for_each_overlap(&runs, &pixels.model, |i, _, px| {
            pieces[labels[i] as usize].model_px += u64::from(px);
        });
        for_each_overlap(&runs, &pixels.support, |i, _, px| {
            pieces[labels[i] as usize].support_px += u64::from(px);
        });
        Self {
            runs,
            labels,
            pieces,
            model: pixels.model,
            split: std::sync::OnceLock::new(),
        }
    }

    /// The runs of piece `piece`, in order.
    pub(super) fn runs_of(&self, piece: u32) -> Vec<Run> {
        self.runs
            .iter()
            .zip(&self.labels)
            .filter(|(_, label)| **label == piece)
            .map(|(run, _)| *run)
            .collect()
    }
}

/// Every layer's pieces and the contacts between consecutive layers: what
/// the island check reads. Pixels are not kept.
#[derive(Clone, Debug, Default)]
pub(super) struct PieceScan {
    /// Every layer's pieces, from the plate up.
    pub layers: Vec<Vec<Piece>>,
    /// `contacts[k]`: how the pieces of layer `k` rest on those of the
    /// layers below, sorted by `(below, lower, upper)`. Empty for the layers
    /// that rest on the plate, which need none.
    pub contacts: Vec<Vec<Contact>>,
    /// `touches[k]`: for every piece of layer `k` that rests on nothing, the
    /// pieces of layer `k - 1` it touches through an edge or a corner, as
    /// `(upper, lower)` pairs, sorted. Empty where `contacts` is.
    pub touches: Vec<Vec<(u32, u32)>>,
}

/// The contacts and touches of one layer with the layers below it.
type Links = (Vec<Contact>, Vec<(u32, u32)>);

/// How many layers of liquid below layer `layer` of `input` its exposure
/// cures through to the solid under them (#403): the most layers `g` such
/// that the layer and the `g` below it are no thicker than `cure_depth_mm`,
/// at most [`MAX_CURED_GAP_LAYERS`].
pub(super) fn cured_gap_layers(input: &ReviewInput<'_>, layer: usize, cure_depth_mm: f64) -> usize {
    let layers = input.layers();
    let Some(own) = layers.get(layer) else {
        return 0;
    };
    let mut depth = own.height_mm();
    let mut gap = 0;
    while gap < MAX_CURED_GAP_LAYERS && gap < layer {
        depth += layers[layer - gap - 1].height_mm();
        if depth > cure_depth_mm + HEIGHT_TOLERANCE_MM {
            break;
        }
        gap += 1;
    }
    gap
}

/// Cuts every layer of `input` into pieces, a piece of no more than
/// `keep_px` pixels cut off at a weak joint staying with what it touches,
/// and links the layers from layer `first_linked` up to those below them:
/// the layer right below, and as many more below a pixel with nothing under
/// it as `cure_depth_mm` reaches through.
///
/// Layers are processed in parallel, a batch of a few per thread at a time,
/// so only one batch of pixels is held in memory at once (and the few layers
/// below it that its contacts reach). The result does not depend on the
/// number of threads.
pub(super) fn scan(
    input: &ReviewInput<'_>,
    first_linked: usize,
    keep_px: u64,
    cure_depth_mm: f64,
) -> PieceScan {
    let _clock = crate::clock::scope("islands.scan");
    let count = input.layer_count();
    let batch = (rayon::current_num_threads() * 4).max(8);
    let mut scan = PieceScan {
        layers: Vec::with_capacity(count),
        contacts: Vec::with_capacity(count),
        touches: Vec::with_capacity(count),
    };
    // The last layers of the batch before, nearest last: the layers below
    // the first layers of a batch.
    let mut previous: Vec<LayerPieces> = Vec::new();
    let mut start = 0;
    while start < count {
        let end = (start + batch).min(count);
        let cut: Vec<LayerPieces> = (start..end)
            .into_par_iter()
            .map(|layer| LayerPieces::compute(input, layer, keep_px))
            .collect();
        let links: Vec<Links> = (0..cut.len())
            .into_par_iter()
            .map(|i| {
                let layer = start + i;
                if layer < first_linked || layer == 0 {
                    return Default::default();
                }
                let reach = 1 + cured_gap_layers(input, layer, cure_depth_mm);
                let lowers: Vec<&LayerPieces> = (1..=reach.min(layer))
                    .map_while(|below| {
                        if below <= i {
                            Some(&cut[i - below])
                        } else {
                            previous.len().checked_sub(below - i).map(|k| &previous[k])
                        }
                    })
                    .collect();
                if lowers.is_empty() {
                    return Default::default();
                }
                let contacts = contacts(&lowers, &cut[i]);
                let touches = touches(lowers[0], &cut[i], &contacts);
                (contacts, touches)
            })
            .collect();
        for (layer, (contacts, touches)) in cut.iter().zip(links) {
            scan.layers.push(layer.pieces.clone());
            scan.contacts.push(contacts);
            scan.touches.push(touches);
        }
        previous.extend(cut);
        let keep = MAX_CURED_GAP_LAYERS + 1;
        if previous.len() > keep {
            previous.drain(..previous.len() - keep);
        }
        start = end;
    }
    scan
}

/// Cuts `solid`, whose runs lie in the 8-connected regions `regions` (of
/// `region_count`), into pieces: runs covering exactly `solid`, sorted by
/// `(row, x0)`, each within one piece; the piece of each run; the number of
/// pieces. A region with at most one core is one piece, numbered first in
/// region order; the pieces of the other regions follow. A piece of no more
/// than `keep_px` pixels joins the largest piece it touches.
fn cut(
    solid: &[Run],
    regions: &[u32],
    region_count: usize,
    keep_px: u64,
) -> (Vec<Run>, Vec<u32>, usize) {
    // A region with at most one core is one piece whatever its thin parts:
    // they all join that core, or each other. Only regions with several
    // cores need the whole cut, which costs as much as the erosion again.
    let cores = erode(solid);
    let (core_labels, core_count) = label_runs(&cores, Connectivity::Eight);
    let mut region_of_core = vec![0u32; core_count];
    for_each_overlap(&cores, solid, |i, j, _| {
        region_of_core[core_labels[i] as usize] = regions[j];
    });
    let mut cores_in = vec![0u32; region_count];
    for &region in &region_of_core {
        cores_in[region as usize] += 1;
    }
    if cores_in.iter().all(|&n| n < 2) {
        return (solid.to_vec(), regions.to_vec(), region_count);
    }
    let mut piece_of_region = vec![u32::MAX; region_count];
    let mut count = 0u32;
    for (slot, &n) in piece_of_region.iter_mut().zip(&cores_in) {
        if n < 2 {
            *slot = count;
            count += 1;
        }
    }
    let several: Vec<Run> = solid
        .iter()
        .zip(regions)
        .filter(|(_, region)| cores_in[**region as usize] >= 2)
        .map(|(run, _)| *run)
        .collect();
    let (cut_runs, cut_labels, cut_count) = keep_small_pieces(cut_regions(&several), keep_px);
    let offset = count;
    count += u32::try_from(cut_count).expect("fewer pieces than pixels");

    let mut runs = Vec::with_capacity(solid.len() + cut_runs.len());
    let mut labels = Vec::with_capacity(solid.len() + cut_runs.len());
    let mut kept = solid
        .iter()
        .zip(regions)
        .filter(|(_, region)| cores_in[**region as usize] < 2)
        .peekable();
    let mut cut_iter = cut_runs.iter().zip(&cut_labels).peekable();
    loop {
        let take_kept = match (kept.peek(), cut_iter.peek()) {
            (Some((a, _)), Some((b, _))) => (a.row, a.x0) < (b.row, b.x0),
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => break,
        };
        if take_kept {
            let (run, region) = kept.next().expect("peeked");
            runs.push(*run);
            labels.push(piece_of_region[*region as usize]);
        } else {
            let (run, label) = cut_iter.next().expect("peeked");
            runs.push(*run);
            labels.push(offset + label);
        }
    }
    (runs, labels, count as usize)
}

/// Cuts `solid` into pieces, region by region: see the module documentation.
fn cut_regions(solid: &[Run]) -> (Vec<Run>, Vec<u32>, usize) {
    let cores = erode(solid);
    let (core_labels, core_count) = label_runs(&cores, Connectivity::Eight);
    // A core is an anchor when its part holds a 5 x 5 square of solid: a
    // pixel of the erosion of the cores.
    let mut anchor = vec![false; core_count];
    for_each_overlap(&cores, &erode(&cores), |i, _, _| {
        anchor[core_labels[i] as usize] = true;
    });
    let (grown, grown_labels) = dilate(&cores, &core_labels);
    let thin = subtract(solid, &grown);
    let (thin_labels, thin_count) = label_runs(&thin, Connectivity::Eight);

    // Parts: cores `0..core_count`, then thin parts. Parts that are not
    // anchors cluster with the parts of their kind they touch, and a cluster
    // joins the lowest anchor it touches.
    let parts = core_count + thin_count;
    let mut clusters = UnionFind::new(parts);
    let mut lowest_anchor = vec![u32::MAX; parts];
    let mut meet = |a: usize, b: usize, clusters: &mut UnionFind| match (
        a < core_count && anchor[a],
        b < core_count && anchor[b],
    ) {
        (false, false) => {
            clusters.union(a, b);
        }
        (false, true) => lowest_anchor[a] = lowest_anchor[a].min(b as u32),
        (true, false) => lowest_anchor[b] = lowest_anchor[b].min(a as u32),
        (true, true) => {}
    };
    for (run, &part) in grown.iter().zip(&grown_labels) {
        if anchor[part as usize] {
            continue;
        }
        for index in touching(&grown, run) {
            if grown_labels[index] != part {
                meet(part as usize, grown_labels[index] as usize, &mut clusters);
            }
        }
    }
    for (run, &part) in thin.iter().zip(&thin_labels) {
        let part = core_count + part as usize;
        for index in touching(&grown, run) {
            meet(part, grown_labels[index] as usize, &mut clusters);
        }
    }

    // Pieces: the anchors in core order, then the clusters that touch no
    // anchor, in the order of their first part.
    let mut piece_of_anchor = vec![u32::MAX; core_count];
    let mut count = 0u32;
    for (core, slot) in piece_of_anchor.iter_mut().enumerate() {
        if anchor[core] {
            *slot = count;
            count += 1;
        }
    }
    let mut cluster_anchor = vec![u32::MAX; parts];
    for (part, &lowest) in lowest_anchor.iter().enumerate() {
        let root = clusters.find(part);
        cluster_anchor[root] = cluster_anchor[root].min(lowest);
    }
    let mut piece_of_part = vec![u32::MAX; parts];
    let mut piece_of_root = vec![u32::MAX; parts];
    for (part, slot) in piece_of_part.iter_mut().enumerate() {
        *slot = if part < core_count && anchor[part] {
            piece_of_anchor[part]
        } else {
            let root = clusters.find(part);
            match cluster_anchor[root] {
                u32::MAX => {
                    if piece_of_root[root] == u32::MAX {
                        piece_of_root[root] = count;
                        count += 1;
                    }
                    piece_of_root[root]
                }
                core => piece_of_anchor[core as usize],
            }
        };
    }

    let mut runs = Vec::with_capacity(grown.len() + thin.len());
    let mut labels = Vec::with_capacity(grown.len() + thin.len());
    let (mut i, mut j) = (0, 0);
    while i < grown.len() || j < thin.len() {
        let take_grown = match (grown.get(i), thin.get(j)) {
            (Some(a), Some(b)) => (a.row, a.x0) < (b.row, b.x0),
            (Some(_), None) => true,
            _ => false,
        };
        if take_grown {
            runs.push(grown[i]);
            labels.push(piece_of_part[grown_labels[i] as usize]);
            i += 1;
        } else {
            runs.push(thin[j]);
            labels.push(piece_of_part[core_count + thin_labels[j] as usize]);
            j += 1;
        }
    }
    (runs, labels, count as usize)
}

/// The pieces `(runs, labels, count)` with every piece of no more than
/// `keep_px` pixels joined to the largest piece it touches (#404), on a tie
/// the first; a small piece that touches none stays as it is. Pieces are
/// numbered again in the order of their first piece.
fn keep_small_pieces(
    (runs, labels, count): (Vec<Run>, Vec<u32>, usize),
    keep_px: u64,
) -> (Vec<Run>, Vec<u32>, usize) {
    if keep_px == 0 || count < 2 {
        return (runs, labels, count);
    }
    let mut area = vec![0u64; count];
    for (run, &label) in runs.iter().zip(&labels) {
        area[label as usize] += u64::from(run.len());
    }
    if area.iter().all(|&a| a > keep_px) {
        return (runs, labels, count);
    }
    // The pieces each small piece touches.
    let mut neighbours: Vec<Vec<u32>> = vec![Vec::new(); count];
    for (run, &label) in runs.iter().zip(&labels) {
        if area[label as usize] > keep_px {
            continue;
        }
        for index in touching(&runs, run) {
            let other = labels[index];
            if other != label && !neighbours[label as usize].contains(&other) {
                neighbours[label as usize].push(other);
            }
        }
    }
    let mut sets = UnionFind::new(count);
    for (piece, touched) in neighbours.iter().enumerate() {
        let largest = touched
            .iter()
            .copied()
            .max_by(|a, b| area[*a as usize].cmp(&area[*b as usize]).then(b.cmp(a)));
        if let Some(other) = largest {
            sets.union(piece, other as usize);
        }
    }
    let mut renumber = vec![u32::MAX; count];
    let mut next = 0u32;
    let mut label_of = vec![0u32; count];
    for (piece, slot) in label_of.iter_mut().enumerate() {
        let root = sets.find(piece);
        if renumber[root] == u32::MAX {
            renumber[root] = next;
            next += 1;
        }
        *slot = renumber[root];
    }
    let labels = labels.iter().map(|&l| label_of[l as usize]).collect();
    (runs, labels, next as usize)
}

/// The pixels of `runs` (each with its label of `labels`) that `cover`
/// leaves out, each with the label of its run.
fn uncovered(runs: &[Run], labels: &[u32], cover: &[Run]) -> (Vec<Run>, Vec<u32>) {
    let mut out_runs = Vec::new();
    let mut out_labels = Vec::new();
    let mut j = 0;
    for (run, &label) in runs.iter().zip(labels) {
        while j < cover.len() && (cover[j].row, cover[j].x1) <= (run.row, run.x0) {
            j += 1;
        }
        let mut x = run.x0;
        let mut k = j;
        while k < cover.len() && cover[k].row == run.row && cover[k].x0 < run.x1 {
            if cover[k].x0 > x {
                out_runs.push(Run::new(run.row, x, cover[k].x0));
                out_labels.push(label);
            }
            x = x.max(cover[k].x1);
            k += 1;
        }
        if x < run.x1 {
            out_runs.push(Run::new(run.row, x, run.x1));
            out_labels.push(label);
        }
    }
    (out_runs, out_labels)
}

/// The rows of a run list: `(row, start, end)` with `runs[start..end]` on
/// `row`, in order.
fn rows_of(runs: &[Run]) -> Vec<(u32, usize, usize)> {
    let mut rows: Vec<(u32, usize, usize)> = Vec::new();
    for (i, run) in runs.iter().enumerate() {
        match rows.last_mut() {
            Some(last) if last.0 == run.row => last.2 = i + 1,
            _ => rows.push((run.row, i, i + 1)),
        }
    }
    rows
}

/// Appends to `out`, on `row`, the stretches two sorted, disjoint run lists
/// share. Their own rows are ignored.
fn intersect_row(a: &[Run], b: &[Run], row: u32, out: &mut Vec<Run>) {
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        let lo = a[i].x0.max(b[j].x0);
        let hi = a[i].x1.min(b[j].x1);
        if lo < hi {
            out.push(Run::new(row, lo, hi));
        }
        if a[i].x1 <= b[j].x1 {
            i += 1;
        } else {
            j += 1;
        }
    }
}

/// The pixels two run lists share.
pub(super) fn intersect(a: &[Run], b: &[Run]) -> Vec<Run> {
    let mut out = Vec::new();
    for_each_overlap(a, b, |i, j, _| {
        out.push(Run::new(
            a[i].row,
            a[i].x0.max(b[j].x0),
            a[i].x1.min(b[j].x1),
        ));
    });
    out
}

/// The erosion of `solid` by a 3 x 3 square: its pixels whose 8 neighbours
/// are all in it.
fn erode(solid: &[Run]) -> Vec<Run> {
    let shrunk: Vec<Run> = solid
        .iter()
        .filter(|run| run.len() >= 3)
        .map(|run| Run::new(run.row, run.x0 + 1, run.x1 - 1))
        .collect();
    let rows = rows_of(&shrunk);
    let mut out = Vec::new();
    let mut both = Vec::new();
    for window in rows.windows(3) {
        let [(above, a0, a1), (row, m0, m1), (below, b0, b1)] = [window[0], window[1], window[2]];
        if above + 1 != row || row + 1 != below {
            continue;
        }
        both.clear();
        intersect_row(&shrunk[a0..a1], &shrunk[m0..m1], row, &mut both);
        intersect_row(&both, &shrunk[b0..b1], row, &mut out);
    }
    out
}

/// Grows every labelled core of `cores` by its 3 x 3 neighbourhood. Returns
/// the grown runs, sorted by `(row, x0)` and disjoint, and the core of each;
/// a pixel two cores reach goes to the core whose stretch starts first on
/// the row (on a tie, the lower core).
fn dilate(cores: &[Run], labels: &[u32]) -> (Vec<Run>, Vec<u32>) {
    let rows = rows_of(cores);
    let mut targets: Vec<u32> = Vec::with_capacity(rows.len() + 2);
    for &(row, _, _) in &rows {
        for target in [row.saturating_sub(1), row, row + 1] {
            if targets.last().is_none_or(|&last| target > last) {
                targets.push(target);
            }
        }
    }
    let mut out_runs: Vec<Run> = Vec::new();
    let mut out_labels: Vec<u32> = Vec::new();
    let mut spans: Vec<(u32, u32, u32)> = Vec::new();
    let mut first = 0;
    for target in targets {
        while first < rows.len() && rows[first].0 + 1 < target {
            first += 1;
        }
        spans.clear();
        for &(row, start, end) in rows[first..].iter().take_while(|r| r.0 <= target + 1) {
            if row + 1 < target {
                continue;
            }
            for (run, &label) in cores[start..end].iter().zip(&labels[start..end]) {
                spans.push((run.x0.saturating_sub(1), run.x1 + 1, label));
            }
        }
        spans.sort_unstable_by_key(|&(x0, _, label)| (x0, label));
        let row_start = out_runs.len();
        for &(x0, x1, label) in &spans {
            let last = out_runs.len().checked_sub(1).filter(|&i| i >= row_start);
            match last {
                Some(i) if x0 <= out_runs[i].x1 => {
                    let end = out_runs[i].x1;
                    if out_labels[i] == label {
                        out_runs[i].x1 = end.max(x1);
                    } else if x1 > end {
                        out_runs.push(Run::new(target, end.max(x0), x1));
                        out_labels.push(label);
                    }
                }
                _ => {
                    out_runs.push(Run::new(target, x0, x1));
                    out_labels.push(label);
                }
            }
        }
    }
    (out_runs, out_labels)
}

/// The pixels of `a` that `b` leaves out, where `b` lies within `a`.
pub(super) fn subtract(a: &[Run], b: &[Run]) -> Vec<Run> {
    let mut out = Vec::new();
    let mut j = 0;
    for run in a {
        while j < b.len() && (b[j].row, b[j].x1) <= (run.row, run.x0) {
            j += 1;
        }
        let mut x = run.x0;
        let mut k = j;
        while k < b.len() && b[k].row == run.row && b[k].x0 < run.x1 {
            if b[k].x0 > x {
                out.push(Run::new(run.row, x, b[k].x0));
            }
            x = x.max(b[k].x1);
            k += 1;
        }
        if x < run.x1 {
            out.push(Run::new(run.row, x, run.x1));
        }
    }
    out
}

/// The indices of the runs of `list` (sorted by `(row, x0)`, disjoint within
/// a row) that share a pixel with `run` or touch it through an edge or a
/// corner.
fn touching<'a>(list: &'a [Run], run: &Run) -> impl Iterator<Item = usize> + 'a {
    let (x0, x1) = (run.x0, run.x1);
    (run.row.saturating_sub(1)..=run.row + 1).flat_map(move |row| {
        let start = list.partition_point(|r| (r.row, r.x1) < (row, x0));
        list[start..]
            .iter()
            .take_while(move |r| r.row == row && r.x0 <= x1)
            .enumerate()
            .map(move |(i, _)| start + i)
    })
}

/// How the pieces of `upper` rest on those of the layers `lowers` below it
/// (the layer right below first): on the layer right below through the
/// pixels they share, and on a deeper layer through the pixels with nothing
/// in the layers between (#403). Sorted by `(below, lower, upper)`.
fn contacts(lowers: &[&LayerPieces], upper: &LayerPieces) -> Vec<Contact> {
    let _clock = crate::clock::scope("pieces.contacts");
    let mut out: Vec<Contact> = Vec::new();
    let mut open: Option<(Vec<Run>, Vec<u32>)> = None;
    for (depth, lower) in lowers.iter().enumerate() {
        let below = u8::try_from(depth + 1).expect("a few layers");
        let (runs, labels) = match &open {
            None => (upper.runs.as_slice(), upper.labels.as_slice()),
            Some((runs, labels)) => (runs.as_slice(), labels.as_slice()),
        };
        if runs.is_empty() {
            break;
        }
        let (split, tags) = lower.split.get_or_init(|| split_by_role(lower));
        for_each_overlap(split, runs, |i, j, px| {
            let (piece, on_model) = tags[i];
            let px = u64::from(px);
            out.push(Contact {
                below,
                lower: piece,
                upper: labels[j],
                model_px: if on_model { px } else { 0 },
                support_px: if on_model { 0 } else { px },
            });
        });
        if depth + 1 < lowers.len() {
            open = Some(uncovered(runs, labels, &lower.runs));
        }
    }
    out.sort_unstable_by_key(|c| (c.below, c.lower, c.upper));
    out.dedup_by(|next, kept| {
        if (next.below, next.lower, next.upper) == (kept.below, kept.lower, kept.upper) {
            kept.model_px += next.model_px;
            kept.support_px += next.support_px;
            true
        } else {
            false
        }
    });
    out
}

/// The runs of `layer` split where model starts and stops, each tagged with
/// its piece and whether it is model.
fn split_by_role(layer: &LayerPieces) -> RoleSplit {
    let model = &layer.model;
    let mut runs = Vec::with_capacity(layer.runs.len());
    let mut tags = Vec::with_capacity(layer.runs.len());
    let mut m = 0;
    for (run, &label) in layer.runs.iter().zip(&layer.labels) {
        while m < model.len() && (model[m].row, model[m].x1) <= (run.row, run.x0) {
            m += 1;
        }
        let mut x = run.x0;
        let mut k = m;
        while k < model.len() && model[k].row == run.row && model[k].x0 < run.x1 {
            let lo = model[k].x0.max(run.x0);
            let hi = model[k].x1.min(run.x1);
            if x < lo {
                runs.push(Run::new(run.row, x, lo));
                tags.push((label, false));
            }
            runs.push(Run::new(run.row, lo, hi));
            tags.push((label, true));
            x = hi;
            k += 1;
        }
        if x < run.x1 {
            runs.push(Run::new(run.row, x, run.x1));
            tags.push((label, false));
        }
    }
    (runs, tags)
}

/// For every piece of `upper` that rests on nothing, the pieces of `lower`
/// it touches through an edge or a corner, as sorted `(upper, lower)` pairs.
fn touches(lower: &LayerPieces, upper: &LayerPieces, contacts: &[Contact]) -> Vec<(u32, u32)> {
    let _clock = crate::clock::scope("pieces.touches");
    let mut rests = vec![false; upper.pieces.len()];
    for contact in contacts {
        rests[contact.upper as usize] = true;
    }
    let mut out = Vec::new();
    for (run, &label) in upper.runs.iter().zip(&upper.labels) {
        if rests[label as usize] {
            continue;
        }
        out.extend(touching(&lower.runs, run).map(|index| (label, lower.labels[index])));
    }
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
#[path = "tests_island_pieces.rs"]
mod tests;
