/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The peel loads of a plate's models (#240): what each solid region pulls
//! on the film with, and the suction cups sealed against it.
//!
//! It reads only the sweep's tables, no pixel: each solid region pulls with
//! its area ([`bread_peel::PeelModel::region_force_n`]), and with the suction of
//! the cavities it closes. The shape term is left out here: the tables keep
//! no perimeter per region, and it is 0 until prints measure it.
//!
//! # Suction cups
//!
//! An air region enclosed within its layer whose air does not drain towards
//! the plate (the drain pass of [`crate::checks::traps`]) is sealed by the
//! film at that layer's peel: resin cannot flow in, and the cavity sucks on
//! the film with its whole section. That holds for every cavity while it is
//! printed, a cup or a trap still open above. Its suction goes to the solid
//! region around it: the smallest whose box holds the cavity's.
//!
//! Supports are not in the tables (#432): what they pull with reaches the
//! raft through themselves, not the model.
//!
//! # Who takes the pull: bodies and tips (#241)
//!
//! On each layer, a *body* is the model printed so far and connected: the
//! solid regions of this layer and of every layer below, joined through
//! their links (a union-find walked from the plate up, so two parts that
//! meet higher up are two bodies until then). A body that reaches down to
//! the build plate, or into the raft, is held by it. Any other is held by
//! the support tips that end in it, read as data ([`ReviewInput::tips`]): a
//! tip belongs to the solid region its end lies in, on the layer of its end.
//!
//! The pull of a body's regions on a layer is shared among its tips as on a
//! rigid body ([`bread_peel::TipGroup`]): the mean, plus the leverage of where
//! the pull centres and of its sideways share at the film. What a tip holds
//! is its contact section times the tips' strength
//! ([`bread_peel::PeelModel::tip_capacity_n`]); its *utilisation* is the load
//! over that. For every tip the review keeps its worst layer, and for every
//! body held by tips alone (followed up to where it joins the rest) the layer
//! where its most loaded tip is worst off.

use std::collections::{BTreeSet, HashMap};

use bread_peel::{Band, PeelModel, TipLoadCase, TipSums};
use sc_sl1cer::geom2d::Point2D;

use crate::checks::islands::PLATE_LAYERS;
use crate::checks::traps::DEFAULT_DRAIN_PX;
use crate::input::ReviewInput;
use crate::regions::{PlateRegions, Region};
use crate::runner::ReviewContext;
use crate::union_find::{RegionRef, UnionFind};

/// Settings of the loads.
#[derive(Clone, Debug, PartialEq)]
pub struct LoadOptions {
    /// The peel's coefficients.
    pub model: PeelModel,
    /// The smallest link through which air drains, and the smallest cavity
    /// that sucks, in pixels: as [`TrapOptions::drain_px`](crate::checks::traps::TrapOptions::drain_px).
    pub drain_px: u64,
}

impl Default for LoadOptions {
    fn default() -> Self {
        Self {
            model: PeelModel::default(),
            drain_px: DEFAULT_DRAIN_PX,
        }
    }
}

/// The loads of a plate's models.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlateLoads {
    /// The suction cups sealed against the film on each layer, mm².
    pub cup_mm2: Vec<f64>,
    /// What each layer's models pull with, cups included, N (the middle of
    /// the band).
    pub model_force_n: Vec<f64>,
    /// The suction under each solid region, mm²: `region_cup_mm2[layer]`
    /// lists `(region, mm²)` for the regions that close a cavity.
    pub region_cup_mm2: Vec<Vec<(u32, f64)>>,
    /// Every support tip that ends in the model, with its worst load (#241).
    pub tips: Vec<TipLoad>,
    /// For every body held by tips alone, the layer its most loaded tip is
    /// worst off, by utilisation.
    pub bodies: Vec<BodyPeak>,
    /// Every body and layer with a suction cup: whether tips hold it and how
    /// hard its worst tip is pulled then.
    pub cups: Vec<CupLoad>,
    /// The utilisation of the most loaded tip on each layer: 0 where the
    /// plate holds all that peels (#245).
    pub layer_utilization: Vec<f64>,
    /// Each tip's load along the print (#245), by index in [`Self::tips`]:
    /// the hardest pull of each run of [`Self::curve_layers`] layers, N.
    /// Empty for a tip no peel reaches.
    pub tip_curves: Vec<Vec<f32>>,
    /// How many layers each point of a tip's curve stands for: one up to
    /// [`TIP_CURVE_POINTS`] layers.
    pub curve_layers: usize,
    /// The critical layers (#244): each body's worst and the plate's
    /// hardest, at most [`CASE_LAYERS`], and the first layer of each body
    /// tips hold, at most [`BIRTH_LAYERS`], with what holds what on each.
    pub cases: Vec<LayerCase>,
}

