/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Parts and islands (#84, #432): pixels that light with nothing under them.
//!
//! Reports [`CheckKind::Island`], [`CheckKind::SurfaceGrowth`] and the
//! [`CheckKind::CleanUp`] notes. It reads only the sweep's tables and the
//! support tips as data ([`crate::tips`]).
//!
//! # Parts
//!
//! A *part* is a stretch of the model from its first layer until it joins
//! something else ([`Part`]). Walking the tables from the plate up, a solid
//! region continues the parts of the regions it stands on, through the links
//! that hold it. A region no link holds starts a part of its own
//! ([`PartStart`](crate::regions::PartStart)): size does not matter, and a
//! neck too fine to hold anything does not count as being held. Where parts
//! meet, the one that is held goes on, the largest if both or neither are,
//! and the others end there.
//!
//! # What holds a part
//!
//! Only its first pixels are judged:
//!
//! - a part that starts on the first [`PLATE_LAYERS`] layers stands on the
//!   build plate;
//! - any other part is held when, on the layer below its first layer, the
//!   pixels of a support tip lie under it, however few: pixels within
//!   [`TIP_REACH_MM`] of the part's own pixels, outside what the model has
//!   on that layer ([`MIN_TIP_SHARE`] can ask for more). Its own pixels
//!   decide, never its box: a tip under the neighbour a part hangs from does
//!   not hold it. Tips are read as data, whatever piece of the support graph
//!   they belong to.
//!
//! # What is done with a part nothing holds
//!
//! - One that lasts fewer than [`MIN_PART_LAYERS`] layers is debris, whatever
//!   its size: it comes loose in the vat. Its pixels are erased from the
//!   layers, with the stray pixels of the sweep, and one note reports them.
//!   A part that a tip too thin to hold it touches stays: erasing it would
//!   leave the tip short of what comes above.
//! - Any other is an island, an error with a [`SuggestedFix::AddSupport`] at
//!   its first pixels. Its [`Island`] tells where to place supports: its
//!   first pixels, its area layer by layer, and where it joins the rest.
//!
//! # Surface growth
//!
//! A region at least [`GROWTH_FACTOR`] times as large as what stood under it
//! is listed on its own, held or not: the peel force of the new surface
//! comes on all at once. The pixels counted are lit pixels, so it counts from
//! the first pixel that exists.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::fill::CavityFill;
use crate::finding::{
    CheckKind, Finding, MAX_FOOTPRINT_LAYERS, PlatePoint, RoleScope, Severity, SuggestedFix,
};
use crate::input::ReviewInput;
use crate::regions::{
    LayerRead, PartStart, PlateRegions, Region, SweepOptions, WantedRegions, link_holds,
    read_layers, runs_in, wanted,
};
use crate::runner::ReviewContext;
use crate::runs::{Run, dilate_square, union};
use crate::tip_plan::{IslandLayer, IslandPixels};
use crate::tips::{TipContact, TipIndex, TipTarget};
use crate::union_find::RegionRef;

/// The layers that rest on the build plate: 2. Nothing that starts on them
/// is an island.
///
/// Layer 0 lies on the plate. A pixel of layer 1 with nothing below it hangs
/// one layer above the plate, and the long exposure of the first layers cures
/// through that gap, so it bonds to the plate as well; no support would fit
/// there either.
pub const PLATE_LAYERS: usize = 2;

/// The layers a part nothing holds must last to be an island: 3. A shorter
/// one is debris that comes loose in the vat, whatever its size.
pub const MIN_PART_LAYERS: usize = 3;

/// How many times what stood under it a region must be for its growth to be
/// listed: 10.
pub const GROWTH_FACTOR: f64 = 10.0;

/// The share of its section a tip must reach a part with to hold it: 0, any
/// pixel of a tip under the part's own pixels holds it, however few. A cone
/// ends in the middle of a layer, so how much of it shows on the layer right
/// below says where the cut fell, not whether a support is there.
///
/// Measured on real scenes, the islands listed with 0 and with a half: a
/// 171 mm hollowed model, 1 and 8; three well-supported minis, 29 and 175.
/// Go back to 0.5 if prints show that the last sliver of a tip does not hold
/// what starts on it: a tip then has to reach a part with half its section,
/// or a part smaller than the tip has to stand with half of itself on tips.
pub const MIN_TIP_SHARE: f64 = 0.0;

/// How far from a part's pixels a tip's pixels on the layer below still
/// touch it, in millimetres: 0.07, a tip beside a part's edge cures into it.
pub const TIP_REACH_MM: f64 = 0.07;

/// How close the ends of two cones are to be the same tip, in millimetres: a
/// neck and the cone above it.
const SAME_TIP_MM: f64 = 0.05;

/// Settings of the island check.
#[derive(Clone, Debug, PartialEq)]
pub struct IslandOptions {
    /// The layers a part nothing holds must last to be an island. Default
    /// [`MIN_PART_LAYERS`].
    pub min_part_layers: usize,
    /// The growth a region must reach to be listed. Default
    /// [`GROWTH_FACTOR`].
    pub growth_factor: f64,
    /// The share of its section a tip must reach a part with to hold it.
    /// Default [`MIN_TIP_SHARE`]; 0 lets any pixel of a tip hold it.
    pub min_tip_share: f64,
    /// How far from a part's pixels a tip's still touch it, in millimetres.
    /// Default [`TIP_REACH_MM`].
    pub tip_reach_mm: f64,
}

impl Default for IslandOptions {
    fn default() -> Self {
        Self {
            min_part_layers: MIN_PART_LAYERS,
            growth_factor: GROWTH_FACTOR,
            min_tip_share: MIN_TIP_SHARE,
            tip_reach_mm: TIP_REACH_MM,
        }
    }
}

/// What holds a part's first pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Held {
    /// The build plate.
    Plate,
    /// A support tip under them.
    Tip,
    /// Nothing.
    Nothing,
}

