/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Islands (#84): parts of the plate that start in mid-air, or rest on too
//! little cured resin to hold them at the next peel.
//!
//! Reports [`CheckKind::Island`] findings.
//!
//! # Pieces
//!
//! Each layer's solid (model and support merged) is cut at its weak joints,
//! where it is narrower than [`MIN_JOINT_PX`] pixels between parts at least
//! [`MIN_ANCHOR_PX`] pixels thick: two parts that meet only through a corner
//! pixel or a one-pixel neck hold each other no better than two parts apart,
//! so each is judged on what is below it. Features thinner than that are not
//! cut. The parts are the layer's *pieces* (see the `pieces` module).
//!
//! # What holds a piece
//!
//! A piece on layer `k` rests on its pixels that have solid straight below,
//! in layer `k - 1`; pixels link across layers only straight up. The first
//! [`PLATE_LAYERS`] layers rest on the build plate. A piece is *held* when
//! its contact covers a margin of its area:
//!
//! - cured model below must cover [`MIN_SUPPORTED_FRACTION`] (25 %): peel
//!   force grows with the area of the new layer, and what resists it is the
//!   cured area below;
//! - support below a piece that has model is a support tip, placed to hold
//!   that model and sunk into it, and must cover
//!   [`MIN_TIP_SUPPORTED_FRACTION`] (5 %). Support below a support-only piece
//!   is the support's own structure and must cover the full 25 %.
//!
//! A mix is held when each pixel of contact covers its share:
//! `model_px / 25 % + tip_px / 5 % >= area_px`. At exactly the margin a piece
//! is held.
//!
//! # Sound pieces and islands
//!
//! From the plate up, a piece is *sound* when its contact with sound pieces
//! below holds it. Every other piece belongs to an island:
//!
//! - a piece that even everything below it does not hold is *weak*. A weak
//!   piece that rests on sound solid, or on nothing at all, *starts* an
//!   island, and every start is a finding with its own suggested support:
//!   two tips that merge higher up are two findings;
//! - any other piece that rests on island pieces continues their island,
//!   held or not: the growing layers of a cone tip, or a part that grows on
//!   an island until something sound holds it. A piece that rests on nothing
//!   but touches island pieces of the layer below through an edge or a corner
//!   continues them too, so a thin strand sliding sideways is one island.
//!
//! Island pieces that rest on one another are joined with union-find, so the
//! finding of a start spans the whole part that hangs from it, up to the last
//! layer before something sound holds it.
//!
//! # The finding
//!
//! - **Numbers:** the start's area and what it rests on and, when the part
//!   that hangs from it grows larger, the area that part reaches and whether
//!   something sound ends up holding it.
//! - **Severity:** an [`Severity::Error`] when the start has less than
//!   [`ERROR_SHARE_OF_MARGIN`] (a fifth) of the hold its margin asks for:
//!   nothing below, a model slab on under 5 % of cured resin, a model on
//!   support tips covering under 1 %. A [`Severity::Warning`] otherwise.
//! - **Role:** a start with model pixels is a model island ("Unsupported
//!   island", "Weakly supported island") with a [`SuggestedFix::AddSupport`].
//!   A start with support pixels only is a support island ("Floating
//!   support", "Weakly held support") and suggests nothing: a support's tip
//!   goes on the model, and a floating support is mended by regenerating or
//!   editing the supports.
//! - **Focus and fix:** the pixel of the start that nothing holds, farthest
//!   from its edge and from what holds it, on the model for a model island
//!   (see the `focus` module): never on a post or a tip already there.
//!
//! # Noise
//!
//! A start smaller than [`MIN_ISLAND_AREA_PX`] pixels is not reported on its
//! own: rasterising a surface that grazes a layer's cutting height can light
//! a stray pixel or two. When every start of an island is that small but
//! the island grows to the minimum, its largest start is reported, so a real
//! tip that starts with a single pixel is found from that pixel.
//!
//! # Cost
//!
//! The check needs every layer cut into pieces, which the shared layer
//! regions do not keep, so it rasterises the layers again, in parallel, and
//! cuts in full only the regions with several cores. On a 1.9 M-triangle
//! miniature (913 layers of 11520 x 5120 pixels, 16 threads) it takes about
//! 0.33 s, against 0.22 s for the shared regions.

use rayon::prelude::*;

use crate::finding::{CheckKind, Finding, RoleScope, Severity, SuggestedFix};
use crate::runner::ReviewContext;
use crate::runs::Run;
use crate::union_find::{Groups, RegionRef, UnionFind};

#[path = "island_focus.rs"]
mod focus;
#[path = "island_pieces.rs"]
mod pieces;

use pieces::{Contact, LayerPieces, Piece, PieceScan};
pub use pieces::{MIN_ANCHOR_PX, MIN_JOINT_PX};

/// The fraction of a piece's area that cured model below it must cover for
/// the piece to count as held: 25 %. It also applies to a support-only piece
/// resting on support.
///
/// Peel force grows with the area of the new layer, and what resists it is
/// the cured area below, so the ratio of the two is the margin. A quarter is
/// the usual starting point for resin island detection. It also matches the
/// commonest real island, a cone tip pointing down: its area grows with the
/// square of its height, so its second layer rests on at most a quarter of
/// its area and its third on more than a quarter.
pub const MIN_SUPPORTED_FRACTION: f64 = 0.25;

/// The fraction of a model piece's area that support below it must cover for
/// the piece to count as held: 5 %.
///
/// That support is a tip, placed to hold the model around it and sunk into
/// it, so a small contact is the design, not a defect: one 0.4 mm tip
/// (0.126 mm²) holds up to 2.5 mm² of surface. What a tip really holds
/// against the peel is what the tip-strength model of #11 will compute;
/// until then the margin is this area ratio, a fifth of the model margin.
/// On a 1.9 M-triangle miniature with generated supports (Saturn 3 Ultra,
/// 0.1 mm layers), wherever a model piece needs its tips (the model below it
/// would not hold it alone), they cover 10 % of it or more: twice this
/// margin, so it leaves them alone and flags a piece on a fraction of a
/// tip.
pub const MIN_TIP_SUPPORTED_FRACTION: f64 = 0.05;

/// The share of its margin under which an island is an error rather than a
/// warning: a fifth. A start with nothing below it, a model slab on less than
/// 5 % of cured resin or a model on tips covering less than 1 % has next to
/// nothing holding it; above that, it is held, but by less than the margin.
pub const ERROR_SHARE_OF_MARGIN: f64 = 0.2;

/// The layers that rest on the build plate: 2. Nothing on them is an island.
///
/// Layer 0 lies on the plate. A pixel of layer 1 with nothing below it hangs
/// one layer above the plate, and the long exposure of the first layers cures
/// through that gap, so it bonds to the plate as well; no support would fit
/// there either.
pub const PLATE_LAYERS: usize = 2;

/// The smallest island start reported on its own, in pixels: 4, a 2 x 2
/// pixel square (38 x 48 µm, 0.0018 mm² on a 19 x 24 µm screen).
///
/// Measured on a 1.9 M-triangle miniature with generated supports (Saturn 3
/// Ultra, 0.1 mm layers): with no minimum at all, the only starts under 67
/// pixels were support specks of 1 and 2 pixels, and the smallest model
/// start had 397 pixels, so the minimum costs no real finding there. By geometry, a cone tip's second layer
/// stays under 4 pixels on that screen only when the cone is sharper than 7
/// to 13° from its axis (depending on where the layers cut it): a needle
/// whose first 0.2 mm the printer does not reproduce anyway.
pub const MIN_ISLAND_AREA_PX: u64 = 4;

/// Slack on the margin comparisons, so a piece at exactly its margin counts
/// as held whatever the rounding of the division.
const TOLERANCE: f64 = 1e-9;

/// Settings of the island check.
#[derive(Clone, Debug, PartialEq)]
pub struct IslandOptions {
    /// The fraction of a piece's area, from 0 to 1, that cured model below it
    /// must cover (support below a support-only piece counts as model here).
    /// Default [`MIN_SUPPORTED_FRACTION`]; 0 turns the check off.
    pub min_supported_fraction: f64,
    /// The fraction of a model piece's area, from 0 to 1, that support tips
    /// below it must cover. Default [`MIN_TIP_SUPPORTED_FRACTION`]; 0 makes
    /// any tip contact enough.
    pub min_tip_supported_fraction: f64,
    /// The smallest island start reported on its own, in pixels. Default
    /// [`MIN_ISLAND_AREA_PX`].
    pub min_area_px: u64,
}

impl Default for IslandOptions {
    fn default() -> Self {
        Self {
            min_supported_fraction: MIN_SUPPORTED_FRACTION,
            min_tip_supported_fraction: MIN_TIP_SUPPORTED_FRACTION,
            min_area_px: MIN_ISLAND_AREA_PX,
        }
    }
}

impl IslandOptions {
    /// These options with every fraction usable: clamped to 0..=1, and its
    /// default when it is not a number. [`run`] applies it.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        let fraction = |value: f64, default: f64| {
            if value.is_nan() {
                default
            } else {
                value.clamp(0.0, 1.0)
            }
        };
        Self {
            min_supported_fraction: fraction(self.min_supported_fraction, MIN_SUPPORTED_FRACTION),
            min_tip_supported_fraction: fraction(
                self.min_tip_supported_fraction,
                MIN_TIP_SUPPORTED_FRACTION,
            ),
            min_area_px: self.min_area_px,
        }
    }

    /// How much of the hold its margin asks for a piece of `area_px` pixels
    /// gets from `rest`: 1 at exactly the margin, more when held, 0 with
    /// nothing below. `tips` says whether support below counts as tips (the
    /// piece has model).
    fn hold(&self, rest: Rest, area_px: u64, tips: bool) -> f64 {
        let share = |px: u64, margin: f64| {
            if px == 0 {
                0.0
            } else if margin > 0.0 {
                px as f64 / margin
            } else {
                f64::INFINITY
            }
        };
        let support_margin = if tips {
            self.min_tip_supported_fraction
        } else {
            self.min_supported_fraction
        };
        (share(rest.model_px, self.min_supported_fraction) + share(rest.support_px, support_margin))
            / area_px.max(1) as f64
    }
}