/// The most critical layers kept (#244).
pub const CASE_LAYERS: usize = 12;

/// The most first layers of bodies kept besides (#244): where a body's
/// tips have just taken hold and nothing else helps them yet.
pub const BIRTH_LAYERS: usize = 12;

/// A critical layer as it peels: every body printed so far that tips or
/// feet tie to the supports.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerCase {
    pub layer: usize,
    /// Its top, mm.
    pub z_mm: f64,
    pub bodies: Vec<BodyCase>,
}

/// A body on a critical layer.
#[derive(Clone, Debug, PartialEq)]
pub struct BodyCase {
    /// What the layer pulls it with: 0 when it has no region there.
    pub force_n: Band,
    /// Where the pull centres, plate mm.
    pub at_mm: [f64; 2],
    /// The plate or the raft holds it.
    pub held: bool,
    /// The tips that end in it, by index in [`PlateLoads::tips`].
    pub tips: Vec<usize>,
    /// Where supports stand on it, plate mm.
    pub feet: Vec<[f64; 3]>,
}

/// The most points a tip's curve has (#245): a chart of the print is drawn
/// from as many.
pub const TIP_CURVE_POINTS: usize = 480;

/// A support tip and the hardest pull on it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TipLoad {
    /// Where it ends in the model, plate millimetres.
    pub at_mm: [f64; 3],
    /// The radius of its contact, mm.
    pub radius_mm: f64,
    /// The layer it ends on.
    pub layer: usize,
    /// The hardest pull on it, N: the middle of the band, 0 while no peel
    /// reaches it (a tip in a body the plate holds).
    pub worst_n: f64,
    /// The layer of that pull.
    pub worst_layer: usize,
    /// What it holds, N.
    pub capacity_n: Band,
}

impl TipLoad {
    /// Its worst load over what it holds, both the middle of their bands.
    pub fn utilization(&self) -> f64 {
        if self.capacity_n.mid > 0.0 {
            self.worst_n / self.capacity_n.mid
        } else {
            0.0
        }
    }
}

/// The worst moment of a body held by tips alone.
#[derive(Clone, Debug, PartialEq)]
pub struct BodyPeak {
    /// The layer whose peel loads its tips worst.
    pub layer: usize,
    /// What the body pulls with on that layer, N.
    pub force_n: Band,
    /// Where that pull centres, plate millimetres.
    pub at_mm: [f64; 2],
    /// How many tips hold it then.
    pub tips: u32,
    /// The most loaded of them, by index in [`PlateLoads::tips`].
    pub worst_tip: usize,
    /// Its load then, N.
    pub worst_n: Band,
    /// Its load over what it holds, the middles of both bands.
    pub utilization: f64,
    /// The first pixels of the body's parts, `(layer, region)`: where tips
    /// reach it from below.
    pub starts: Vec<(usize, u32)>,
}

/// A suction cup on a layer, and the body around it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CupLoad {
    /// The layer.
    pub layer: usize,
    /// The centre of the cup's section, plate millimetres.
    pub at_mm: [f64; 2],
    /// Its section, mm².
    pub cup_mm2: f64,
    /// Whether tips alone hold the body: else the plate does, or nothing.
    pub on_tips: bool,
    /// The worst utilisation of the body's tips on this layer, 0 when tips
    /// do not hold it.
    pub utilization: f64,
}

