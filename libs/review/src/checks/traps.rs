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
//! the film, further from the plate. *Air* is every pixel of a layer that is
//! not solid, which in the vat is liquid resin.
//!
//! Resin needs an opening of some width to flow. The check reads every
//! layer's air *opened* by a square [`drain_width_px`] pixels on a side:
//! only the air pixels that such a square of air covers are kept. A crack
//! narrower than that, through a wall, between two parts or anywhere else,
//! is a hairline that thick resin barely passes and light bleed tends to
//! cure shut: it closes, whichever way it runs. Wider air keeps its exact
//! shape. The opened air is split into 4-connected regions; a region that
//! reaches the screen edge is *open air*.
//!
//! Two air regions of consecutive layers are *connected* when the air they
//! share straight above one another, opened the same way, holds at least
//! [`drain_px`] pixels. A smaller link is a pinhole: it neither drains nor
//! vents a cavity, and it does not join two cavities into one.
//!
//! The shared [`PlateRegions`](crate::regions::PlateRegions) join air
//! through a single pixel, so this check builds its own air map, at about
//! the cost of the shared regions.
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
//! The air between support trunks reaches the screen edge, so it is open air
//! and drains whatever sits on it; a pocket that trunks and a model close
//! only within one layer drains through the open air below it.
//!
//! A cup walled by the supports alone is a pocket in the support structure:
//! a hole in a raft around a model's foot, or a trench between a raft and
//! the support bases fused on it. Such a *support pocket* is not reported:
//! every region of it has an outer wall (the solid around it, not the
//! islands standing in it) at least half support, and its floor is at least
//! half support or build plate. It comes off with the supports, a drain in
//! the model cannot reach it, and in a raft it pulls on the film over far
//! less area than the raft does. A cavity with a region walled by the model
//! on any layer, such as a recess under a model, even over a raft hole or
//! among support pads, is reported. Resin traps are always reported.
//!
//! # What is not reported
//!
//! Real plates hold many cavities too small to matter: on a 1.9-million
//! triangle miniature with generated supports, over a thousand, nearly all
//! one or two pixels on a single layer, slivers where support meshes merge.
//! A cavity is reported only when it holds at least [`drain_px`] pixels on
//! some layer, and, for a trap, at least
//! [`min_trap_volume_mm3`](TrapOptions::min_trap_volume_mm3) or, for a cup,
//! at least [`min_cup_area_mm2`](TrapOptions::min_cup_area_mm2) of
//! cross-section.
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
//!   information. The fix is to fill it, or to leave it;
//! - **medium**, under [`error_volume_mm3`](TrapOptions::error_volume_mm3):
//!   a warning. The fix is to fill it;
//! - **large**: an error. The fix is a drain.
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
//! The suggested fix of a large trap is a drain at that point, at the bottom
//! of the lowest layer: the floor of the cavity, the side facing the plate.
//! A tiny or medium trap, and a cup no drain helps that holds less than
//! [`error_volume_mm3`](TrapOptions::error_volume_mm3), get a
//! [`SuggestedFix::Fill`] instead: its exact pixels, cured solid on the next
//! slice ([`run_with_fills`], [`crate::fill`]). Filling a cup also fills the
//! recess it makes in the model's surface.
//!
//! - A large **trap** gets a drain unless the solid straight below that point is
//!   support alone. The resin collects there when the part stands on its
//!   base, and a hole drilled toward the plate side lets it out once the
//!   part is off the plate.
//! - A **cup** gets one only when a hole straight down from that point,
//!   through model alone, reaches drained air within
//!   [`max_drain_mm`](TrapOptions::max_drain_mm): then the cavity is no
//!   longer closed toward the plate. A cup on layer 0, on solid all the way
//!   down to the plate, over a thicker base or over supports gets none: its
//!   drain would end at the plate, deep in the part or in a support, and
//!   relieve nothing while printing.
//!
//! The scope (model, support or both) comes from the roles of the solid
//! around the cavity on every layer: its walls on all four sides, its floor
//! and its lid.
//!
//! [`drain_px`]: TrapOptions::drain_px
//! [`drain_width_px`]: TrapOptions::drain_width_px

#[path = "traps_air.rs"]
mod air;

use crate::fill::CavityFill;
use crate::finding::{CheckKind, Finding, PlatePoint, RoleScope, Severity, SuggestedFix};
use crate::input::ReviewInput;
use crate::regions::Region;
use crate::runner::ReviewContext;
use crate::union_find::{Groups, RegionRef};

use air::{AirMap, Pixel, Roles};

/// Default of [`TrapOptions::drain_px`].
pub const DEFAULT_DRAIN_PX: u64 = 10;

/// Default of [`TrapOptions::drain_width_px`].
pub const DEFAULT_DRAIN_WIDTH_PX: u32 = 3;