/// One part of the model: from its first layer until it joins something
/// else or ends.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    /// The layer it starts on.
    pub first_layer: usize,
    /// The solid region it starts with, on that layer.
    pub start: u32,
    /// What holds it.
    pub held: Held,
    /// The pixels of tips it touches on the layer below its first, whether
    /// they hold it or not.
    pub tip_contact_px: usize,
    /// Its pixels on each layer from its first, until it joins something.
    pub area_px: Vec<u64>,
    /// The layer it joins another part on and that part's index, when it
    /// does.
    pub joined: Option<(usize, usize)>,
    /// Its regions on its first [`MAX_FOOTPRINT_LAYERS`] layers.
    pub members: Vec<RegionRef>,
}

impl Part {
    /// The layers it lasts on its own.
    pub fn layers(&self) -> usize {
        self.area_px.len()
    }

    /// The last layer it is on, before it joins anything.
    pub fn last_layer(&self) -> usize {
        self.first_layer + self.area_px.len().max(1) - 1
    }

    /// Its largest section, in pixels.
    pub fn largest_px(&self) -> u64 {
        self.area_px.iter().copied().max().unwrap_or(0)
    }

    fn add(&mut self, layer: usize, index: u32, area_px: u64) {
        let at = layer - self.first_layer;
        if at >= self.area_px.len() {
            self.area_px.resize(at + 1, 0);
        }
        self.area_px[at] += area_px;
        if at < MAX_FOOTPRINT_LAYERS {
            self.members.push(RegionRef { layer, index });
        }
    }
}

const NO_PART: u32 = u32::MAX;