impl PlateLoads {
    /// The point of a tip's curve that layer `layer` falls in.
    pub fn curve_point(&self, layer: usize) -> usize {
        layer / self.curve_layers.max(1)
    }

    /// The load on tip `tip` when layer `layer` peels, N: the hardest of the
    /// run of layers it falls in, 0 when none reaches it.
    pub fn tip_load_at(&self, tip: usize, layer: usize) -> f64 {
        self.tip_curves
            .get(tip)
            .and_then(|curve| curve.get(self.curve_point(layer)))
            .map_or(0.0, |&n| f64::from(n))
    }

    /// The suction cup closed by solid region `region` of layer `layer`,
    /// mm².
    pub fn cup_of(&self, layer: usize, region: u32) -> f64 {
        self.region_cup_mm2
            .get(layer)
            .and_then(|cups| cups.iter().find(|(index, _)| *index == region))
            .map_or(0.0, |(_, mm2)| *mm2)
    }

    /// What solid region `region` of layer `layer` pulls with, N (the middle
    /// of the band), given its table.
    pub fn region_force_n(
        &self,
        ctx: &ReviewContext<'_>,
        model: &PeelModel,
        layer: usize,
        region: u32,
    ) -> f64 {
        let table = &ctx.regions.layer(layer).solid[region as usize];
        let area = table.area_px as f64 * ctx.input.pixel_area_mm2();
        model
            .region_force_n(area, None, self.cup_of(layer, region))
            .mid
    }
}

/// The loads of the plate `ctx` reviews.
pub fn plate_loads(ctx: &ReviewContext<'_>, options: &LoadOptions) -> PlateLoads {
    let (input, regions) = (ctx.input, ctx.regions);
    let pixel_mm2 = input.pixel_area_mm2();
    let drained = drained_air(regions, options.drain_px);
    let count = regions.layers().len();
    let mut loads = PlateLoads {
        cup_mm2: vec![0.0; count],
        model_force_n: vec![0.0; count],
        region_cup_mm2: vec![Vec::new(); count],
        layer_utilization: vec![0.0; count],
        curve_layers: count.div_ceil(TIP_CURVE_POINTS).max(1),
        ..PlateLoads::default()
    };
    // The centre of each region's cups, in pixels, weighted by area.
    let mut centres: Vec<Vec<(u32, [f64; 2])>> = vec![Vec::new(); count];
    for (layer, table) in regions.layers().iter().enumerate() {
        let mut cups: Vec<(u32, f64)> = Vec::new();
        let mut moments: Vec<[f64; 2]> = Vec::new();
        for (air, region) in table.air.iter().enumerate() {
            if drained[layer][air] || region.area_px < options.drain_px {
                continue;
            }
            let Some(around) = enclosing(&table.solid, region) else {
                continue;
            };
            let mm2 = region.area_px as f64 * pixel_mm2;
            let moment = region.centroid_px.map(|v| v * region.area_px as f64);
            match cups.iter().position(|(index, _)| *index == around) {
                Some(at) => {
                    cups[at].1 += mm2;
                    moments[at][0] += moment[0];
                    moments[at][1] += moment[1];
                }
                None => {
                    cups.push((around, mm2));
                    moments.push(moment);
                }
            }
        }
        centres[layer] = cups
            .iter()
            .zip(&moments)
            .map(|((index, mm2), moment)| {
                let px = mm2 / pixel_mm2;
                (*index, [moment[0] / px, moment[1] / px])
            })
            .collect();
        cups.sort_unstable_by_key(|(index, _)| *index);
        loads.cup_mm2[layer] = cups.iter().map(|(_, mm2)| mm2).sum();
        loads.model_force_n[layer] = table
            .solid
            .iter()
            .enumerate()
            .map(|(index, region)| {
                let cup = cups
                    .iter()
                    .find(|(around, _)| *around as usize == index)
                    .map_or(0.0, |(_, mm2)| *mm2);
                options
                    .model
                    .region_force_n(region.area_px as f64 * pixel_mm2, None, cup)
                    .mid
            })
            .sum();
        loads.region_cup_mm2[layer] = cups;
    }
    let walked = loads.clone();
    share(ctx, options, &mut loads, &centres, &BTreeSet::new());
    let layers = critical_layers(&loads);
    loads.cases = share(ctx, options, &mut walked.clone(), &centres, &layers);
    loads
}