/// Default of [`TrapOptions::min_trap_volume_mm3`].
pub const DEFAULT_MIN_TRAP_VOLUME_MM3: f64 = 0.01;

/// Default of [`TrapOptions::min_cup_area_mm2`].
pub const DEFAULT_MIN_CUP_AREA_MM2: f64 = 1.0;

/// Default of [`TrapOptions::info_volume_mm3`].
pub const DEFAULT_INFO_VOLUME_MM3: f64 = 1.0;

/// Default of [`TrapOptions::error_volume_mm3`].
pub const DEFAULT_ERROR_VOLUME_MM3: f64 = 27.0;

/// Default of [`TrapOptions::error_cup_area_mm2`].
pub const DEFAULT_ERROR_CUP_AREA_MM2: f64 = 100.0;

/// Default of [`TrapOptions::max_drain_mm`].
pub const DEFAULT_MAX_DRAIN_MM: f64 = 5.0;

/// Settings of the resin-trap and suction-cup check.
#[derive(Clone, Debug, PartialEq)]
pub struct TrapOptions {
    /// The smallest link, in opened air pixels shared straight above one
    /// another, through which the air of one layer drains into, or vents
    /// to, the air of the next. 10 pixels is about 0.005 mm² on a 19 x 24 µm
    /// screen. A cavity smaller than this on every layer is not reported
    /// either.
    pub drain_px: u64,
    /// The narrowest opening, in pixels, that resin flows through, within a
    /// layer or from one layer to the next: air is kept only where a square
    /// of air this many pixels on a side covers it. 3 pixels is 57 x 72 µm
    /// on a 19 x 24 µm screen; a narrower crack is a hairline that thick
    /// resin barely passes and light bleed tends to cure shut. 0 or 1 keeps
    /// every air pixel.
    pub drain_width_px: u32,
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
    /// Resin traps smaller than this, in cubic millimetres, are *tiny*:
    /// information, to fill or to leave. A cubic millimetre is a drop about
    /// a millimetre across, a microlitre of resin: too little to crack a
    /// part, and partly cured by light bleeding through its walls anyway.
    pub info_volume_mm3: f64,
    /// Resin traps at least this large, in cubic millimetres, are *large*:
    /// errors, to drain. Smaller ones, down to
    /// [`info_volume_mm3`](Self::info_volume_mm3), are warnings, to fill.
    /// 27 mm³ is a 3 mm cube: about the smallest cavity a common 2-3 mm
    /// drain is not as large as. Filling a smaller one cures at most 27 µl
    /// of resin and keeps the part whole. It also caps the cups offered a
    /// fill.
    pub error_volume_mm3: f64,
    /// Suction cups whose largest cross-section is at least this, in square
    /// millimetres, are errors; smaller ones are warnings. At up to 0.1 N
    /// per mm², a 100 mm² cup adds up to 10 N to every peel it seals: enough
    /// to tear thin walls, blow holes through the part or pull it off its
    /// supports.
    pub error_cup_area_mm2: f64,
    /// The longest drain suggested for a suction cup, in millimetres of
    /// solid between the cup's floor and the drained air below it. A deeper
    /// hole is no practical fix; 5 mm is more than the walls of a hollowed
    /// part.
    pub max_drain_mm: f64,
}

impl Default for TrapOptions {
    fn default() -> Self {
        Self {
            drain_px: DEFAULT_DRAIN_PX,
            drain_width_px: DEFAULT_DRAIN_WIDTH_PX,
            min_trap_volume_mm3: DEFAULT_MIN_TRAP_VOLUME_MM3,
            min_cup_area_mm2: DEFAULT_MIN_CUP_AREA_MM2,
            info_volume_mm3: DEFAULT_INFO_VOLUME_MM3,
            error_volume_mm3: DEFAULT_ERROR_VOLUME_MM3,
            error_cup_area_mm2: DEFAULT_ERROR_CUP_AREA_MM2,
            max_drain_mm: DEFAULT_MAX_DRAIN_MM,
        }
    }
}

/// Finds the resin traps and suction cups of the plate.
pub fn run(ctx: &ReviewContext<'_>, options: &TrapOptions) -> Vec<Finding> {
    run_with_fills(ctx, options).0
}

