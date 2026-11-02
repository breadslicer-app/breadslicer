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
//! # Which islands want a support (#163)
//!
//! The thresholds live in [`island_policy`]; every one is on by default.
//!
//! - **Cured through (#403):** a pixel with nothing straight below it rests
//!   on solid as many layers further down as the exposure cures through
//!   ([`island_policy::CURE_DEPTH_MM`]): one layer of liquid at 0.05 mm.
//! - **Weak joints (#404):** a piece no larger than a tip's contact is not
//!   cut off at a weak joint (see the `pieces` module).
//! - **Minimal islands (#402):** an island that something sound holds within
//!   [`island_policy::MINIMAL_ISLAND_HEIGHT_MM`] of its first layer, its
//!   layers included, whose largest section is no larger than a tip's
//!   contact and that gathers less than
//!   [`island_policy::MINIMAL_ISLAND_VOLUME_MM3`], is a note with no fix:
//!   the next layers lift it, and a support there would do more harm than
//!   good.
//! - **Join to below (#235):** a model island with model under it across a
//!   gap no thicker than [`island_policy::JOIN_GAP_MM`], enough of it to
//!   hold the island, suggests [`SuggestedFix::JoinBelow`]: curing the gap
//!   on the next slice. No support fits in such a gap.
//! - **Aim higher (#405):** for a growing model island whose first layer is
//!   smaller than a tip's contact, the suggested support goes where the
//!   island first reaches that size, at most
//!   [`island_policy::SUPPORT_CLIMB_MM`] higher, on the part that grew.
//!
//! # Cost
//!
//! The check needs every layer cut into pieces, which the shared layer
//! regions do not keep, so it rasterises the layers again, in parallel, and
//! cuts in full only the regions with several cores. On a 1.9 M-triangle
//! miniature (913 layers of 11520 x 5120 pixels, 16 threads) it takes about
//! 0.33 s, against 0.22 s for the shared regions.

use rayon::prelude::*;

use crate::fill::CavityFill;
use crate::finding::{CheckKind, Finding, RoleScope, Severity, SuggestedFix};
use crate::regions::LayerPixels;
use crate::runner::ReviewContext;
use crate::runs::{self, Run};
use crate::union_find::{Groups, RegionRef, UnionFind};

#[path = "island_focus.rs"]
mod focus;
#[path = "island_policy.rs"]
pub mod island_policy;
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
    /// How deep a layer's exposure cures, its own thickness included, in mm:
    /// solid that deep below a pixel holds it (#403). Default
    /// [`island_policy::CURE_DEPTH_MM`]; 0 counts only the layer right
    /// below.
    pub cure_depth_mm: f64,
    /// The largest piece kept with what it touches across a weak joint, in
    /// mm² (#404). Default [`island_policy::WEAK_JOINT_KEEP_MM2`]; 0 cuts
    /// every weak joint.
    pub weak_joint_keep_mm2: f64,
    /// The highest a minimal island grows, its layers included, in mm
    /// (#402). Default [`island_policy::MINIMAL_ISLAND_HEIGHT_MM`]; 0 makes
    /// no island minimal.
    pub minimal_height_mm: f64,
    /// The resin a minimal island gathers, in mm³: it must gather less.
    /// Default [`island_policy::MINIMAL_ISLAND_VOLUME_MM3`].
    pub minimal_volume_mm3: f64,
    /// The largest section of a minimal island, in mm². Default
    /// [`island_policy::MINIMAL_ISLAND_SECTION_MM2`].
    pub minimal_section_mm2: f64,
    /// The thickest gap over model that "Join to below" closes, in mm
    /// (#235). Default [`island_policy::JOIN_GAP_MM`]; 0 offers none.
    pub join_gap_mm: f64,
    /// How much higher a growing island's support may aim, in mm (#405).
    /// Default [`island_policy::SUPPORT_CLIMB_MM`]; 0 keeps it at the first
    /// layer.
    pub support_climb_mm: f64,
    /// A tip's contact, in mm²: the section from which an island holds a
    /// support (#405). Default [`island_policy::TIP_CONTACT_MM2`].
    pub tip_contact_mm2: f64,
}

