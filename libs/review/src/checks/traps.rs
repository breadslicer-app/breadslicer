/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Resin traps and suction cups (#83): cavities of air the resin cannot
//! leave.
//!
//! Reports [`CheckKind::ResinTrap`] and [`CheckKind::SuctionCup`] findings.
//!
//! # Air, and the openings resin flows through
//!
//! Layer 0 is cured on the build plate; every later layer is cured against
//! the film, further from the plate. The sweep reads every layer's air
//! *opened*, so a crack too narrow for resin closes, and links air regions
//! of consecutive layers through the air they share, opened the same way
//! (see [`crate::regions`]). Two air regions of consecutive layers are
//! *connected* when that link holds at least [`drain_px`] pixels. A smaller
//! link is a pinhole: it neither drains nor vents a cavity, and it does not
//! join two cavities into one.
//!
//! # The rule: a two-pass air map
//!
//! 1. **Drain pass, from the plate up.** An air region is *drained* when it
//!    is open air, or connected to a drained region of the layer below.
//!    Below layer 0 is the build plate, which closes everything. Every other
//!    air region is a *candidate*: a pocket closed toward the plate.
//! 2. **Vent pass, from the top down.** Connected candidates join into one
//!    cavity through the layers (union-find, so each cavity is one finding).
//!    A cavity is *vented* when one of its regions connects to drained air in
//!    the layer above, or lies on the last layer: nothing above the last
//!    layer is represented, and it is all open resin.
//!
//! A vented cavity is a **suction cup**: closed toward the plate and open
//! toward the film, it seals against the film and pulls on it at every peel
//! until the print reaches its opening. A cavity that is never vented is a
//! **resin trap**: a sealed volume that keeps its liquid resin after the
//! print. A cavity that starts on layer 0 is closed by the build plate
//! itself, so it opens when the part comes off the plate; until then it
//! seals against the film like any cup, and it is reported as one.
//!
//! # Supports
//!
//! The review reads no support geometry (#432): air is what the models leave.
//! A cavity is judged as if the supports were not there, which is what it is
//! once they come off, and a cup closed only by a raft is no cup.
//!
//! # What is not reported
//!
//! Real plates hold many cavities too small to matter, nearly all one or two
//! pixels on a single layer. A cavity is reported only when it holds at least [`drain_px`] pixels on
//! some layer, and, for a trap, at least
//! [`min_trap_volume_mm3`](TrapOptions::min_trap_volume_mm3) or, for a cup,
//! at least [`min_cup_area_mm2`](TrapOptions::min_cup_area_mm2) of
//! cross-section.
//!
//! A cup is not reported either when its section is under
//! [`min_cup_share`](TrapOptions::min_cup_share) of the solid cured on each
//! of its layers: the rest of the layer's peel force swamps its pull.
//!
//! Cups have no minimum height. A cup pulls with its sealed area at every
//! peel whatever its depth, since the resin inside cannot stretch, so one
//! peel of a wide cup can tear the part. The area limit keeps the noise out
//! instead. It is in square millimetres, not pixels, and is far above the
//! 100 pixels (0.05 mm² on a 19 x 24 µm screen) a pixel count would allow.
//!
//! # Severity: traps by size (#132)
//!
//! What a trap risks is its liquid resin: sealed in, it seeps out or cracks
//! the part later, and the more there is the worse. A few pixels of it do
//! not, and a drain several millimetres wide for them would do more harm
//! than the resin. Traps come in three tiers by volume ([`trap_severity`]):
//!
//! - **tiny**, under [`info_volume_mm3`](TrapOptions::info_volume_mm3):
//!   information;
//! - **medium**, under [`error_volume_mm3`](TrapOptions::error_volume_mm3):
//!   a warning;
//! - **large**: an error.
//!
//! A cup of [`error_cup_area_mm2`](TrapOptions::error_cup_area_mm2) or more
//! is an error, a smaller one a warning: what a cup risks is its pull on the
//! film, which grows with its area.
//!
//! # What a finding says
//!
//! One finding per cavity, over its layer range, with its volume (the air
//! pixels of every layer times the pixel area and that layer's height) and
//! its largest cross-section. The camera centres on the cavity's floor: the
//! pixel nearest the centroid of the largest air region on its lowest layer.
//!
//! # The fix: a hollow's volume is the person's (#431)
//!
//! A cavity may be the whole point of the model: a hollow, made here or in
//! the file as it came. So the review warns about it and never offers to
//! cure it, by size, since an imported hollow carries no record of being one:
//!
//! - A sealed trap under [`SMALL_TRAP_MAX_ML`] is a stray pocket, not a
//!   hollow: it gets a [`SuggestedFix::Fill`], its exact pixels cured solid
//!   on the next slice ([`run_with_fills`], [`crate::fill`]).
//! - Any other trap, and every cup whatever its size, gets a
//!   [`SuggestedFix::AddDrain`], with no pixels to fill: the person places
//!   the holes. It points at the bottom of the lowest layer, the floor of the
//!   cavity, the side facing the plate.
//!
//! [`drain_px`]: TrapOptions::drain_px

use std::sync::Arc;

use crate::fill::{CavityFill, FillLayerRuns, FillSlab};
use crate::finding::{
    CheckKind, Finding, MAX_FOOTPRINT_LAYERS, PlatePoint, RoleScope, Severity, SuggestedFix,
};
use crate::input::ReviewInput;
use crate::regions::{LayerRead, PlateRegions, Region, SweepOptions, read_layers, runs_in, wanted};
use crate::runner::ReviewContext;
use crate::runs::{dilate_square, intersection};
use crate::union_find::{Groups, RegionRef};

/// Default of [`TrapOptions::drain_px`].
pub const DEFAULT_DRAIN_PX: u64 = 10;

/// Default of [`TrapOptions::min_trap_volume_mm3`].
pub const DEFAULT_MIN_TRAP_VOLUME_MM3: f64 = 0.01;

/// Default of [`TrapOptions::min_cup_share`].
pub const DEFAULT_MIN_CUP_SHARE: f64 = 0.10;

/// Default of [`TrapOptions::min_cup_area_mm2`].
pub const DEFAULT_MIN_CUP_AREA_MM2: f64 = 1.0;

/// Default of [`TrapOptions::info_volume_mm3`].
pub const DEFAULT_INFO_VOLUME_MM3: f64 = 1.0;

/// Default of [`TrapOptions::error_volume_mm3`].
pub const DEFAULT_ERROR_VOLUME_MM3: f64 = 27.0;

/// Default of [`TrapOptions::error_cup_area_mm2`].
pub const DEFAULT_ERROR_CUP_AREA_MM2: f64 = 100.0;

/// The volume, in millilitres, from which a sealed trap is no longer small:
/// from here on it may be a hollow, so it is never filled, whichever printer
/// it is for. Under it a trap is a stray pocket, and curing it costs less
/// than a millilitre of resin (#431).
pub const SMALL_TRAP_MAX_ML: f64 = 1.0;

/// Cubic millimetres in a millilitre.
const MM3_PER_ML: f64 = 1000.0;

/// Settings of the resin-trap and suction-cup check.
#[derive(Clone, Debug, PartialEq)]
pub struct TrapOptions {
    /// The smallest link, in opened air pixels shared straight above one
    /// another, through which the air of one layer drains into, or vents
    /// to, the air of the next. 10 pixels is about 0.005 mm² on a 19 x 24 µm
    /// screen. A cavity smaller than this on every layer is not reported
    /// either.
    pub drain_px: u64,
    /// Resin traps smaller than this, in cubic millimetres, are not
    /// reported. 0.01 mm³ is a cube about 0.2 mm on a side, ten nanolitres
    /// of resin: rasterisation slivers inside merged supports are smaller.
    pub min_trap_volume_mm3: f64,
    /// Suction cups whose largest cross-section is smaller than this, in
    /// square millimetres, are not reported. A cup pulls on the film with
    /// its sealed area times the pressure drop, at most about 0.1 N per mm²
    /// at a full vacuum, so a cup under a square millimetre is lost in the
    /// peel force of an ordinary layer.
    pub min_cup_area_mm2: f64,
    /// Suction cups whose cross-section is smaller than this share of the
    /// solid cured on each of their layers are not reported either: their
    /// pull is lost in the peel force of the layer around them. 10 % is a safe
    /// margin (#156, from printing).
    pub min_cup_share: f64,
    /// Resin traps smaller than this, in cubic millimetres, are *tiny*:
    /// information. A cubic millimetre is a drop about
    /// a millimetre across, a microlitre of resin: too little to crack a
    /// part, and partly cured by light bleeding through its walls anyway.
    pub info_volume_mm3: f64,
    /// Resin traps at least this large, in cubic millimetres, are *large*:
    /// errors. Smaller ones, down to
    /// [`info_volume_mm3`](Self::info_volume_mm3), are warnings. 27 mm³ is
    /// a 3 mm cube: about the smallest cavity a common 2-3 mm drain is not
    /// as large as.
    pub error_volume_mm3: f64,
    /// Suction cups whose largest cross-section is at least this, in square
    /// millimetres, are errors; smaller ones are warnings. At up to 0.1 N
    /// per mm², a 100 mm² cup adds up to 10 N to every peel it seals: enough
    /// to tear thin walls, blow holes through the part or pull it off its
    /// supports.
    pub error_cup_area_mm2: f64,
}

impl Default for TrapOptions {
    fn default() -> Self {
        Self {
            drain_px: DEFAULT_DRAIN_PX,
            min_trap_volume_mm3: DEFAULT_MIN_TRAP_VOLUME_MM3,
            min_cup_area_mm2: DEFAULT_MIN_CUP_AREA_MM2,
            min_cup_share: DEFAULT_MIN_CUP_SHARE,
            info_volume_mm3: DEFAULT_INFO_VOLUME_MM3,
            error_volume_mm3: DEFAULT_ERROR_VOLUME_MM3,
            error_cup_area_mm2: DEFAULT_ERROR_CUP_AREA_MM2,
        }
    }
}

/// Finds the resin traps and suction cups of the plate, with the pixels of
/// every small trap: a [`SuggestedFix::Fill`] names its [`CavityFill`] by
/// index in the second list. No other cavity has one (see the module
/// documentation).
pub fn run(
    ctx: &ReviewContext<'_>,
    options: &TrapOptions,
    sweep: &SweepOptions,
) -> (Vec<Finding>, Vec<CavityFill>) {
    let (input, regions) = (ctx.input, ctx.regions);
    if input.layer_count() == 0 {
        return (Vec::new(), Vec::new());
    }
    let drained = drain_pass(regions, options.drain_px);
    let (groups, vented) = vent_pass(regions, &drained, options.drain_px);
    let cavities: Vec<Cavity<'_>> = (0..groups.len())
        .filter(|&group| {
            let lowest = groups.members(group)[0];
            !drained[lowest.layer][lowest.index as usize]
        })
        .map(|group| Cavity::measure(input, regions, groups.members(group), vented[group]))
        .filter(|cavity| cavity.is_reported(options) && cavity.pulls(ctx, options))
        .collect();
    // A fill needs every pixel; any other cavity, those it highlights.
    let read: Vec<&[RegionRef]> = cavities.iter().map(Cavity::regions_read).collect();
    let pixels = cavity_runs(input, sweep, &read)
        .into_iter()
        .map(|layers| CavityFill::with_rims(input, layers));
    let mut fills = Vec::new();
    let mut findings = Vec::with_capacity(cavities.len());
    for (cavity, pixels) in cavities.iter().zip(pixels) {
        let at = cavity.floor_at(input);
        // The cavity's own pixels, for a frontend to highlight (#409): its
        // first layers, without the rim.
        let footprint = CavityFill {
            slabs: pixels
                .slabs
                .iter()
                .take(MAX_FOOTPRINT_LAYERS)
                .map(|slab| FillSlab {
                    rim: Vec::new(),
                    ..slab.clone()
                })
                .collect(),
        };
        let fix = if cavity.is_small_trap() {
            fills.push(pixels);
            SuggestedFix::Fill {
                at,
                cavity: fills.len() - 1,
            }
        } else {
            SuggestedFix::AddDrain { at }
        };
        findings.push(
            cavity
                .finding(input, fix, options, sweep)
                .with_footprint(Arc::new(footprint)),
        );
    }
    (findings, fills)
}

/// Pass 1, from the plate up: which air regions of every layer are drained.
fn drain_pass(regions: &PlateRegions, drain_px: u64) -> Vec<Vec<bool>> {
    let mut drained: Vec<Vec<bool>> = Vec::with_capacity(regions.layers().len());
    for (layer, table) in regions.layers().iter().enumerate() {
        let mut flags: Vec<bool> = table
            .air
            .iter()
            .map(|region| region.touches_border)
            .collect();
        if let Some(below) = layer.checked_sub(1).map(|lower| &drained[lower]) {
            for link in regions.air_links(layer) {
                if link.pixels >= drain_px && below[link.lower as usize] {
                    flags[link.upper as usize] = true;
                }
            }
        }
        drained.push(flags);
    }
    drained
}

/// Pass 2, from the top down: the candidates joined into cavities, and
/// whether each group is vented (meaningful for cavities only; a drained
/// region is a group of its own).
fn vent_pass(regions: &PlateRegions, drained: &[Vec<bool>], drain_px: u64) -> (Groups, Vec<bool>) {
    let candidate = |layer: usize, index: u32| !drained[layer][index as usize];
    let groups = regions.group_air(|upper, link| {
        link.pixels >= drain_px && candidate(upper - 1, link.lower) && candidate(upper, link.upper)
    });
    let mut vented = vec![false; groups.len()];
    let top = regions.layers().len() - 1;
    for upper in (1..=top).rev() {
        let lower = upper - 1;
        for link in regions.air_links(upper) {
            if link.pixels >= drain_px
                && candidate(lower, link.lower)
                && !candidate(upper, link.upper)
            {
                let region = RegionRef {
                    layer: lower,
                    index: link.lower,
                };
                vented[groups.group_of(region)] = true;
            }
        }
    }
    for index in 0..regions.layer(top).air.len() as u32 {
        if candidate(top, index) {
            vented[groups.group_of(RegionRef { layer: top, index })] = true;
        }
    }
    (groups, vented)
}

/// The opened air runs of each cavity in `cavities` (its member regions),
/// layer by layer from the plate up, with the model runs of their rim: the
/// pixels a fill cures (see [`crate::fill`]). Each layer any cavity touches
/// is rasterised once, in parallel.
fn cavity_runs(
    input: &ReviewInput<'_>,
    sweep: &SweepOptions,
    cavities: &[&[RegionRef]],
) -> Vec<Vec<FillLayerRuns>> {
    let wants = cavities
        .iter()
        .enumerate()
        .flat_map(|(cavity, members)| members.iter().map(move |member| wanted(member, cavity)))
        .collect();
    let (width_px, height_px) = (input.width_px(), input.height_px());
    read_layers(
        input,
        sweep,
        LayerRead::All,
        wants,
        cavities.len(),
        |pixels, regions| {
            let runs = runs_in(&pixels.air, &pixels.air_labels, regions);
            // The model pixels touching the cavity, sides or corners.
            let rim = intersection(&dilate_square(&runs, 1, width_px, height_px), &pixels.model);
            (runs, rim)
        },
    )
    .into_iter()
    .map(|layers| {
        layers
            .into_iter()
            .map(|(layer, (runs, rim))| (layer, runs, rim))
            .collect()
    })
    .collect()
}

/// One cavity: a group of candidate air regions.
#[derive(Debug)]
struct Cavity<'g> {
    /// Its regions, lowest first.
    members: &'g [RegionRef],
    /// Whether it connects to drained air above or reaches the last layer.
    vented: bool,
    /// Its volume.
    volume_mm3: f64,
    /// The air pixels of its widest layer.
    largest_px: u64,
    /// Its air pixels on each of its layers.
    sections: Vec<(usize, u64)>,
    /// The area of its widest layer.
    largest_section_mm2: f64,
    /// Where it is aimed at: the inside pixel of the largest region of its
    /// lowest layer, in raster space.
    floor_px: [u32; 2],
}