/// Finds the resin traps and suction cups of the plate, with the pixels of
/// every cavity offered a fill: a [`SuggestedFix::Fill`] names its
/// [`CavityFill`] by index in the second list.
pub fn run_with_fills(
    ctx: &ReviewContext<'_>,
    options: &TrapOptions,
) -> (Vec<Finding>, Vec<CavityFill>) {
    let input = ctx.input;
    if input.layer_count() == 0 {
        return (Vec::new(), Vec::new());
    }
    let map = AirMap::compute(input, options.drain_width_px);
    let drained = drain_pass(&map, options.drain_px);
    let (groups, vented) = vent_pass(&map, &drained, options.drain_px);
    let cavities: Vec<(Cavity<'_>, Remedy)> = (0..groups.len())
        .filter(|&group| {
            let lowest = groups.members(group)[0];
            !drained[lowest.layer][lowest.index as usize]
        })
        .map(|group| Cavity::measure(input, &map, groups.members(group), vented[group]))
        .filter(|cavity| cavity.is_reported(options))
        .map(|cavity| {
            let remedy = cavity.remedy(input, &drained, options);
            (cavity, remedy)
        })
        .collect();
    let filled: Vec<&[RegionRef]> = cavities
        .iter()
        .filter(|(_, remedy)| *remedy == Remedy::Fill)
        .map(|(cavity, _)| cavity.members)
        .collect();
    let fills: Vec<CavityFill> = air::cavity_runs(input, options.drain_width_px, &filled)
        .into_iter()
        .map(|layers| CavityFill::from_runs(input, layers))
        .collect();
    let mut next_fill = 0;
    let findings = cavities
        .iter()
        .map(|(cavity, remedy)| {
            let fix = match *remedy {
                Remedy::Drain(fix) => fix,
                Remedy::Fill => {
                    next_fill += 1;
                    Some(SuggestedFix::Fill {
                        at: cavity.floor_at(input),
                        cavity: next_fill - 1,
                    })
                }
            };
            cavity.finding(input, fix, options)
        })
        .collect();
    (findings, fills)
}

/// How a cavity is fixed.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Remedy {
    /// A drain, when one works.
    Drain(Option<SuggestedFix>),
    /// Cure it solid.
    Fill,
}