/// The parts of the plate, in the order they start, and what they became.
fn walk_parts(
    input: &ReviewInput<'_>,
    regions: &PlateRegions,
    tips: &TipIndex,
    options: &IslandOptions,
    sweep: &SweepOptions,
) -> Vec<Part> {
    let min_link_px = sweep.min_link_px(input);
    let [size_x, size_y] = input.pixel_size_mm();
    let reach_px = [size_x, size_y].map(|size| (options.tip_reach_mm / size).round() as u32);
    let contact_of = |layer: usize, start: &PartStart, region: &Region| {
        let target = TipTarget {
            runs: &start.runs,
            area_px: region.area_px,
            bbox: region.bbox,
            below: &start.below,
        };
        tips.contact(input, layer - 1, &target, reach_px, options.min_tip_share)
    };
    let mut parts: Vec<Part> = Vec::new();
    // The part each part went on as, itself while it goes on.
    let mut went_on: Vec<u32> = Vec::new();
    fn root(went_on: &mut [u32], mut part: u32) -> u32 {
        while went_on[part as usize] != part {
            let next = went_on[went_on[part as usize] as usize];
            went_on[part as usize] = next;
            part = next;
        }
        part
    }
    let mut below: Vec<u32> = Vec::new();
    for (layer, table) in regions.layers().iter().enumerate() {
        let mut by_upper: Vec<(u32, u32, u64)> = regions
            .solid_links(layer)
            .iter()
            .map(|link| (link.upper, link.lower, link.pixels))
            .collect();
        by_upper.sort_unstable();
        let mut here = vec![NO_PART; table.solid.len()];
        let mut at = 0;
        for (index, region) in table.solid.iter().enumerate() {
            let index = index as u32;
            let from = at;
            while at < by_upper.len() && by_upper[at].0 == index {
                at += 1;
            }
            let links = &by_upper[from..at];
            let holding = if layer > 0 && table.start_of(index).is_none() {
                holding_links(links, region, min_link_px)
            } else {
                Vec::new()
            };
            if holding.is_empty() {
                let start = table.start_of(index);
                let contact = match start {
                    Some(start) if layer >= PLATE_LAYERS => contact_of(layer, start, region),
                    _ => TipContact {
                        px: 0,
                        holds: false,
                    },
                };
                let held = if layer < PLATE_LAYERS {
                    Held::Plate
                } else if contact.holds {
                    Held::Tip
                } else {
                    Held::Nothing
                };
                here[index as usize] = parts.len() as u32;
                went_on.push(parts.len() as u32);
                parts.push(Part {
                    first_layer: layer,
                    start: index,
                    held,
                    tip_contact_px: contact.px,
                    area_px: Vec::new(),
                    joined: None,
                    members: Vec::new(),
                });
                continue;
            }
            let mut standing: Vec<u32> = holding
                .iter()
                .map(|(_, lower, _)| root(&mut went_on, below[*lower as usize]))
                .collect();
            standing.sort_unstable();
            standing.dedup();
            let keep = *standing
                .iter()
                .max_by_key(|&&p| {
                    let part = &parts[p as usize];
                    (
                        part.held != Held::Nothing,
                        part.area_px.last().copied().unwrap_or(0),
                        std::cmp::Reverse(p),
                    )
                })
                .expect("a region held by something");
            for &other in standing.iter().filter(|&&p| p != keep) {
                parts[other as usize].joined = Some((layer, keep as usize));
                went_on[other as usize] = keep;
            }
            here[index as usize] = keep;
        }
        for (index, region) in table.solid.iter().enumerate() {
            let part = root(&mut went_on, here[index]);
            here[index] = part;
            parts[part as usize].add(layer, index as u32, region.area_px);
        }
        below = here;
    }
    parts
}

/// The links of `links` that hold `region`: those that do alone, or all of
/// them when they only do together.
fn holding_links<'l>(
    links: &'l [(u32, u32, u64)],
    region: &Region,
    min_link_px: u64,
) -> Vec<&'l (u32, u32, u64)> {
    let strong: Vec<_> = links
        .iter()
        .filter(|(_, _, px)| link_holds(*px, region.area_px, min_link_px))
        .collect();
    if strong.is_empty() {
        links.iter().collect()
    } else {
        strong
    }
}

/// An island, as a placement of supports needs it.
#[derive(Clone, Debug, PartialEq)]
pub struct Island {
    /// The layer it starts on.
    pub first_layer: usize,
    /// The centres of its first pixels, in plate millimetres.
    pub first_pixels_mm: Vec<[f64; 2]>,
    /// The area of its first pixels, in square millimetres.
    pub first_area_mm2: f64,
    /// Its area on each layer from its first, in square millimetres, until
    /// it joins the rest of the model.
    pub growth_mm2: Vec<f64>,
    /// The layer it joins the rest of the model on, `None` when it never
    /// does.
    pub joins_on: Option<usize>,
    /// Where its finding's [`SuggestedFix::AddSupport`] points: what names
    /// this island among the findings.
    pub at: PlatePoint,
    /// Its pixels, layer by layer.
    pub pixels: Arc<IslandPixels>,
    /// The tips that stand in it already, higher than its first pixels, in
    /// plate millimetres.
    pub tips_mm: Vec<[f64; 3]>,
}