/// What a piece rests on, in pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Rest {
    /// Pixels on model below.
    model_px: u64,
    /// Pixels on support (and not model) below.
    support_px: u64,
}

impl Rest {
    fn add(&mut self, contact: &Contact) {
        self.model_px += contact.model_px;
        self.support_px += contact.support_px;
    }

    const fn total(&self) -> u64 {
        self.model_px + self.support_px
    }
}

/// A piece that starts an island.
#[derive(Clone, Copy, Debug)]
struct Start {
    /// The piece.
    at: RegionRef,
    /// What it rests on.
    rest: Rest,
    /// Its hold, under 1 (see [`IslandOptions::hold`]).
    hold: f64,
}

/// The island pieces of a plate.
struct Islands {
    /// First global id of each layer's pieces, plus the total at the end.
    offsets: Vec<usize>,
    /// Whether each piece (by global id) belongs to an island.
    island: Vec<bool>,
    /// Island pieces joined by resting on one another.
    sets: UnionFind,
    /// The pieces that start islands, lowest layer first.
    starts: Vec<Start>,
    /// Island pieces that a sound piece rests on.
    held_from_above: Vec<usize>,
}

/// Finds the islands of the plate.
pub fn run(ctx: &ReviewContext<'_>, options: &IslandOptions) -> Vec<Finding> {
    let options = options.sanitized();
    if options.min_supported_fraction == 0.0 || ctx.input.layer_count() <= PLATE_LAYERS {
        return Vec::new();
    }
    let scan = pieces::scan(ctx.input, PLATE_LAYERS);
    let islands = classify(&scan, &options);
    if islands.starts.is_empty() {
        return Vec::new();
    }
    report(ctx, &scan, islands, &options)
}