/// The layers worth checking as a frame (#244): each body's worst, then the
/// plate's hardest peaks, at most [`CASE_LAYERS`]; and the first layer of
/// each body tips hold, the worst off first, at most [`BIRTH_LAYERS`].
fn critical_layers(loads: &PlateLoads) -> BTreeSet<usize> {
    let mut by_worst: Vec<(f64, usize)> = loads
        .bodies
        .iter()
        .map(|body| (body.utilization, body.layer))
        .collect();
    let u = &loads.layer_utilization;
    by_worst.extend((0..u.len()).filter_map(|l| {
        let peak = u[l] > 0.0
            && l.checked_sub(1).is_none_or(|below| u[l] >= u[below])
            && u.get(l + 1).is_none_or(|above| u[l] > *above);
        peak.then_some((u[l] * 0.999, l))
    }));
    by_worst.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut chosen = BTreeSet::new();
    for (_, layer) in by_worst {
        if chosen.len() >= CASE_LAYERS {
            break;
        }
        chosen.insert(layer);
    }
    let mut births: Vec<(f64, usize)> = loads
        .bodies
        .iter()
        .filter_map(|body| {
            let first = body.starts.iter().map(|(layer, _)| *layer).min()?;
            Some((body.utilization, first))
        })
        .collect();
    births.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut added = 0;
    for (_, layer) in births {
        if added >= BIRTH_LAYERS {
            break;
        }
        if chosen.insert(layer) {
            added += 1;
        }
    }
    chosen
}

/// Where a tip ends and how wide its contact is.
#[derive(Clone, Copy, Debug)]
struct Contact {
    at: [f64; 3],
    radius: f64,
}

/// The tips of `input`, one per tip: a tip with a neck is two cones, the
/// second starting where the first ends, and only the first ends in the
/// model.
fn contacts(input: &ReviewInput<'_>) -> Vec<Contact> {
    let key = |p: [f64; 3]| p.map(|v| (v * 1e5).round() as i64);
    let joints: BTreeSet<[i64; 3]> = input.tips().iter().map(|cone| key(cone.base_mm)).collect();
    input
        .tips()
        .iter()
        .filter(|cone| !joints.contains(&key(cone.tip_mm)))
        .filter(|cone| cone.tip_mm.iter().all(|v| v.is_finite()))
        .map(|cone| Contact {
            at: cone.tip_mm,
            radius: cone.tip_radius_mm.max(0.0),
        })
        .collect()
}

/// The solid region tip `contact` ends in: on the layer of its end, or on
/// the one above or below when the end falls on a layer's edge. Within the
/// tip's reach of a region's box, the nearest box, then the nearest centre.
fn region_of(
    input: &ReviewInput<'_>,
    regions: &PlateRegions,
    contact: &Contact,
) -> Option<RegionRef> {
    let z = contact.at[2];
    let layer = input.layers().partition_point(|span| span.top_mm <= z);
    if layer >= input.layer_count() {
        return None;
    }
    let (x, y) = input
        .pixel_map()
        .mm_to_pixel(Point2D::new(contact.at[0], contact.at[1]));
    let [pw, ph] = input.pixel_size_mm();
    let reach = contact.radius / pw.min(ph) + 1.0;
    let mut tried = vec![layer, layer + 1];
    if let Some(below) = layer.checked_sub(1) {
        tried.push(below);
    }
    tried
        .into_iter()
        .filter(|&l| l < input.layer_count())
        .find_map(|l| {
            let mut best: Option<(f64, f64, u32)> = None;
            for (index, region) in regions.layer(l).solid.iter().enumerate() {
                let b = region.bbox;
                let dx = (f64::from(b.x0) - x).max(x - f64::from(b.x1)).max(0.0);
                let dy = (f64::from(b.y0) - y).max(y - f64::from(b.y1)).max(0.0);
                let gap = dx.hypot(dy);
                if gap > reach {
                    continue;
                }
                let centre = (region.centroid_px[0] - x).hypot(region.centroid_px[1] - y);
                if best.is_none_or(|(g, c, _)| (gap, centre) < (g, c)) {
                    best = Some((gap, centre, index as u32));
                }
            }
            best.map(|(_, _, index)| RegionRef { layer: l, index })
        })
}