/// What the check gives: the findings, what they found and what to erase.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IslandReport {
    /// Every finding: islands, surface growth and the clean-up notes.
    pub findings: Vec<Finding>,
    /// Every island of `findings`, in the same order, whichever findings a
    /// raft explains later.
    pub islands: Vec<Island>,
    /// Every part of the plate.
    pub parts: Vec<Part>,
    /// The pixels that leave the layers: every stray pixel and all of the
    /// debris, as rectangles on each layer.
    pub erased: CavityFill,
}

/// Finds the parts and islands of the plate.
pub fn run(ctx: &ReviewContext<'_>, options: &IslandOptions, sweep: &SweepOptions) -> IslandReport {
    let (input, regions) = (ctx.input, ctx.regions);
    if input.layer_count() == 0 {
        return IslandReport::default();
    }
    let tips = TipIndex::new(input.tips());
    let parts = walk_parts(input, regions, &tips, options, sweep);
    let islands: Vec<&Part> = parts
        .iter()
        .filter(|part| part.held == Held::Nothing && part.layers() >= options.min_part_layers)
        .collect();
    let debris: Vec<&Part> = parts
        .iter()
        .filter(|part| is_debris(part, options))
        .collect();
    // Who asks for pixels: each island, for its footprint, then the debris.
    let mut wants: Vec<WantedRegions> = Vec::new();
    for (asker, part) in islands.iter().enumerate() {
        wants.extend(part.members.iter().map(|member| wanted(member, asker)));
    }
    let debris_asker = islands.len();
    for part in &debris {
        wants.extend(part.members.iter().map(|m| wanted(m, debris_asker)));
    }
    let askers = islands.len() + 1;
    let pixels = read_layers(
        input,
        sweep,
        LayerRead::Solid,
        wants,
        askers,
        |pixels, regions| runs_in(&pixels.solid, &pixels.solid_labels, regions),
    );
    let mut findings = Vec::new();
    let mut reported = Vec::with_capacity(islands.len());
    for (part, runs) in islands.iter().zip(pixels.iter()) {
        let footprint = CavityFill::from_runs(input, runs.clone());
        let finding = island_finding(input, regions, part).with_footprint(Arc::new(footprint));
        reported.push(island_of(input, regions, part, &finding, runs, &tips));
        findings.push(finding);
    }
    findings.extend(growth_findings(ctx, options, &parts));
    let debris_runs = pixels.last().cloned().unwrap_or_default();
    findings.extend(debris_note(input, regions, &debris));
    findings.extend(stray_note(input, regions));
    let erased = erased_pixels(input, regions, debris_runs);
    IslandReport {
        findings,
        islands: reported,
        parts,
        erased,
    }
}

