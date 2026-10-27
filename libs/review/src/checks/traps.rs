/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Resin traps and suction cups (#83): cavities of air the resin cannot
//! leave.
//!
//! Reports [`CheckKind::ResinTrap`] and [`CheckKind::SuctionCup`] findings
//! from every layer's air [`Region`]s (4-connected; `touches_border` marks
//! the open air) and the air overlaps between consecutive layers
//! ([`PlateRegions::air_links`]).
//!
//! # The rule: a two-pass air map
//!
//! Layer 0 is cured on the build plate; every later layer is cured against
//! the film, further from the plate. *Air* is every pixel of a layer that is
//! not solid, which in the vat is liquid resin. An air region that reaches
//! the screen edge is *open air*.
//!
//! Two air regions of consecutive layers are *connected* when they share at
//! least [`TrapOptions::drain_px`] pixels straight above one another. A
//! narrower link is a pinhole that resin barely flows through: it neither
//! drains nor vents a cavity, and it does not join two cavities into one.
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
//! Supports: the air between support trunks reaches the screen edge, so it
//! is open air and drains whatever sits on it, and a pocket that trunks and
//! a model close only within one layer drains through the open air below
//! it. At plate level, an air region of layer 0 whose outer wall is at least
//! half support (a hole in a raft, or among support feet fused on the plate)
//! counts as open air too: those feet are cured onto the plate with the
//! longest exposure and come off with the supports, and the raft pulls on
//! the film over far more area than the hole does. A recess under a model
//! standing on the plate is walled by the model and stays a cup.
//!
//! # What is not reported
//!
//! Real plates hold many cavities too small to matter: on a 1.9-million
//! triangle miniature with generated supports, over a thousand, nearly all
//! one or two pixels on a single layer, slivers where support meshes merge.
//! A cavity is reported only when it is at least
//! [`drain_px`](TrapOptions::drain_px) pixels wide on some layer (a narrower
//! void is a hairline that light bleed cures shut, and could not connect to
//! anything anyway), and, for a trap, holds
//! at least [`min_trap_volume_mm3`](TrapOptions::min_trap_volume_mm3) or,
//! for a cup, is at least [`min_cup_area_mm2`](TrapOptions::min_cup_area_mm2)
//! wide.
//!
//! # What a finding says
//!
//! One finding per cavity, over its layer range, with its volume (the air
//! pixels of every layer times the pixel area and that layer's height) and
//! its largest cross-section. The camera centres on the cavity's floor: the
//! pixel nearest the centroid of the largest air region on its lowest layer.
//! The suggested fix is a drain (for a cup, a vent) at that point, at the
//! bottom of the lowest layer: the floor of the cavity, the side facing the
//! plate, where the resin collects when the part stands on its base and
//! where a hole drilled toward the plate side lets it out and the air in.
//! No drain is suggested for a cavity on layer 0, whose floor is the plate,
//! or for one walled by supports alone, which a drain in the model cannot
//! reach.
//!
//! The scope (model, support or both) comes from the walls: the solid pixels
//! beside the cavity on each pixel row, at its lowest layer, its widest
//! layer and its highest layer, read from those layers' model and support
//! rasters.

use rayon::prelude::*;

use crate::finding::{CheckKind, Finding, PlatePoint, RoleScope, Severity, SuggestedFix};
use crate::input::ReviewInput;
use crate::regions::{LayerPixels, LayerRegions, PlateRegions, Region};
use crate::runner::ReviewContext;
use crate::runs::Run;
use crate::union_find::{Groups, RegionRef};

/// Default of [`TrapOptions::drain_px`].
pub const DEFAULT_DRAIN_PX: u64 = 10;

/// Default of [`TrapOptions::min_trap_volume_mm3`].
pub const DEFAULT_MIN_TRAP_VOLUME_MM3: f64 = 0.01;

/// Default of [`TrapOptions::min_cup_area_mm2`].
pub const DEFAULT_MIN_CUP_AREA_MM2: f64 = 1.0;

/// Default of [`TrapOptions::error_volume_mm3`].
pub const DEFAULT_ERROR_VOLUME_MM3: f64 = 1.0;

/// Settings of the resin-trap and suction-cup check.
#[derive(Clone, Debug, PartialEq)]
pub struct TrapOptions {
    /// The narrowest link, in pixels shared straight above one another,
    /// through which the air of one layer drains into, or vents to, the air
    /// of the next. 10 pixels is about 0.005 mm² on a 19 x 24 µm screen (an
    /// opening some 70 µm wide): anything narrower is a pinhole that thick
    /// resin barely flows through and that light bleed tends to close. A
    /// cavity narrower than this on every layer is such a hairline void
    /// itself and is not reported either.
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
    /// Resin traps at least this large, in cubic millimetres, are errors;
    /// smaller ones are warnings. A cubic millimetre of liquid resin sealed
    /// in a part is enough to seep out or crack it later. Suction cups are
    /// always warnings: they stress the print but do not ruin the part.
    pub error_volume_mm3: f64,
}

impl Default for TrapOptions {
    fn default() -> Self {
        Self {
            drain_px: DEFAULT_DRAIN_PX,
            min_trap_volume_mm3: DEFAULT_MIN_TRAP_VOLUME_MM3,
            min_cup_area_mm2: DEFAULT_MIN_CUP_AREA_MM2,
            error_volume_mm3: DEFAULT_ERROR_VOLUME_MM3,
        }
    }
}

/// Finds the resin traps and suction cups of the plate.
pub fn run(ctx: &ReviewContext<'_>, options: &TrapOptions) -> Vec<Finding> {
    let regions = ctx.regions;
    if regions.layers().is_empty() {
        return Vec::new();
    }
    let drained = drain_pass(ctx.input, regions, options.drain_px);
    let (groups, vented) = vent_pass(regions, &drained, options.drain_px);
    let cavities: Vec<Cavity<'_>> = (0..groups.len())
        .filter(|&group| {
            let lowest = groups.members(group)[0];
            !drained[lowest.layer][lowest.index as usize]
        })
        .map(|group| Cavity::measure(ctx.input, regions, groups.members(group), vented[group]))
        .filter(|cavity| cavity.is_reported(options))
        .collect();
    let scopes = wall_scopes(ctx.input, &cavities);
    cavities
        .iter()
        .zip(scopes)
        .map(|(cavity, scope)| cavity.finding(ctx.input, scope, options))
        .collect()
}

/// Pass 1, from the plate up: which air regions of every layer are drained.
fn drain_pass(input: &ReviewInput<'_>, regions: &PlateRegions, drain_px: u64) -> Vec<Vec<bool>> {
    let mut drained: Vec<Vec<bool>> = Vec::with_capacity(regions.layers().len());
    for (layer, layer_regions) in regions.layers().iter().enumerate() {
        let mut flags: Vec<bool> = if layer == 0 {
            plate_level_open_air(input, layer_regions)
        } else {
            layer_regions
                .air
                .iter()
                .map(|region| region.touches_border)
                .collect()
        };
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

/// Which air regions of layer 0 are open air: those that reach the screen
/// edge, and those whose outer wall is at least half support (a hole in a
/// raft, or among support feet fused on the plate). Layer 0 is rasterised
/// again only when it holds an enclosed air region.
fn plate_level_open_air(input: &ReviewInput<'_>, layer: &LayerRegions) -> Vec<bool> {
    let mut open: Vec<bool> = layer
        .air
        .iter()
        .map(|region| region.touches_border)
        .collect();
    if open.iter().all(|&is_open| is_open) {
        return open;
    }
    let pixels = LayerPixels::compute(input, 0);
    for (is_open, wall) in open.iter_mut().zip(outer_walls(&pixels)) {
        if !*is_open && wall.support_px > 0 && 2 * wall.support_px >= wall.wall_px {
            *is_open = true;
        }
    }
    open
}

/// The roles of the outer wall of every air region of a layer: the solid
/// pixel left of the region's first run and right of its last run on each
/// row. Inside an enclosed region, an island is flanked by the region's own
/// air on both sides, so these pixels belong to the wall around it. Regions
/// that reach the screen edge lose their wall pixels there.
fn outer_walls(pixels: &LayerPixels) -> Vec<WallRoles> {
    let mut walls = vec![WallRoles::default(); pixels.air_count];
    // The rightmost run seen so far of each region on its latest row.
    let mut last: Vec<Option<Run>> = vec![None; pixels.air_count];
    for (run, &label) in pixels.air.iter().zip(&pixels.air_labels) {
        let region = label as usize;
        match last[region] {
            Some(previous) if previous.row == run.row => {}
            Some(previous) => {
                walls[region].add(pixels, previous.row, Some(previous.x1));
                walls[region].add(pixels, run.row, run.x0.checked_sub(1));
            }
            None => walls[region].add(pixels, run.row, run.x0.checked_sub(1)),
        }
        last[region] = Some(*run);
    }
    for (wall, previous) in walls.iter_mut().zip(last) {
        if let Some(previous) = previous {
            wall.add(pixels, previous.row, Some(previous.x1));
        }
    }
    walls
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
    /// Its widest layer (the lowest of equals).
    largest_layer: usize,
    /// Where it is aimed at: the inside pixel of the largest region of its
    /// lowest layer, in raster space.
    floor_px: [f64; 2],
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
        let (mut largest_px, mut largest_layer) = (0u64, first_layer);
        let mut floor: Option<&Region> = None;
        for layer_members in members.chunk_by(|a, b| a.layer == b.layer) {
            let layer = layer_members[0].layer;
            let air = &regions.layer(layer).air;
            let area_px: u64 = layer_members
                .iter()
                .map(|member| air[member.index as usize].area_px)
                .sum();
            volume_mm3 += area_px as f64 * pixel_mm2 * input.layers()[layer].height_mm();
            if area_px > largest_px {
                (largest_px, largest_layer) = (area_px, layer);
            }
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
            largest_section_mm2: largest_px as f64 * pixel_mm2,
            largest_layer,
            floor_px: floor.map_or([0.0, 0.0], Region::inside_point),
        }
    }

    fn first_layer(&self) -> usize {
        self.members[0].layer
    }

    fn last_layer(&self) -> usize {
        self.members[self.members.len() - 1].layer
    }

    /// The layers whose walls decide the scope.
    fn wall_layers(&self) -> Vec<usize> {
        let mut layers = vec![self.first_layer(), self.largest_layer, self.last_layer()];
        layers.sort_unstable();
        layers.dedup();
        layers
    }

    /// Whether the cavity is worth a finding: wider than a pinhole on at
    /// least one layer, and a trap holding enough resin or a cup pulling on
    /// enough film.
    fn is_reported(&self, options: &TrapOptions) -> bool {
        self.largest_px >= options.drain_px
            && match self.kind() {
                CheckKind::ResinTrap => self.volume_mm3 >= options.min_trap_volume_mm3,
                _ => self.largest_section_mm2 >= options.min_cup_area_mm2,
            }
    }

    fn kind(&self) -> CheckKind {
        if self.vented || self.first_layer() == 0 {
            CheckKind::SuctionCup
        } else {
            CheckKind::ResinTrap
        }
    }

    fn finding(&self, input: &ReviewInput<'_>, scope: RoleScope, options: &TrapOptions) -> Finding {
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
        let (severity, title, detail) = match kind {
            CheckKind::ResinTrap => (
                if self.volume_mm3 >= options.error_volume_mm3 {
                    Severity::Error
                } else {
                    Severity::Warning
                },
                "Sealed cavity traps resin",
                format!(
                    "A sealed cavity of {volume} mm³ over {span}, up to {section} mm² wide, has no opening of {} pixels or more, so its liquid resin stays inside.",
                    options.drain_px
                ),
            ),
            _ if first == 0 => (
                Severity::Warning,
                "Cavity sealed by the build plate",
                format!(
                    "A cavity of {volume} mm³ over {span} is closed by the build plate below, so up to {section} mm² of it pulls on the film at every peel."
                ),
            ),
            _ => (
                Severity::Warning,
                "Cavity closed toward the plate",
                format!(
                    "A cavity of {volume} mm³ over {span} is closed toward the plate and opens only above, so up to {section} mm² of it pulls on the film at every peel."
                ),
            ),
        };
        let focus = input.focus(first, self.floor_px);
        let finding = Finding::new(kind, severity, first, last, focus, scope, title, detail);
        if first == 0 || scope == RoleScope::Support {
            finding
        } else {
            finding.with_fix(SuggestedFix::AddDrain {
                at: PlatePoint::new(focus.x_mm, focus.y_mm, layers[first].bottom_mm),
            })
        }
    }
}

/// The scope of every cavity, from the roles of its wall pixels on its
/// lowest, widest and highest layers. Each of those layers is rasterised
/// once, in parallel.
fn wall_scopes(input: &ReviewInput<'_>, cavities: &[Cavity<'_>]) -> Vec<RoleScope> {
    // (layer, air region, cavity) of every region whose walls are read.
    let mut samples: Vec<(usize, u32, usize)> = Vec::new();
    for (id, cavity) in cavities.iter().enumerate() {
        let layers = cavity.wall_layers();
        samples.extend(
            cavity
                .members
                .iter()
                .filter(|member| layers.contains(&member.layer))
                .map(|member| (member.layer, member.index, id)),
        );
    }
    samples.sort_unstable();
    let chunks: Vec<&[(usize, u32, usize)]> = samples.chunk_by(|a, b| a.0 == b.0).collect();
    let counts: Vec<Vec<(usize, WallRoles)>> = chunks
        .par_iter()
        .map(|chunk| wall_roles(&LayerPixels::compute(input, chunk[0].0), chunk))
        .collect();
    let mut totals = vec![WallRoles::default(); cavities.len()];
    for (id, roles) in counts.into_iter().flatten() {
        totals[id].wall_px += roles.wall_px;
        totals[id].model_px += roles.model_px;
        totals[id].support_px += roles.support_px;
    }
    totals
        .into_iter()
        .map(|roles| RoleScope::from_counts(roles.model_px, roles.support_px))
        .collect()
}

/// Wall pixels by role. A pixel can be both model and support.
#[derive(Clone, Copy, Debug, Default)]
struct WallRoles {
    /// Wall pixels counted.
    wall_px: u64,
    /// Those that are model.
    model_px: u64,
    /// Those that are support.
    support_px: u64,
}

impl WallRoles {
    /// Counts the wall pixel `(x, row)` of `pixels`, if `x` is on the
    /// screen.
    fn add(&mut self, pixels: &LayerPixels, row: u32, x: Option<u32>) {
        let Some(x) = x else {
            return;
        };
        let model = covers(&pixels.model, row, x);
        let support = covers(&pixels.support, row, x);
        if model || support {
            self.wall_px += 1;
            self.model_px += u64::from(model);
            self.support_px += u64::from(support);
        }
    }
}

/// The roles of the solid pixels right of and left of every run of the
/// sampled air regions of one layer. `samples` holds `(layer, region,
/// cavity)`, sorted by region.
fn wall_roles(pixels: &LayerPixels, samples: &[(usize, u32, usize)]) -> Vec<(usize, WallRoles)> {
    let mut found: Vec<(usize, WallRoles)> = Vec::new();
    for (run, &label) in pixels.air.iter().zip(&pixels.air_labels) {
        let Ok(at) = samples.binary_search_by_key(&label, |sample| sample.1) else {
            continue;
        };
        let cavity = samples[at].2;
        let entry = match found.iter().position(|(id, _)| *id == cavity) {
            Some(position) => &mut found[position].1,
            None => {
                found.push((cavity, WallRoles::default()));
                &mut found.last_mut().expect("just pushed").1
            }
        };
        entry.add(pixels, run.row, run.x0.checked_sub(1));
        entry.add(pixels, run.row, Some(run.x1));
    }
    found
}

/// Whether the run list `runs` holds pixel `(x, row)`.
fn covers(runs: &[Run], row: u32, x: u32) -> bool {
    let after = runs.partition_point(|run| (run.row, run.x0) <= (row, x));
    after > 0 && {
        let run = runs[after - 1];
        run.row == row && x < run.x1
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