impl<'g> Cavity<'g> {
    fn measure(
        input: &ReviewInput<'_>,
        regions: &PlateRegions,
        members: &'g [RegionRef],
        vented: bool,
    ) -> Self {
        let pixel_mm2 = input.pixel_area_mm2();
        let first_layer = members[0].layer;
        let mut volume_mm3 = 0.0;
        let mut largest_px = 0u64;
        let mut sections = Vec::new();
        let mut floor: Option<&Region> = None;
        for layer_members in members.chunk_by(|a, b| a.layer == b.layer) {
            let layer = layer_members[0].layer;
            let air = &regions.layer(layer).air;
            let area_px: u64 = layer_members
                .iter()
                .map(|member| air[member.index as usize].area_px)
                .sum();
            volume_mm3 += area_px as f64 * pixel_mm2 * input.layers()[layer].height_mm();
            largest_px = largest_px.max(area_px);
            sections.push((layer, area_px));
            if layer == first_layer {
                floor = layer_members
                    .iter()
                    .map(|member| &air[member.index as usize])
                    .reduce(|best, region| {
                        if region.area_px > best.area_px {
                            region
                        } else {
                            best
                        }
                    });
            }
        }
        Self {
            members,
            vented,
            volume_mm3,
            largest_px,
            sections,
            largest_section_mm2: largest_px as f64 * pixel_mm2,
            floor_px: floor.map_or([0, 0], |region| region.inside_px),
        }
    }