/// Whether `part` comes loose: it starts on nothing, not even a tip too thin
/// to hold it, and lasts too little to be an island. What stands on a tip is
/// never erased: the tip would stop short of what comes above.
fn is_debris(part: &Part, options: &IslandOptions) -> bool {
    part.held == Held::Nothing
        && part.tip_contact_px == 0
        && part.layers() < options.min_part_layers
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// The finding of island `part`.
fn island_finding(input: &ReviewInput<'_>, regions: &PlateRegions, part: &Part) -> Finding {
    let pixel_mm2 = input.pixel_area_mm2();
    let start = regions.layer(part.first_layer).solid[part.start as usize];
    let focus = input.focus(part.first_layer, start.inside_point());
    let first_mm2 = start.area_px as f64 * pixel_mm2;
    let largest_mm2 = part.largest_px() as f64 * pixel_mm2;
    let last = part.last_layer();
    let span = plural(part.layers(), "layer", "layers");
    let contact_mm2 = part.tip_contact_px as f64 * pixel_mm2;
    let under = if part.tip_contact_px == 0 {
        "nothing under it".to_owned()
    } else {
        format!("only {contact_mm2:.3} mm² of a support tip under it")
    };
    let detail = match part.joined {
        Some(_) if largest_mm2 > first_mm2 => format!(
            "{first_mm2:.3} mm² of model starts with {under} and grows to {largest_mm2:.3} mm² over {span} before it joins the rest."
        ),
        Some(_) => format!(
            "{first_mm2:.3} mm² of model starts with {under} and lasts {span} before it joins the rest."
        ),
        None => format!(
            "{first_mm2:.3} mm² of model starts with {under}, reaches {largest_mm2:.3} mm² over {span} and never joins anything."
        ),
    };
    Finding::new(
        CheckKind::Island,
        Severity::Error,
        part.first_layer,
        last,
        focus,
        RoleScope::Model,
        "Unsupported island",
        detail,
    )
    .with_fix(SuggestedFix::AddSupport {
        at: input.underside(part.first_layer, focus),
    })
    .with_largest_mm2(largest_mm2)
}

/// Island `part`, found as `finding` and of the pixels `layers`, as a
/// placement of supports needs it.
fn island_of(
    input: &ReviewInput<'_>,
    regions: &PlateRegions,
    part: &Part,
    finding: &Finding,
    layers: &[(usize, Vec<Run>)],
    tips: &TipIndex,
) -> Island {
    let pixel_mm2 = input.pixel_area_mm2();
    let table = regions.layer(part.first_layer);
    let runs = table
        .start_of(part.start)
        .map_or(&[][..], |start| start.runs.as_slice());
    let first_pixels_mm = runs
        .iter()
        .flat_map(|run| {
            (run.x0..run.x1)
                .map(|x| input.pixel_to_plate([f64::from(x) + 0.5, f64::from(run.row) + 0.5]))
        })
        .collect();
    let pixels = island_pixels(input, layers);
    let at = match finding.fix {
        Some(SuggestedFix::AddSupport { at }) => at,
        _ => finding.focus,
    };
    Island {
        first_layer: part.first_layer,
        first_pixels_mm,
        first_area_mm2: table.solid[part.start as usize].area_px as f64 * pixel_mm2,
        growth_mm2: part
            .area_px
            .iter()
            .map(|px| *px as f64 * pixel_mm2)
            .collect(),
        joins_on: part.joined.map(|(layer, _)| layer),
        at,
        tips_mm: tips.standing_in(input, &pixels, SAME_TIP_MM),
        pixels: Arc::new(pixels),
    }
}

/// The pixels of an island, with how raster pixels land on the plate.
fn island_pixels(input: &ReviewInput<'_>, layers: &[(usize, Vec<Run>)]) -> IslandPixels {
    let origin_mm = input.pixel_to_plate([0.0, 0.0]);
    let next = input.pixel_to_plate([1.0, 1.0]);
    IslandPixels {
        origin_mm,
        step_mm: [next[0] - origin_mm[0], next[1] - origin_mm[1]],
        layers: layers
            .iter()
            .map(|(layer, runs)| IslandLayer {
                layer: *layer,
                bottom_mm: input.layers()[*layer].bottom_mm,
                top_mm: input.layers()[*layer].top_mm,
                runs: runs.clone(),
            })
            .collect(),
    }
}

/// One finding per region at least `growth_factor` times what stood under
/// it, leaving out what the list already tells: debris, which leaves the
/// layers, and the first pixels of an island.
fn growth_findings(
    ctx: &ReviewContext<'_>,
    options: &IslandOptions,
    parts: &[Part],
) -> Vec<Finding> {
    let (input, regions) = (ctx.input, ctx.regions);
    let pixel_mm2 = input.pixel_area_mm2();
    let mut left_out: Vec<RegionRef> = parts
        .iter()
        .filter(|part| is_debris(part, options))
        .flat_map(|part| part.members.iter().copied())
        .chain(
            parts
                .iter()
                .filter(|part| part.held == Held::Nothing)
                .map(|part| RegionRef {
                    layer: part.first_layer,
                    index: part.start,
                }),
        )
        .collect();
    left_out.sort_unstable();
    let is_left_out =
        |layer: usize, index: u32| left_out.binary_search(&RegionRef { layer, index }).is_ok();
    let mut findings = Vec::new();
    for layer in 1..regions.layers().len() {
        let table = regions.layer(layer);
        let lower = regions.layer(layer - 1);
        let mut links: Vec<(u32, u32)> = regions
            .solid_links(layer)
            .iter()
            .map(|link| (link.upper, link.lower))
            .collect();
        links.sort_unstable();
        for stood in links.chunk_by(|a, b| a.0 == b.0) {
            let index = stood[0].0;
            let region = &table.solid[index as usize];
            let from_px: u64 = stood
                .iter()
                .filter(|(_, below)| !is_left_out(layer - 1, *below))
                .map(|(_, below)| lower.solid[*below as usize].area_px)
                .sum();
            if from_px == 0
                || (region.area_px as f64) < options.growth_factor * from_px as f64
                || is_left_out(layer, index)
            {
                continue;
            }
            let (from_mm2, to_mm2) = (
                from_px as f64 * pixel_mm2,
                region.area_px as f64 * pixel_mm2,
            );
            let focus = input.focus(layer, region.inside_point());
            findings.push(
                Finding::new(
                    CheckKind::SurfaceGrowth,
                    Severity::Warning,
                    layer,
                    layer,
                    focus,
                    RoleScope::Model,
                    "Surface grows at once",
                    format!(
                        "The section grows from {from_mm2:.3} mm² to {to_mm2:.3} mm² on one layer (×{:.0}): it wants more support tips.",
                        region.area_px as f64 / from_px as f64
                    ),
                )
                .with_largest_mm2(to_mm2),
            );
        }
    }
    findings
}

/// The one note about the debris erased from the layers.
fn debris_note(
    input: &ReviewInput<'_>,
    regions: &PlateRegions,
    debris: &[&Part],
) -> Option<Finding> {
    let first = debris.iter().map(|part| part.first_layer).min()?;
    let last = debris.iter().map(|part| part.last_layer()).max()?;
    let lowest = debris.iter().min_by_key(|part| part.first_layer)?;
    let start = regions.layer(lowest.first_layer).solid[lowest.start as usize];
    let largest_mm2 = debris
        .iter()
        .map(|part| part.largest_px())
        .max()
        .map_or(0.0, |px| px as f64 * input.pixel_area_mm2());
    Some(
        Finding::new(
            CheckKind::CleanUp,
            Severity::Info,
            first,
            last,
            input.focus(lowest.first_layer, start.inside_point()),
            RoleScope::Model,
            "Debris removed",
            format!(
                "{} that started on nothing and lasted under {MIN_PART_LAYERS} layers removed on layers {}–{}, the largest {largest_mm2:.3} mm²: they would come loose in the vat.",
                plural(debris.len(), "part", "parts"),
                first + 1,
                last + 1,
            ),
        )
        .with_largest_mm2(largest_mm2),
    )
}

/// The one note about the stray pixels taken off the layers.
fn stray_note(input: &ReviewInput<'_>, regions: &PlateRegions) -> Option<Finding> {
    let layers = regions.layers();
    let first = layers.iter().position(|layer| !layer.stray.is_empty())?;
    let last = layers.iter().rposition(|layer| !layer.stray.is_empty())?;
    let pixels: u64 = layers
        .iter()
        .flat_map(|layer| &layer.stray)
        .map(|run| u64::from(run.len()))
        .sum();
    let run = layers[first].stray[0];
    Some(Finding::new(
        CheckKind::CleanUp,
        Severity::Info,
        first,
        last,
        input.focus(first, [f64::from(run.x0) + 0.5, f64::from(run.row) + 0.5]),
        RoleScope::Model,
        "Stray pixels removed",
        format!(
            "{} removed on layers {}–{}: walls, pins and edges thinner than a pixel, which do not print.",
            plural(pixels as usize, "stray pixel", "stray pixels"),
            first + 1,
            last + 1,
        ),
    ))
}

/// The pixels that leave the layers: the sweep's stray pixels and `debris`,
/// the layers of the debris with their runs. Debris goes with a pixel around
/// it: it stands alone on its layer, so that pixel is air or stray, and it
/// takes the part of its polygons too thin to light a pixel, which an
/// anti-aliased layer would still grey. Each layer's pixels are one list, so
/// no rectangle of it overlaps another: a ring would be cancelled twice.
fn erased_pixels(
    input: &ReviewInput<'_>,
    regions: &PlateRegions,
    debris: Vec<(usize, Vec<Run>)>,
) -> CavityFill {
    let (width_px, height_px) = (input.width_px(), input.height_px());
    let mut by_layer: BTreeMap<usize, Vec<Run>> = regions
        .layers()
        .iter()
        .enumerate()
        .filter(|(_, table)| !table.stray.is_empty())
        .map(|(layer, table)| (layer, table.stray.clone()))
        .collect();
    for (layer, runs) in debris {
        let grown = dilate_square(&runs, 1, width_px, height_px);
        let layer_runs = by_layer.entry(layer).or_default();
        *layer_runs = union(layer_runs, &grown);
    }
    CavityFill::from_runs(input, by_layer.into_iter().collect())
}

#[cfg(test)]
#[path = "tests_islands.rs"]
mod tests;