impl Default for IslandOptions {
    fn default() -> Self {
        Self {
            min_supported_fraction: MIN_SUPPORTED_FRACTION,
            min_tip_supported_fraction: MIN_TIP_SUPPORTED_FRACTION,
            min_area_px: MIN_ISLAND_AREA_PX,
            cure_depth_mm: island_policy::CURE_DEPTH_MM,
            weak_joint_keep_mm2: island_policy::WEAK_JOINT_KEEP_MM2,
            minimal_height_mm: island_policy::MINIMAL_ISLAND_HEIGHT_MM,
            minimal_volume_mm3: island_policy::MINIMAL_ISLAND_VOLUME_MM3,
            minimal_section_mm2: island_policy::MINIMAL_ISLAND_SECTION_MM2,
            join_gap_mm: island_policy::JOIN_GAP_MM,
            support_climb_mm: island_policy::SUPPORT_CLIMB_MM,
            tip_contact_mm2: island_policy::TIP_CONTACT_MM2,
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
        // A length, an area or a volume: 0 or more, its default when it is
        // not a number.
        let size = |value: f64, default: f64| {
            if value.is_nan() {
                default
            } else {
                value.max(0.0)
            }
        };
        Self {
            min_supported_fraction: fraction(self.min_supported_fraction, MIN_SUPPORTED_FRACTION),
            min_tip_supported_fraction: fraction(
                self.min_tip_supported_fraction,
                MIN_TIP_SUPPORTED_FRACTION,
            ),
            min_area_px: self.min_area_px,
            cure_depth_mm: size(self.cure_depth_mm, island_policy::CURE_DEPTH_MM),
            weak_joint_keep_mm2: size(self.weak_joint_keep_mm2, island_policy::WEAK_JOINT_KEEP_MM2),
            minimal_height_mm: size(
                self.minimal_height_mm,
                island_policy::MINIMAL_ISLAND_HEIGHT_MM,
            ),
            minimal_volume_mm3: size(
                self.minimal_volume_mm3,
                island_policy::MINIMAL_ISLAND_VOLUME_MM3,
            ),
            minimal_section_mm2: size(
                self.minimal_section_mm2,
                island_policy::MINIMAL_ISLAND_SECTION_MM2,
            ),
            join_gap_mm: size(self.join_gap_mm, island_policy::JOIN_GAP_MM),
            support_climb_mm: size(self.support_climb_mm, island_policy::SUPPORT_CLIMB_MM),
            tip_contact_mm2: size(self.tip_contact_mm2, island_policy::TIP_CONTACT_MM2),
        }
    }

    /// These options with the island policy of #163 off: every pixel rests
    /// only on the layer right below, every weak joint is cut, no island is
    /// minimal, none is joined below, and supports aim at the first layer.
    #[must_use]
    pub fn without_policy(&self) -> Self {
        Self {
            cure_depth_mm: 0.0,
            weak_joint_keep_mm2: 0.0,
            minimal_height_mm: 0.0,
            join_gap_mm: 0.0,
            support_climb_mm: 0.0,
            ..self.clone()
        }
    }

    /// The piece size kept across a weak joint, in pixels of `input`.
    fn keep_px(&self, input: &crate::input::ReviewInput<'_>) -> u64 {
        (self.weak_joint_keep_mm2 / input.pixel_area_mm2() + 1e-9).floor() as u64
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
    run_with_joins(ctx, options).0
}

/// [`run`], with the pixels that join each island to the model below it:
/// the cavity fills its [`SuggestedFix::JoinBelow`] fixes name, by index
/// (#235).
pub fn run_with_joins(
    ctx: &ReviewContext<'_>,
    options: &IslandOptions,
) -> (Vec<Finding>, Vec<CavityFill>) {
    let options = options.sanitized();
    if options.min_supported_fraction == 0.0 || ctx.input.layer_count() <= PLATE_LAYERS {
        return (Vec::new(), Vec::new());
    }
    let scan = pieces::scan(
        ctx.input,
        PLATE_LAYERS,
        options.keep_px(ctx.input),
        options.cure_depth_mm,
    );
    let islands = classify(&scan, &options);
    if islands.starts.is_empty() {
        return (Vec::new(), Vec::new());
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
    // A contact's lower piece by global id, on whichever layer it lies.
    let lower_id = |contact: &Contact| {
        islands.offsets[layer - contact.below as usize] + contact.lower as usize
    };
    let mut all = vec![Rest::default(); pieces.len()];
    let mut on_sound = vec![Rest::default(); pieces.len()];
    for contact in contacts {
        all[contact.upper as usize].add(contact);
        if !islands.island[lower_id(contact)] {
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
        let lower = islands.offsets[layer - contact.below as usize] + contact.lower as usize;
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
    /// Its island, when minimal (#402).
    minimal: Option<IslandSize>,
    /// Its island's pieces from its layer up to where a support may aim
    /// (#405), with the island's section on each of those layers in pixels.
    climb: Vec<(RegionRef, u64)>,
}

/// How large an island grows before something sound holds it.
#[derive(Clone, Copy, Debug)]
struct IslandSize {
    /// Its layers, its own first and last included.
    layers: usize,
    /// From the bottom of its first layer to the top of its last, in mm.
    height_mm: f64,
    /// The resin it gathers, in mm³.
    volume_mm3: f64,
    /// Its largest section (all its pieces on one layer), in mm².
    section_mm2: f64,
}

/// The size of the island of `members` (its pieces, by layer).
fn island_size(ctx: &ReviewContext<'_>, scan: &PieceScan, members: &[RegionRef]) -> IslandSize {
    let spans = ctx.input.layers();
    let pixel_mm2 = ctx.input.pixel_area_mm2();
    let first = members.iter().map(|m| m.layer).min().unwrap_or(0);
    let last = members.iter().map(|m| m.layer).max().unwrap_or(0);
    let mut section_px = vec![0u64; last - first + 1];
    let mut volume_mm3 = 0.0;
    for member in members {
        let area = scan.layers[member.layer][member.index as usize].area_px;
        section_px[member.layer - first] += area;
        volume_mm3 += area as f64 * pixel_mm2 * spans[member.layer].height_mm();
    }
    IslandSize {
        layers: last - first + 1,
        height_mm: spans[last].top_mm - spans[first].bottom_mm,
        volume_mm3,
        section_mm2: section_px.iter().copied().max().unwrap_or(0) as f64 * pixel_mm2,
    }
}

impl IslandOptions {
    /// Whether an island of `size` that something sound holds is minimal
    /// (#402).
    fn is_minimal(&self, size: &IslandSize) -> bool {
        self.minimal_height_mm > 0.0
            && size.height_mm <= self.minimal_height_mm + island_policy::HEIGHT_TOLERANCE_MM
            && size.volume_mm3 < self.minimal_volume_mm3
            && size.section_mm2 <= self.minimal_section_mm2 + 1e-12
    }
}

/// The findings for the starts worth reporting, and the pixels that join
/// islands to the model below them, by the index their fixes name.
fn report(
    ctx: &ReviewContext<'_>,
    scan: &PieceScan,
    mut islands: Islands,
    options: &IslandOptions,
) -> (Vec<Finding>, Vec<CavityFill>) {
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
    let spans = ctx.input.layers();
    let pixel_mm2 = ctx.input.pixel_area_mm2();
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
        if chosen.is_empty() {
            continue;
        }
        let last_layer = members[members.len() - 1].layer;
        let held = held_from_above[group];
        let size = island_size(ctx, scan, members);
        let minimal = (held && options.is_minimal(&size)).then_some(size);
        for start in chosen {
            let largest_px = members
                .iter()
                .filter(|m| m.layer >= start.at.layer)
                .map(|&m| piece(m).area_px)
                .max()
                .unwrap_or(0);
            // The layers a support may climb to, and the island's pieces
            // on them (#405).
            let climb: Vec<(RegionRef, u64)> =
                if minimal.is_none() && options.support_climb_mm > 0.0 {
                    let bottom = spans[start.at.layer].bottom_mm;
                    members
                        .iter()
                        .filter(|m| {
                            m.layer > start.at.layer
                                && spans[m.layer].bottom_mm - bottom
                                    <= options.support_climb_mm + island_policy::HEIGHT_TOLERANCE_MM
                        })
                        .map(|&m| {
                            let section = members
                                .iter()
                                .filter(|o| o.layer == m.layer)
                                .map(|&o| piece(o).area_px)
                                .sum();
                            (m, section)
                        })
                        .collect()
                } else {
                    Vec::new()
                };
            reported.push(Reported {
                start,
                last_layer,
                largest_px,
                held_from_above: held,
                minimal,
                climb,
            });
        }
    }
    let keep_px = options.keep_px(ctx.input);
    let mut layers: Vec<usize> = reported.iter().map(|r| r.start.at.layer).collect();
    layers.sort_unstable();
    layers.dedup();
    let described: Vec<(Finding, Option<CavityFill>)> = layers
        .par_iter()
        .flat_map_iter(|&layer| {
            let cut = LayerPieces::compute(ctx.input, layer, keep_px);
            let below = Below::read(ctx, options, layer);
            reported
                .iter()
                .filter(move |r| r.start.at.layer == layer)
                .map(|r| {
                    island_finding(ctx, options, r, piece(r.start.at), &cut, &below, pixel_mm2)
                })
                .collect::<Vec<_>>()
        })
        .collect();
    // The joins, numbered in the order of the findings.
    let mut joins = Vec::new();
    let findings = described
        .into_iter()
        .map(|(mut finding, join)| {
            if let Some(join) = join {
                if let Some(SuggestedFix::JoinBelow { join: index, .. }) = &mut finding.fix {
                    *index = joins.len();
                }
                joins.push(join);
            }
            finding
        })
        .collect();
    (findings, joins)
}

/// What lies below one layer, as the islands starting on it need it: the
/// solid of the layers the exposure cures through to (#403), merged, and
/// the layers further down within a join's gap (#235), each with its gap.
struct Below {
    /// Solid of the layers below that hold a pixel above them.
    held: Vec<Run>,
    /// The layers below that, within [`IslandOptions::join_gap_mm`], from
    /// the nearest: the layer, the gap of liquid above it in mm, its pixels.
    /// Read the first time a join is looked for: a minimal island, or one
    /// of support, needs none.
    deeper: std::cell::OnceCell<Vec<(usize, f64, LayerPixels)>>,
    /// How many layers below hold a pixel (`held`'s).
    reach: usize,
    layer: usize,
}

impl Below {
    fn read(ctx: &ReviewContext<'_>, options: &IslandOptions, layer: usize) -> Self {
        let reach = 1 + pieces::cured_gap_layers(ctx.input, layer, options.cure_depth_mm);
        let mut held: Vec<Run> = Vec::new();
        for below in 1..=reach.min(layer) {
            held = runs::union(&held, &LayerPixels::compute(ctx.input, layer - below).solid);
        }
        Self {
            held,
            deeper: std::cell::OnceCell::new(),
            reach,
            layer,
        }
    }

    /// The layers within a join's gap below, read once.
    fn deeper(
        &self,
        ctx: &ReviewContext<'_>,
        options: &IslandOptions,
    ) -> &[(usize, f64, LayerPixels)] {
        self.deeper.get_or_init(|| {
            let (layer, reach) = (self.layer, self.reach);
            let spans = ctx.input.layers();
            let mut deeper = Vec::new();
            if options.join_gap_mm <= 0.0 {
                return deeper;
            }
            let mut gap: f64 = (1..=reach.min(layer))
                .map(|below| spans[layer - below].height_mm())
                .sum();
            let mut below = reach + 1;
            while below <= layer && gap <= options.join_gap_mm + island_policy::HEIGHT_TOLERANCE_MM
            {
                deeper.push((
                    layer - below,
                    gap,
                    LayerPixels::compute(ctx.input, layer - below),
                ));
                gap += spans[layer - below].height_mm();
                below += 1;
            }
            deeper
        })
    }
}

/// The pixels of the model below `unheld` (the pixels of an island's first
/// layer that nothing holds) across a gap within the join's reach, and the
/// fill that closes that gap under them: for each layer of the gap, the
/// pixels whose column reaches model below it (#235). Only model: a pixel
/// over support first is never joined.
fn join_below(
    ctx: &ReviewContext<'_>,
    options: &IslandOptions,
    unheld: &[Run],
    below: &Below,
    layer: usize,
) -> Option<(u64, f64, CavityFill)> {
    let mut open = unheld.to_vec();
    let mut joined_px = 0u64;
    let mut widest_gap: f64 = 0.0;
    // Each layer of the gap, from the nearest: the columns that go through
    // it to model further down.
    let mut columns: Vec<(usize, Vec<Run>)> = Vec::new();
    let mut found: Vec<(usize, Vec<Run>)> = Vec::new();
    for (lower, gap, pixels) in below.deeper(ctx, options) {
        if open.is_empty() {
            break;
        }
        let model = pieces::intersect(&open, &pixels.model);
        if !model.is_empty() {
            joined_px += runs::area_px(&model);
            widest_gap = widest_gap.max(*gap);
            found.push((*lower, model));
        }
        open = crate::checks::thin::difference(&open, &pixels.solid);
    }
    if joined_px == 0 {
        return None;
    }
    // Every layer between the island and a column's model gets the column.
    for gap_layer in (0..layer).rev() {
        let through: Vec<Run> = found
            .iter()
            .filter(|(lower, _)| *lower < gap_layer)
            .fold(Vec::new(), |acc, (_, runs)| runs::union(&acc, runs));
        if through.is_empty() {
            break;
        }
        columns.push((gap_layer, through));
    }
    columns.reverse();
    Some((
        joined_px,
        widest_gap,
        CavityFill::from_runs(ctx.input, columns),
    ))
}

/// The finding for one reported start, whose layer is cut into `cut`, with
/// what lies `below` that layer; and the fill of its join, when it offers
/// one (its index is set later).
fn island_finding(
    ctx: &ReviewContext<'_>,
    options: &IslandOptions,
    reported: &Reported,
    piece: Piece,
    cut: &LayerPieces,
    below: &Below,
    pixel_mm2: f64,
) -> (Finding, Option<CavityFill>) {
    let start = &reported.start;
    let is_model = piece.model_px > 0;
    let runs = cut.runs_of(start.at.index);
    let prefer = is_model.then_some(cut.model.as_slice());
    let (pixel, on_model) = focus::deepest_unheld(&runs, &below.held, prefer)
        .unwrap_or(([runs[0].x0, runs[0].row], false));
    let focus = ctx.input.focus(
        start.at.layer,
        [f64::from(pixel[0]) + 0.5, f64::from(pixel[1]) + 0.5],
    );
    let scope = if is_model {
        RoleScope::Model
    } else {
        RoleScope::Support
    };
    if let Some(size) = reported.minimal {
        let title = if is_model {
            "Minimal island"
        } else {
            "Minimal floating support"
        };
        let finding = Finding::new(
            CheckKind::Island,
            Severity::Info,
            start.at.layer,
            reported.last_layer,
            focus,
            scope,
            title,
            minimal_detail(piece, &size, pixel_mm2, is_model),
        );
        return (finding, None);
    }
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
    let mut detail = detail(reported, piece, pixel_mm2);
    let finding = |detail: String| {
        Finding::new(
            CheckKind::Island,
            severity,
            start.at.layer,
            reported.last_layer,
            focus,
            scope,
            title,
            detail,
        )
    };
    if !(is_model && on_model) {
        return (finding(detail), None);
    }
    // A gap thinner than a support over model below: close it (#235).
    if options.join_gap_mm > 0.0 {
        let held = pieces::intersect(&runs, &below.held);
        let unheld = pieces::subtract(&runs, &held);
        if let Some((joined_px, gap_mm, fill)) =
            join_below(ctx, options, &unheld, below, start.at.layer)
        {
            let rest = Rest {
                model_px: start.rest.model_px + joined_px,
                support_px: start.rest.support_px,
            };
            if options.hold(rest, piece.area_px, true) >= 1.0 - TOLERANCE {
                detail.push_str(&format!(
                    " Model lies {} mm below it, too close for a support: curing that gap joins it to the model.",
                    significant(gap_mm)
                ));
                let finding = finding(detail).with_fix(SuggestedFix::JoinBelow {
                    at: ctx.input.underside(start.at.layer, focus),
                    join: 0,
                });
                return (finding, Some(fill));
            }
        }
    }
    let mut at = ctx.input.underside(start.at.layer, focus);
    // A first layer smaller than a tip: aim where the island has grown
    // to a tip's size, on the part that grew (#405).
    let tip_px = options.tip_contact_mm2 / pixel_mm2;
    if (piece.area_px as f64) < tip_px
        && let Some((higher, climbed)) = climb_point(ctx, options, reported, tip_px)
    {
        at = higher;
        detail.push_str(&format!(
            " Its first layer is smaller than a support tip, so the support aims {} higher, where it is large enough.",
            if climbed == 1 {
                "one layer".to_string()
            } else {
                format!("{climbed} layers")
            }
        ));
    }
    (
        finding(detail).with_fix(SuggestedFix::AddSupport { at }),
        None,
    )
}

/// Where a growing island's support goes when its first layer is smaller
/// than a tip (#405): on the lowest layer within the climb where its
/// section reaches `tip_px` pixels, at the deepest pixel of its largest
/// piece there that the layers below leave unheld (the part that grew), at
/// the bottom of that layer; and how many layers up that is. `None` when it
/// reaches that size nowhere within the climb, or the part that grew has
/// no model pixel.
fn climb_point(
    ctx: &ReviewContext<'_>,
    options: &IslandOptions,
    reported: &Reported,
    tip_px: f64,
) -> Option<(crate::finding::PlatePoint, usize)> {
    let layer = reported
        .climb
        .iter()
        .filter(|(_, section)| *section as f64 >= tip_px)
        .map(|(m, _)| m.layer)
        .min()?;
    let cut = LayerPieces::compute(ctx.input, layer, options.keep_px(ctx.input));
    // The island's largest piece on that layer, the first on a tie.
    let index = reported
        .climb
        .iter()
        .filter(|(m, _)| m.layer == layer)
        .map(|(m, _)| m.index)
        .max_by(|a, b| {
            cut.pieces[*a as usize]
                .area_px
                .cmp(&cut.pieces[*b as usize].area_px)
                .then(b.cmp(a))
        })?;
    let runs = cut.runs_of(index);
    let below = Below::read(
        ctx,
        &IslandOptions {
            join_gap_mm: 0.0,
            ..options.clone()
        },
        layer,
    );
    let (pixel, on_model) = focus::deepest_unheld(&runs, &below.held, Some(&cut.model))?;
    if !on_model {
        return None;
    }
    let focus = ctx.input.focus(
        layer,
        [f64::from(pixel[0]) + 0.5, f64::from(pixel[1]) + 0.5],
    );
    Some((
        ctx.input.underside(layer, focus),
        layer - reported.start.at.layer,
    ))
}

/// The sentence of a minimal island (#402): how small it is, how soon the
/// rest of the print holds it, and why it gets no support.
fn minimal_detail(piece: Piece, size: &IslandSize, pixel_mm2: f64, is_model: bool) -> String {
    let what = if is_model { "model" } else { "support" };
    let area = significant(piece.area_px as f64 * pixel_mm2);
    let within = if size.layers == 1 {
        format!(
            "the next layer holds it ({} mm)",
            significant(size.height_mm)
        )
    } else {
        format!(
            "the rest of the print holds it within {} layers ({} mm)",
            size.layers,
            significant(size.height_mm)
        )
    };
    format!(
        "{area} mm² of {what} starts with nothing cured below it, but {within}, {} mm³ in all and never larger than {} mm². The next layers lift it with them; a support this small would do more harm than good.",
        significant(size.volume_mm3),
        significant(size.section_mm2),
    )
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