    fn first_layer(&self) -> usize {
        self.members[0].layer
    }

    fn last_layer(&self) -> usize {
        self.members[self.members.len() - 1].layer
    }

    /// The centre of the floor pixel, in raster space.
    fn floor_point(&self) -> [f64; 2] {
        [
            f64::from(self.floor_px[0]) + 0.5,
            f64::from(self.floor_px[1]) + 0.5,
        ]
    }

    /// Whether the cavity is worth a finding: as large as a drain on at
    /// least one layer, and a trap holding enough resin or a cup pulling on
    /// enough film.
    fn is_reported(&self, options: &TrapOptions) -> bool {
        self.largest_px >= options.drain_px
            && match self.kind() {
                CheckKind::ResinTrap => self.volume_mm3 >= options.min_trap_volume_mm3,
                _ => self.largest_section_mm2 >= options.min_cup_area_mm2,
            }
    }

    /// Whether a cup pulls on the film for some layer: its section there is
    /// at least [`min_cup_share`](TrapOptions::min_cup_share) of the solid
    /// cured on that layer. A trap always counts.
    fn pulls(&self, ctx: &ReviewContext<'_>, options: &TrapOptions) -> bool {
        self.kind() != CheckKind::SuctionCup
            || self.sections.iter().any(|&(layer, area_px)| {
                let solid_px: u64 = ctx
                    .regions
                    .layer(layer)
                    .solid
                    .iter()
                    .map(|region| region.area_px)
                    .sum();
                area_px as f64 >= options.min_cup_share * solid_px as f64
            })
    }