/// What a body knows of itself, kept at its root.
#[derive(Clone, Debug, Default)]
struct Body {
    /// The plate or the raft holds it.
    anchored: bool,
    sums: TipSums,
    tips: Vec<usize>,
    /// The feet standing on it, by index in [`ReviewInput::feet`].
    feet: Vec<usize>,
    starts: Vec<(usize, u32)>,
}

impl Body {
    fn absorb(&mut self, other: Self) {
        self.anchored |= other.anchored;
        self.sums.merge(&other.sums);
        self.tips.extend(other.tips);
        self.feet.extend(other.feet);
        self.starts.extend(other.starts);
    }
}

/// What a body's regions pull with on one layer.
#[derive(Clone, Copy, Debug)]
struct LayerPull {
    root: usize,
    /// A region of the body on this layer, for the body it ends up in.
    region: u32,
    force: Band,
    /// The pull's moments, in pixels.
    moment: [f64; 2],
    cup_mm2: f64,
    cup_moment: [f64; 2],
}

/// How far a foot reaches to the region it stands on, mm.
const FOOT_REACH_MM: f64 = 0.5;

/// The walk of #241: bodies from the plate up, each layer's pull shared
/// among the tips of each body. With `record` given, it only records what
/// holds what on those layers (#244), and returns it.
fn share(
    ctx: &ReviewContext<'_>,
    options: &LoadOptions,
    loads: &mut PlateLoads,
    centres: &[Vec<(u32, [f64; 2])>],
    record: &BTreeSet<usize>,
) -> Vec<LayerCase> {
    let mut cases = Vec::new();
    let (input, regions) = (ctx.input, ctx.regions);
    let model = &options.model;
    let pixel_mm2 = input.pixel_area_mm2();
    let layers = regions.layers();
    let mut offsets = Vec::with_capacity(layers.len() + 1);
    let mut total = 0;
    for table in layers {
        offsets.push(total);
        total += table.solid.len();
    }
    offsets.push(total);

    // Tips, each on its region.
    let mut tips_at: Vec<Vec<usize>> = vec![Vec::new(); total];
    for contact in contacts(input) {
        let Some(region) = region_of(input, regions, &contact) else {
            continue;
        };
        tips_at[offsets[region.layer] + region.index as usize].push(loads.tips.len());
        loads.tip_curves.push(Vec::new());
        loads.tips.push(TipLoad {
            at_mm: contact.at,
            radius_mm: contact.radius,
            layer: region.layer,
            worst_n: 0.0,
            worst_layer: region.layer,
            capacity_n: model.tip_capacity_n(contact.radius),
        });
    }

    // Feet, each on the region it stands on.
    let mut feet_at: Vec<Vec<usize>> = vec![Vec::new(); total];
    for (k, foot) in input.feet().iter().enumerate() {
        let contact = Contact {
            at: *foot,
            radius: FOOT_REACH_MM,
        };
        if let Some(region) = region_of(input, regions, &contact) {
            feet_at[offsets[region.layer] + region.index as usize].push(k);
        }
    }

    let held_below = PLATE_LAYERS.max(input.raft_layers());
    let ends = regions.group_solid(|_, _| true);
    let mut peaks: Vec<Option<BodyPeak>> = vec![None; ends.len()];
    let mut sets = UnionFind::new(total);
    let mut bodies: Vec<Body> = vec![Body::default(); total];
    for (layer, table) in layers.iter().enumerate() {
        for index in 0..table.solid.len() {
            let id = offsets[layer] + index;
            let body = &mut bodies[id];
            body.anchored = layer < held_below;
            if !body.anchored && table.start_of(index as u32).is_some() {
                body.starts.push((layer, index as u32));
            }
            for &tip in &tips_at[id] {
                let at = loads.tips[tip].at_mm;
                body.sums.add([at[0], at[1]], at[2]);
                body.tips.push(tip);
            }
            body.feet.extend(&feet_at[id]);
        }
        for link in regions.solid_links(layer) {
            let a = offsets[layer - 1] + link.lower as usize;
            let b = offsets[layer] + link.upper as usize;
            let (ra, rb) = (sets.find(a), sets.find(b));
            if ra == rb {
                continue;
            }
            sets.union(ra, rb);
            let root = sets.find(ra);
            let other = if root == ra { rb } else { ra };
            let taken = std::mem::take(&mut bodies[other]);
            bodies[root].absorb(taken);
        }

        // Each body's pull on this layer.
        let mut pulls: Vec<LayerPull> = Vec::new();
        let mut pull_of: HashMap<usize, usize> = HashMap::new();
        for (index, region) in table.solid.iter().enumerate() {
            let root = sets.find(offsets[layer] + index);
            let cup = loads.cup_of(layer, index as u32);
            let force = model.region_force_n(region.area_px as f64 * pixel_mm2, None, cup);
            let cup_centre = centres[layer]
                .iter()
                .find(|(around, _)| *around as usize == index)
                .map_or([0.0; 2], |(_, centre)| *centre);
            let pull = LayerPull {
                root,
                region: index as u32,
                force,
                moment: region.centroid_px.map(|v| v * force.mid),
                cup_mm2: cup,
                cup_moment: cup_centre.map(|v| v * cup),
            };
            match pull_of.get(&root).map(|&at| &mut pulls[at]) {
                Some(known) => {
                    known.force = known.force.plus(force);
                    for axis in 0..2 {
                        known.moment[axis] += pull.moment[axis];
                        known.cup_moment[axis] += pull.cup_moment[axis];
                    }
                    known.cup_mm2 += cup;
                }
                None => {
                    pull_of.insert(root, pulls.len());
                    pulls.push(pull);
                }
            }
        }

        if !record.is_empty() {
            if record.contains(&layer) {
                cases.push(layer_case(ctx, layer, &offsets, &mut sets, &bodies, &pulls));
            }
            if record.last().is_some_and(|&last| layer >= last) {
                break;
            }
            continue;
        }

        for pull in pulls {
            let body = &bodies[pull.root];
            let on_tips = !body.anchored && body.sums.count() > 0;
            let at_px = pull
                .moment
                .map(|v| v / pull.force.mid.max(f64::MIN_POSITIVE));
            let at_mm = input.pixel_to_plate(at_px);
            let mut worst: Option<(usize, f64)> = None;
            if on_tips && let Some(group) = body.sums.group() {
                let case = TipLoadCase {
                    force_n: pull.force.mid,
                    at_mm,
                    z_mm: input.layers()[layer].top_mm,
                    lateral_share: model.lateral_share,
                };
                let points = layers.len().div_ceil(loads.curve_layers);
                let point = layer / loads.curve_layers;
                for &tip in &body.tips {
                    let load = &mut loads.tips[tip];
                    let n = group.load_n([load.at_mm[0], load.at_mm[1]], &case);
                    let curve = &mut loads.tip_curves[tip];
                    if curve.is_empty() {
                        curve.resize(points, 0.0);
                    }
                    curve[point] = curve[point].max(n.max(0.0) as f32);
                    if n > load.worst_n {
                        load.worst_n = n;
                        load.worst_layer = layer;
                    }
                    let used = if load.capacity_n.mid > 0.0 {
                        n / load.capacity_n.mid
                    } else {
                        0.0
                    };
                    if worst.is_none_or(|(_, u)| used > u) {
                        worst = Some((tip, used));
                    }
                }
            }
            if pull.cup_mm2 > 0.0 {
                let cup_px = pull.cup_moment.map(|v| v / pull.cup_mm2);
                loads.cups.push(CupLoad {
                    layer,
                    at_mm: input.pixel_to_plate(cup_px),
                    cup_mm2: pull.cup_mm2,
                    on_tips,
                    utilization: worst.map_or(0.0, |(_, u)| u),
                });
            }
            let Some((tip, utilization)) = worst else {
                continue;
            };
            let layer_worst = &mut loads.layer_utilization[layer];
            *layer_worst = layer_worst.max(utilization);
            let end = ends.group_of(RegionRef {
                layer,
                index: pull.region,
            });
            if peaks[end]
                .as_ref()
                .is_some_and(|peak| peak.utilization >= utilization)
            {
                continue;
            }
            let worst_mid = utilization * loads.tips[tip].capacity_n.mid;
            let ratio = |v: f64| {
                if pull.force.mid > 0.0 {
                    v / pull.force.mid
                } else {
                    0.0
                }
            };
            peaks[end] = Some(BodyPeak {
                layer,
                force_n: pull.force,
                at_mm,
                tips: body.sums.count(),
                worst_tip: tip,
                worst_n: Band::new(
                    worst_mid * ratio(pull.force.low),
                    worst_mid,
                    worst_mid * ratio(pull.force.high),
                ),
                utilization,
                starts: body.starts.clone(),
            });
        }
    }
    if record.is_empty() {
        loads.bodies = peaks.into_iter().flatten().collect();
    }
    cases
}