/// Sorts every piece into sound or island, bottom up, and finds the starts.
fn classify(scan: &PieceScan, options: &IslandOptions) -> Islands {
    let mut offsets = Vec::with_capacity(scan.layers.len() + 1);
    let mut total = 0;
    offsets.push(0);
    for layer in &scan.layers {
        total += layer.len();
        offsets.push(total);
    }
    let mut islands = Islands {
        offsets,
        island: vec![false; total],
        sets: UnionFind::new(total),
        starts: Vec::new(),
        held_from_above: Vec::new(),
    };
    for layer in PLATE_LAYERS..scan.layers.len() {
        classify_layer(scan, options, layer, &mut islands);
    }
    islands
}

fn classify_layer(scan: &PieceScan, options: &IslandOptions, layer: usize, islands: &mut Islands) {
    let pieces = &scan.layers[layer];
    let contacts = &scan.contacts[layer];
    let below = islands.offsets[layer - 1];
    let base = islands.offsets[layer];
    let mut all = vec![Rest::default(); pieces.len()];
    let mut on_sound = vec![Rest::default(); pieces.len()];
    for contact in contacts {
        all[contact.upper as usize].add(contact);
        if !islands.island[below + contact.lower as usize] {
            on_sound[contact.upper as usize].add(contact);
        }
    }
    for (index, piece) in pieces.iter().enumerate() {
        let tips = piece.model_px > 0;
        if options.hold(on_sound[index], piece.area_px, tips) >= 1.0 - TOLERANCE {
            continue;
        }
        islands.island[base + index] = true;
        let hold = options.hold(all[index], piece.area_px, tips);
        if hold >= 1.0 - TOLERANCE {
            continue;
        }
        let starts = if on_sound[index].total() > 0 {
            true
        } else if all[index].total() > 0 {
            false
        } else {
            // Resting on nothing, it continues the pieces of the layer below
            // that it touches through an edge or a corner when they are all
            // island pieces; it starts an island when it touches none, or
            // touches sound solid.
            let mut touched = touched(&scan.touches[layer], index as u32).peekable();
            touched.peek().is_none() || touched.any(|lower| !islands.island[below + lower as usize])
        };
        if starts {
            islands.starts.push(Start {
                at: RegionRef {
                    layer,
                    index: index as u32,
                },
                rest: all[index],
                hold,
            });
        }
    }
    for contact in contacts {
        let lower = below + contact.lower as usize;
        let upper = base + contact.upper as usize;
        match (islands.island[lower], islands.island[upper]) {
            (true, true) => {
                islands.sets.union(lower, upper);
            }
            (true, false) => islands.held_from_above.push(lower),
            _ => {}
        }
    }
    for &(upper, lower) in &scan.touches[layer] {
        let (lower, upper) = (below + lower as usize, base + upper as usize);
        if islands.island[lower] && islands.island[upper] {
            islands.sets.union(lower, upper);
        }
    }
}

