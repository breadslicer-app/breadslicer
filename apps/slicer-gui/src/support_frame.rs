/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The supports checked as a beam frame on the critical layers (#244): what
//! the rigid share of #241 cannot see, as soon as supports hang from one
//! another, are braced, or bend under the sideways share of the peel.
//!
//! Each set of supports in force is read as a frame
//! ([`sc_geom::supports::SupportSession::support_graph`]), moved into the
//! slice's plate frame, and every critical layer of the review
//! ([`bread_review::LayerCase`]) is solved on its own thread: each body the
//! layer pulls, held by its tips and its feet, pulled up and sideways in
//! every direction. A report only: nothing here changes a support.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use bread_review::PlateLoads;
use bread_structure::{
    BodyLoad, CheckOptions, LayerCheck, Material, Part, SupportGraph, check_layer,
};
use rayon::prelude::*;
use sc_print_glue::PlateFrame;

use crate::support_persist::DrawnSet;

/// A support whose frame utilisation passes 1 by more than this is a weak
/// spot to reinforce (#247).
pub(crate) const WEAK_FROM: f64 = 1.0;

/// Each support's utilisation, per set, by id: as [`crate::stress_view`]
/// paints them.
pub(crate) type Shares = Vec<Vec<(String, f64)>>;

/// What the frame found.
#[derive(Clone, Debug, Default)]
pub(crate) struct FrameShares {
    /// Each critical layer and what it does to each support.
    pub(crate) layers: Vec<(usize, Shares)>,
    /// Each support's worst over those layers.
    pub(crate) worst: Shares,
    /// Per set, the part of each support worst off then.
    pub(crate) parts: Vec<std::collections::HashMap<String, Part>>,
    /// The highest utilisation and its layer.
    pub(crate) peak: Option<(f64, usize)>,
    /// How long it took.
    pub(crate) took: Duration,
}

impl FrameShares {
    /// The supports past [`WEAK_FROM`], per set: id, utilisation and the
    /// part worst off.
    pub(crate) fn weak(&self) -> Vec<Vec<(String, f64, Part)>> {
        self.worst
            .iter()
            .zip(&self.parts)
            .map(|(shares, parts)| {
                shares
                    .iter()
                    .filter(|(_, u)| *u >= WEAK_FROM)
                    .map(|(id, u)| (id.clone(), *u, parts.get(id).copied().unwrap_or_default()))
                    .collect()
            })
            .collect()
    }

    /// What layer `layer` does to the supports, when it is one checked.
    pub(crate) fn at(&self, layer: usize) -> Option<&Shares> {
        self.layers
            .iter()
            .find(|(l, _)| *l == layer)
            .map(|(_, shares)| shares)
    }
}

/// How the frame is checked: the peel's sideways share, as the loads of
/// #241 take it, and the resin's strength in the vat. The tips' strength of
/// #241 has the concentration of a tip's narrowing taken in; the frame
/// applies that concentration where a tip meets the model
/// ([`bread_structure::NECK_KT`]) and nowhere else, so it checks every beam
/// against the strength without it: a tip comes out as #241 has it, a
/// shaft against the resin's own strength.
fn options() -> CheckOptions {
    let model = bread_peel::PeelModel::default();
    CheckOptions {
        material: Material::GREEN_RESIN,
        strength_mpa: model.tip_strength_mpa.mid * bread_structure::NECK_KT,
        lateral_share: model.lateral_share,
        directions: 8,
    }
}

fn shares_of(graphs: &[SupportGraph], check: &LayerCheck) -> Shares {
    graphs
        .iter()
        .zip(&check.owners)
        .map(|(graph, owners)| {
            let mut out: Vec<(String, f64)> = graph
                .owners()
                .iter()
                .zip(owners)
                .filter(|(_, u)| **u > 0.0)
                .map(|(id, u)| (id.clone(), *u))
                .collect();
            out.sort_by(|a, b| a.0.cmp(&b.0));
            out
        })
        .collect()
}

/// Checks `sets` as a frame on every critical layer of `loads`, in the
/// slice's `frame`. `None` with nothing to check, or once `cancel` trips.
pub(crate) fn frame_shares(
    loads: &PlateLoads,
    frame: &PlateFrame,
    sets: &[DrawnSet],
    cancel: &AtomicBool,
) -> Option<FrameShares> {
    if loads.cases.is_empty() || sets.is_empty() {
        return None;
    }
    let start = Instant::now();
    let options = options();
    let graphs: Vec<SupportGraph> = sets
        .par_iter()
        .map(|drawn| {
            drawn
                .session
                .support_graph()
                .mapped(|p| frame.point_mm(drawn.leaf, p))
        })
        .collect();
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let checks: Vec<(usize, LayerCheck)> = loads
        .cases
        .par_iter()
        .filter_map(|case| {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let bodies: Vec<BodyLoad> = case
                .bodies
                .iter()
                .map(|body| BodyLoad {
                    force_n: body.force_n.mid,
                    at: [body.at_mm[0], body.at_mm[1], case.z_mm],
                    held: body.held,
                    tips: body.tips.iter().map(|&k| loads.tips[k].at_mm).collect(),
                    feet: body.feet.clone(),
                })
                .collect();
            let check = check_layer(&graphs, case.z_mm, &bodies, &options).ok()?;
            Some((case.layer, check))
        })
        .collect();
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let mut out = FrameShares::default();
    let mut worst: Vec<std::collections::BTreeMap<String, (f64, Part)>> =
        vec![Default::default(); graphs.len()];
    for (layer, check) in &checks {
        if out.peak.is_none_or(|(u, _)| check.worst > u) && check.worst > 0.0 {
            out.peak = Some((check.worst, *layer));
        }
        for (set, graph) in graphs.iter().enumerate() {
            for (owner, id) in graph.owners().iter().enumerate() {
                let u = check.owners[set][owner];
                if u <= 0.0 {
                    continue;
                }
                let slot = worst[set].entry(id.clone()).or_insert((0.0, Part::Shaft));
                if u > slot.0 {
                    *slot = (u, check.parts[set][owner]);
                }
            }
        }
        out.layers.push((*layer, shares_of(&graphs, check)));
    }
    out.parts = worst
        .iter()
        .map(|m| {
            m.iter()
                .map(|(id, (_, part))| (id.clone(), *part))
                .collect()
        })
        .collect();
    out.worst = worst
        .into_iter()
        .map(|m| m.into_iter().map(|(id, (u, _))| (id, u)).collect())
        .collect();
    out.took = start.elapsed();
    Some(out)
}