    fn kind(&self) -> CheckKind {
        if self.vented || self.first_layer() == 0 {
            CheckKind::SuctionCup
        } else {
            CheckKind::ResinTrap
        }
    }

    /// Whether it is a sealed trap small enough to fill (see the module
    /// documentation).
    fn is_small_trap(&self) -> bool {
        self.kind() == CheckKind::ResinTrap && self.volume_mm3 < SMALL_TRAP_MAX_ML * MM3_PER_ML
    }

    /// The regions whose pixels are read: all of a small trap, for its
    /// fill; of any other cavity, those of the layers its footprint keeps.
    fn regions_read(&self) -> &'g [RegionRef] {
        if self.is_small_trap() {
            return self.members;
        }
        let end = self.first_layer() + MAX_FOOTPRINT_LAYERS;
        &self.members[..self.members.partition_point(|member| member.layer < end)]
    }

    /// The bottom of the floor point: where a drain or a fill is aimed.
    fn floor_at(&self, input: &ReviewInput<'_>) -> PlatePoint {
        let [x_mm, y_mm] = input.pixel_to_plate(self.floor_point());
        PlatePoint::new(x_mm, y_mm, input.layers()[self.first_layer()].bottom_mm)
    }

    fn finding(
        &self,
        input: &ReviewInput<'_>,
        fix: SuggestedFix,
        options: &TrapOptions,
        sweep: &SweepOptions,
    ) -> Finding {
        let (first, last) = (self.first_layer(), self.last_layer());
        let layers = input.layers();
        let span = format!(
            "{} ({:.2} mm)",
            plural(last - first + 1, "layer", "layers"),
            layers[last].top_mm - layers[first].bottom_mm,
        );
        let volume = amount(self.volume_mm3);
        let section = amount(self.largest_section_mm2);
        let kind = self.kind();
        let worse_than = |value: f64, limit: f64| {
            if value >= limit {
                Severity::Error
            } else {
                Severity::Warning
            }
        };
        let (severity, title, detail) = match kind {
            CheckKind::ResinTrap => (
                trap_severity(self.volume_mm3, options),
                match trap_severity(self.volume_mm3, options) {
                    Severity::Error => "Sealed cavity traps resin",
                    Severity::Warning => "Small sealed cavity traps resin",
                    Severity::Info => "Tiny sealed cavity",
                },
                format!(
                    "A sealed cavity of {volume} mm³ over {span}, up to {section} mm² wide, has no opening at least {} pixels wide and {} pixels in area, so its liquid resin stays inside.",
                    sweep.drain_width_px, options.drain_px
                ),
            ),
            _ if first == 0 => (
                worse_than(self.largest_section_mm2, options.error_cup_area_mm2),
                "Cavity sealed by the build plate",
                format!(
                    "A cavity of {volume} mm³ over {span} is closed by the build plate below, so up to {section} mm² of it pulls on the film at every peel."
                ),
            ),
            _ => (
                worse_than(self.largest_section_mm2, options.error_cup_area_mm2),
                "Cavity closed toward the plate",
                format!(
                    "A cavity of {volume} mm³ over {span} is closed toward the plate and opens only above, so up to {section} mm² of it pulls on the film at every peel."
                ),
            ),
        };
        let focus = input.focus(first, self.floor_point());
        let detail = match fix {
            SuggestedFix::AddDrain { .. } => format!("{detail} Add drain holes."),
            _ => detail,
        };
        Finding::new(
            kind,
            severity,
            first,
            last,
            focus,
            RoleScope::Model,
            title,
            detail,
        )
        .with_fix(fix)
    }
}

/// The tier of a resin trap holding `volume_mm3` of resin: tiny
/// ([`Severity::Info`]), medium ([`Severity::Warning`]) or large
/// ([`Severity::Error`]).
pub fn trap_severity(volume_mm3: f64, options: &TrapOptions) -> Severity {
    if volume_mm3 < options.info_volume_mm3 {
        Severity::Info
    } else if volume_mm3 < options.error_volume_mm3 {
        Severity::Warning
    } else {
        Severity::Error
    }
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// A quantity with as many decimals as its size deserves.
fn amount(value: f64) -> String {
    if value >= 10.0 {
        format!("{value:.1}")
    } else if value >= 1.0 {
        format!("{value:.2}")
    } else {
        format!("{value:.3}")
    }
}

#[cfg(test)]
#[path = "tests_traps.rs"]
mod tests;