/// The pieces of the layer below that piece `upper` touches, from `touches`
/// (sorted `(upper, lower)` pairs).
fn touched(touches: &[(u32, u32)], upper: u32) -> impl Iterator<Item = u32> + '_ {
    let start = touches.partition_point(|&(u, _)| u < upper);
    touches[start..]
        .iter()
        .take_while(move |&&(u, _)| u == upper)
        .map(|&(_, lower)| lower)
}

/// One start to report, with what the finding needs from its island.
struct Reported {
    start: Start,
    /// The last layer of its island.
    last_layer: usize,
    /// The largest piece of its island from its layer up, in pixels.
    largest_px: u64,
    /// Whether something sound rests on its island.
    held_from_above: bool,
}

/// The findings for the starts worth reporting.
fn report(
    ctx: &ReviewContext<'_>,
    scan: &PieceScan,
    mut islands: Islands,
    options: &IslandOptions,
) -> Vec<Finding> {
    let piece = |at: RegionRef| scan.layers[at.layer][at.index as usize];
    let groups = Groups::from_union_find(islands.offsets.clone(), &mut islands.sets);
    let ref_of = |id: usize| {
        let layer = islands.offsets.partition_point(|&offset| offset <= id) - 1;
        RegionRef {
            layer,
            index: (id - islands.offsets[layer]) as u32,
        }
    };
    let mut held_from_above = vec![false; groups.len()];
    for &id in &islands.held_from_above {
        held_from_above[groups.group_of(ref_of(id))] = true;
    }
    let mut starts_of: Vec<Vec<Start>> = vec![Vec::new(); groups.len()];
    for start in &islands.starts {
        starts_of[groups.group_of(start.at)].push(*start);
    }
    let mut reported = Vec::new();
    for (group, starts) in starts_of.iter().enumerate() {
        if starts.is_empty() {
            continue;
        }
        let members = groups.members(group);
        let mut chosen: Vec<Start> = starts
            .iter()
            .filter(|start| piece(start.at).area_px >= options.min_area_px)
            .copied()
            .collect();
        let largest = members.iter().map(|&m| piece(m).area_px).max().unwrap_or(0);
        if chosen.is_empty() && largest >= options.min_area_px {
            // Every start is a speck, yet the island grows: report the
            // largest start, the lowest on a tie.
            chosen.extend(
                starts
                    .iter()
                    .max_by(|a, b| {
                        piece(a.at)
                            .area_px
                            .cmp(&piece(b.at).area_px)
                            .then(b.at.cmp(&a.at))
                    })
                    .copied(),
            );
        }
        let last_layer = members[members.len() - 1].layer;
        for start in chosen {
            let largest_px = members
                .iter()
                .filter(|m| m.layer >= start.at.layer)
                .map(|&m| piece(m).area_px)
                .max()
                .unwrap_or(0);
            reported.push(Reported {
                start,
                last_layer,
                largest_px,
                held_from_above: held_from_above[group],
            });
        }
    }
    let mut layers: Vec<usize> = reported.iter().map(|r| r.start.at.layer).collect();
    layers.sort_unstable();
    layers.dedup();
    layers
        .par_iter()
        .flat_map_iter(|&layer| {
            let cut = LayerPieces::compute(ctx.input, layer);
            let below = LayerPieces::compute(ctx.input, layer - 1);
            reported
                .iter()
                .filter(move |r| r.start.at.layer == layer)
                .map(|r| island_finding(ctx, r, piece(r.start.at), &cut, &below.runs))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The finding for one reported start, whose layer is cut into `cut` and
/// whose layer below has the solid `below`.
fn island_finding(
    ctx: &ReviewContext<'_>,
    reported: &Reported,
    piece: Piece,
    cut: &LayerPieces,
    below: &[Run],
) -> Finding {
    let start = &reported.start;
    let is_model = piece.model_px > 0;
    let runs = cut.runs_of(start.at.index);
    let prefer = is_model.then_some(cut.model.as_slice());
    let (pixel, on_model) =
        focus::deepest_unheld(&runs, below, prefer).unwrap_or(([runs[0].x0, runs[0].row], false));
    let focus = ctx.input.focus(
        start.at.layer,
        [f64::from(pixel[0]) + 0.5, f64::from(pixel[1]) + 0.5],
    );
    let severity = if start.hold < ERROR_SHARE_OF_MARGIN - TOLERANCE {
        Severity::Error
    } else {
        Severity::Warning
    };
    let title = match (is_model, start.rest.total() == 0) {
        (true, true) => "Unsupported island",
        (true, false) => "Weakly supported island",
        (false, true) => "Floating support",
        (false, false) => "Weakly held support",
    };
    let scope = if is_model {
        RoleScope::Model
    } else {
        RoleScope::Support
    };
    let detail = detail(reported, piece, ctx.input.pixel_area_mm2());
    let finding = Finding::new(
        CheckKind::Island,
        severity,
        start.at.layer,
        reported.last_layer,
        focus,
        scope,
        title,
        detail,
    );
    if is_model && on_model {
        finding.with_fix(SuggestedFix::AddSupport {
            at: ctx.input.underside(start.at.layer, focus),
        })
    } else {
        finding
    }
}

/// The finding's sentence: the start's area and what it rests on, then how
/// large the part hanging from it grows.
fn detail(reported: &Reported, piece: Piece, pixel_mm2: f64) -> String {
    let start = &reported.start;
    let is_model = piece.model_px > 0;
    let what = if is_model { "model" } else { "support" };
    let area = significant(piece.area_px as f64 * pixel_mm2);
    let below = start.rest.total();
    let mut text = if below == 0 {
        format!("{area} mm² of {what} starts with nothing cured below it (0 % supported)")
    } else {
        // The margin this mix of contact asks for: the fraction over the
        // share of its hold it gets.
        let margin = below as f64 / piece.area_px as f64 / start.hold;
        let (supported, margin) = percents(below, piece.area_px, margin);
        let (kind, whose) = match (is_model, start.rest.model_px, start.rest.support_px) {
            (true, 0, _) => ("support tips", " for tips"),
            (true, _, tips) if tips > 0 => ("cured resin and support tips", " for that mix"),
            _ => ("cured resin", ""),
        };
        format!(
            "{area} mm² of {what} rests on only {} mm² of {kind} ({supported} supported, under the {margin} margin{whose})",
            significant(below as f64 * pixel_mm2),
        )
    };
    let largest = significant(reported.largest_px as f64 * pixel_mm2);
    if reported.largest_px > piece.area_px && largest != area {
        text.push_str(&format!(", and the part it starts grows to {largest} mm²"));
        text.push_str(if reported.held_from_above {
            " before the rest of the print holds it"
        } else {
            " with nothing else holding it"
        });
    }
    text.push('.');
    text
}

/// `value` with three significant digits and no exponent: `0.00456`,
/// `0.123`, `1.23`, `12.3`, `123`; larger values with no decimals.
fn significant(value: f64) -> String {
    if value <= 0.0 || !value.is_finite() {
        return "0".to_string();
    }
    let mut magnitude = value.log10().floor() as i32;
    // Rounding to three digits can carry into the next power of ten
    // (9.9996 to 10.00): count the digits of the rounded value.
    let decimals = |magnitude: i32| (2 - magnitude).max(0);
    let rounded = |magnitude: i32| {
        let scale = 10f64.powi(decimals(magnitude));
        (value * scale).round() / scale
    };
    if rounded(magnitude) >= 10f64.powi(magnitude + 1) {
        magnitude += 1;
    }
    let decimals = decimals(magnitude) as usize;
    format!("{value:.decimals$}")
}

/// The supported fraction `part / whole` and the `margin` it falls under, as
/// percentages that read in that order: the fraction rounded down, the
/// margin to the nearest, both with the fewest decimals (up to 3) that show
/// the margin and keep the fraction below it. A non-zero fraction under 1 %
/// of a whole-percent margin reads `"<1 %"`.
fn percents(part: u64, whole: u64, margin: f64) -> (String, String) {
    let exact = |decimals: u32| {
        let scaled = margin * 100.0 * 10f64.powi(decimals as i32);
        (scaled - scaled.round()).abs() < 1e-6
    };
    let mut decimals = (0..2).find(|&d| exact(d)).unwrap_or(2);
    loop {
        let scale = 100 * 10u128.pow(decimals);
        let shown_fraction = u128::from(part) * scale / u128::from(whole.max(1));
        let shown_margin = (margin * scale as f64).round();
        if (shown_fraction as f64) < shown_margin || decimals == 3 {
            let d = decimals as usize;
            let divisor = 10f64.powi(decimals as i32);
            let supported = if shown_fraction == 0 && part > 0 && decimals == 0 {
                "<1 %".to_string()
            } else {
                format!("{:.d$} %", shown_fraction as f64 / divisor)
            };
            return (supported, format!("{:.d$} %", shown_margin / divisor));
        }
        decimals += 1;
    }
}

#[cfg(test)]
#[path = "tests_islands.rs"]
mod tests;