/// Pass 1, from the plate up: which air regions of every layer are drained.
fn drain_pass(map: &AirMap, drain_px: u64) -> Vec<Vec<bool>> {
    let mut drained: Vec<Vec<bool>> = Vec::with_capacity(map.layers.len());
    for (layer, air) in map.layers.iter().enumerate() {
        let mut flags: Vec<bool> = air
            .regions
            .iter()
            .map(|region| region.touches_border)
            .collect();
        if let Some(below) = layer.checked_sub(1).map(|lower| &drained[lower]) {
            for link in &map.links[layer] {
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
fn vent_pass(map: &AirMap, drained: &[Vec<bool>], drain_px: u64) -> (Groups, Vec<bool>) {
    let candidate = |layer: usize, index: u32| !drained[layer][index as usize];
    let groups = map.group(|upper, link| {
        link.pixels >= drain_px && candidate(upper - 1, link.lower) && candidate(upper, link.upper)
    });
    let mut vented = vec![false; groups.len()];
    let top = map.layers.len() - 1;
    for upper in (1..=top).rev() {
        let lower = upper - 1;
        for link in &map.links[upper] {
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
    for index in 0..map.layers[top].regions.len() as u32 {
        if candidate(top, index) {
            vented[groups.group_of(RegionRef { layer: top, index })] = true;
        }
    }
    (groups, vented)
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
    /// The area of its widest layer.
    largest_section_mm2: f64,
    /// Where it is aimed at: the inside pixel of the largest region of its
    /// lowest layer, in raster space.
    floor_px: [u32; 2],
    /// The solid around it on every layer: walls, floor and lid.
    around: Roles,
    /// Whether it is a pocket in the support structure: every region with
    /// an outer wall at least half support, on a floor at least half
    /// support or build plate.
    support_pocket: bool,
}

impl<'g> Cavity<'g> {
    fn measure(
        input: &ReviewInput<'_>,
        map: &AirMap,
        members: &'g [RegionRef],
        vented: bool,
    ) -> Self {
        let pixel_mm2 = input.pixel_area_mm2();
        let first_layer = members[0].layer;
        let mut volume_mm3 = 0.0;
        let mut largest_px = 0u64;
        let mut floor: Option<&Region> = None;
        let mut around = Roles::default();
        let mut base = Roles::default();
        let mut outer_support = true;
        for layer_members in members.chunk_by(|a, b| a.layer == b.layer) {
            let layer = layer_members[0].layer;
            let air = &map.layers[layer];
            let mut area_px = 0u64;
            for member in layer_members {
                let (region, sides) = (
                    &air.regions[member.index as usize],
                    &air.sides[member.index as usize],
                );
                area_px += region.area_px;
                around.add(sides.walls);
                around.add(sides.floor);
                around.add(sides.lid);
                base.add(sides.floor);
                if layer == 0 {
                    // The build plate under it counts as support.
                    base.add(Roles {
                        px: region.area_px,
                        model: 0,
                        support: region.area_px,
                    });
                }
                outer_support &= sides.outer.mostly_support();
            }
            volume_mm3 += area_px as f64 * pixel_mm2 * input.layers()[layer].height_mm();
            largest_px = largest_px.max(area_px);
            if layer == first_layer {
                floor = layer_members
                    .iter()
                    .map(|member| &air.regions[member.index as usize])
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
            largest_section_mm2: largest_px as f64 * pixel_mm2,
            floor_px: floor.map_or([0, 0], |region| region.inside_px),
            around,
            support_pocket: outer_support && base.mostly_support(),
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
    /// least one layer, and a trap holding enough resin or a cup, other than
    /// a support pocket, pulling on enough film.
    fn is_reported(&self, options: &TrapOptions) -> bool {
        self.largest_px >= options.drain_px
            && match self.kind() {
                CheckKind::ResinTrap => self.volume_mm3 >= options.min_trap_volume_mm3,
                _ => self.largest_section_mm2 >= options.min_cup_area_mm2 && !self.support_pocket,
            }
    }

    fn kind(&self) -> CheckKind {
        if self.vented || self.first_layer() == 0 {
            CheckKind::SuctionCup
        } else {
            CheckKind::ResinTrap
        }
    }

    /// The fix of the cavity (see the module documentation): a fill for a
    /// trap too small to drain, and for a cup as small that no drain helps;
    /// otherwise a drain, when one works.
    fn remedy(
        &self,
        input: &ReviewInput<'_>,
        drained: &[Vec<bool>],
        options: &TrapOptions,
    ) -> Remedy {
        let small = self.volume_mm3 < options.error_volume_mm3;
        match self.kind() {
            CheckKind::ResinTrap if small => Remedy::Fill,
            CheckKind::ResinTrap => Remedy::Drain(self.drain(input, drained, options)),
            _ => match self.drain(input, drained, options) {
                None if small => Remedy::Fill,
                drain => Remedy::Drain(drain),
            },
        }
    }

    /// The bottom of the floor point: where a drain is drilled and a fill
    /// is aimed.
    fn floor_at(&self, input: &ReviewInput<'_>) -> PlatePoint {
        let [x_mm, y_mm] = input.pixel_to_plate(self.floor_point());
        PlatePoint::new(x_mm, y_mm, input.layers()[self.first_layer()].bottom_mm)
    }

    /// The drain suggested for the cavity, if one would work (see the module
    /// documentation).
    fn drain(
        &self,
        input: &ReviewInput<'_>,
        drained: &[Vec<bool>],
        options: &TrapOptions,
    ) -> Option<SuggestedFix> {
        let first = self.first_layer();
        if first == 0 {
            return None;
        }
        let works = match self.kind() {
            CheckKind::ResinTrap => {
                let below = air::classify(
                    input,
                    options.drain_width_px,
                    drained,
                    &[first - 1],
                    self.floor_px,
                );
                below[0] != Pixel::Support
            }
            _ => self.reaches_drained_air(input, drained, options),
        };
        works.then(|| SuggestedFix::AddDrain {
            at: self.floor_at(input),
        })
    }

    /// Whether a hole straight down from the floor point, through model
    /// alone, reaches drained air within `max_drain_mm` of solid. The layers
    /// below are read a few at a time, in parallel, until one decides.
    fn reaches_drained_air(
        &self,
        input: &ReviewInput<'_>,
        drained: &[Vec<bool>],
        options: &TrapOptions,
    ) -> bool {
        let layers = input.layers();
        let floor_mm = layers[self.first_layer()].bottom_mm;
        // Layer `l` is reachable when the solid above it, up to the floor, is
        // no thicker than the longest drain.
        let reachable: Vec<usize> = (0..self.first_layer())
            .rev()
            .take_while(|&layer| floor_mm - layers[layer].top_mm <= options.max_drain_mm + 1e-9)
            .collect();
        let step = rayon::current_num_threads().max(1);
        for chunk in reachable.chunks(step) {
            let pixels =
                air::classify(input, options.drain_width_px, drained, chunk, self.floor_px);
            for pixel in pixels {
                match pixel {
                    Pixel::Model => {}
                    Pixel::Support => return false,
                    Pixel::Air { drained } => return drained,
                }
            }
        }
        // The plate, or solid deeper than the longest drain.
        false
    }

    fn finding(
        &self,
        input: &ReviewInput<'_>,
        fix: Option<SuggestedFix>,
        options: &TrapOptions,
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
                    options.drain_width_px, options.drain_px
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
        let scope = RoleScope::from_counts(self.around.model, self.around.support);
        let finding = Finding::new(kind, severity, first, last, focus, scope, title, detail);
        match fix {
            Some(fix) => finding.with_fix(fix),
            None => finding,
        }
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