/// What holds what on `layer`: every body printed so far with tips or feet,
/// and what the layer pulls each with.
fn layer_case(
    ctx: &ReviewContext<'_>,
    layer: usize,
    offsets: &[usize],
    sets: &mut UnionFind,
    bodies: &[Body],
    pulls: &[LayerPull],
) -> LayerCase {
    let input = ctx.input;
    let zero = Band::new(0.0, 0.0, 0.0);
    let mut out = Vec::new();
    for (id, body) in bodies.iter().enumerate().take(offsets[layer + 1]) {
        if sets.find(id) != id {
            continue;
        }
        if body.tips.is_empty() && body.feet.is_empty() {
            continue;
        }
        let pull = pulls.iter().find(|p| p.root == id);
        let (force_n, at_mm) = match pull {
            Some(pull) if !body.anchored => {
                let at_px = pull
                    .moment
                    .map(|v| v / pull.force.mid.max(f64::MIN_POSITIVE));
                (pull.force, input.pixel_to_plate(at_px))
            }
            _ => (zero, [0.0, 0.0]),
        };
        out.push(BodyCase {
            force_n,
            at_mm,
            held: body.anchored,
            tips: body.tips.clone(),
            feet: body.feet.iter().map(|&k| input.feet()[k]).collect(),
        });
    }
    LayerCase {
        layer,
        z_mm: input.layers()[layer].top_mm,
        bodies: out,
    }
}

/// Which air regions of every layer drain: open air, or air linked through
/// at least `drain_px` pixels to drained air on the layer below. The build
/// plate closes layer 0.
pub(crate) fn drained_air(regions: &PlateRegions, drain_px: u64) -> Vec<Vec<bool>> {
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

/// The solid region around the enclosed air region `air`: the smallest of
/// `solid` whose box holds its box.
fn enclosing(solid: &[Region], air: &Region) -> Option<u32> {
    let holds = |outer: &Region| {
        outer.bbox.x0 <= air.bbox.x0
            && outer.bbox.y0 <= air.bbox.y0
            && outer.bbox.x1 >= air.bbox.x1
            && outer.bbox.y1 >= air.bbox.y1
    };
    let span = |region: &Region| u64::from(region.bbox.width()) * u64::from(region.bbox.height());
    solid
        .iter()
        .enumerate()
        .filter(|(_, region)| holds(region))
        .min_by_key(|(_, region)| span(region))
        .map(|(index, _)| index as u32)
}

#[cfg(test)]
#[path = "tests_loads.rs"]
mod tests_loads;
