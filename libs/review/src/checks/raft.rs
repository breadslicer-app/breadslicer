/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The raft check (#131): a model that reaches down into the supports' raft.
//!
//! Reports [`CheckKind::ModelInRaft`] findings. Generated supports stand on
//! a raft whose plate and crenelated wall rise [`ReviewInput::raft_top_mm`]
//! above the build plate, and the raft is cleared around a model that stands
//! in it; a model lower than the raft's top shares its first layers with the
//! raft, and the supports, which start on the raft, never reach what the
//! model overhangs there.
//!
//! - **Model inside the raft** (error), one finding per model leaf with at
//!   least [`RaftOptions::min_area_px`] pixels' worth of contour on a layer
//!   of the raft zone ([`ReviewInput::raft_layers`]), from its first such
//!   layer to its last, focused on the centre of what it has there. Only
//!   when the plate has a raft: an input that knows the raft's top, with
//!   supports on the first layer. It suggests
//!   [`SuggestedFix::ClearRaft`]: lift the model to the recommended support
//!   lift ([`ReviewInput::recommended_lift_mm`]), or turn the raft off.
//!   Lifting it only above the raft would leave the supports no room to
//!   grow under its lowest overhangs.
//!
//! Islands that start below the recommended lift, within the footprint the
//! model in the raft has below it, are not reported on their own
//! ([`covers`]): the raft finding explains them, and no support fits under
//! them until the model is lifted.
//!
//! [`ReviewInput::raft_top_mm`]: crate::input::ReviewInput::raft_top_mm
//! [`ReviewInput::raft_layers`]: crate::input::ReviewInput::raft_layers
//! [`ReviewInput::recommended_lift_mm`]: crate::input::ReviewInput::recommended_lift_mm

use sc_sl1cer::contour::Polygon;

use crate::finding::{CheckKind, Finding, PlatePoint, RoleScope, Severity, SuggestedFix};
use crate::runner::ReviewContext;

/// Settings of the raft check.
#[derive(Clone, Debug, PartialEq)]
pub struct RaftOptions {
    /// The least contour area, in pixels, a model must have on a layer of
    /// the raft zone to be in the raft: less is a sliver of the mesh.
    pub min_area_px: f64,
    /// How far, in millimetres, around the footprint a model in the raft has
    /// below the recommended lift an island may start and still be covered
    /// by the raft finding.
    pub island_margin_mm: f64,
}

impl Default for RaftOptions {
    fn default() -> Self {
        Self {
            min_area_px: 1.0,
            island_margin_mm: 1.0,
        }
    }
}

/// A model in the raft: its finding and where it stands in the raft zone.
#[derive(Clone, Debug, PartialEq)]
pub struct RaftClash {
    /// The finding reported for it.
    pub finding: Finding,
    /// The layers below the recommended lift, `0..cover_layers`: an island
    /// that starts on one of them, within the footprint, is covered.
    pub cover_layers: usize,
    /// The bounding box of the model's contours below the recommended lift,
    /// `[min_x, min_y, max_x, max_y]` in plate millimetres.
    pub footprint_mm: [f64; 4],
}

/// Runs the check: one finding per model in the raft.
pub fn run(ctx: &ReviewContext<'_>, options: &RaftOptions) -> Vec<Finding> {
    clashes(ctx, options)
        .into_iter()
        .map(|clash| clash.finding)
        .collect()
}

/// Every model in the raft, with what [`covers`] needs.
pub fn clashes(ctx: &ReviewContext<'_>, options: &RaftOptions) -> Vec<RaftClash> {
    let input = ctx.input;
    let zone = input.raft_layers().min(input.layer_count());
    let Some(raft_top_mm) = input.raft_top_mm() else {
        return Vec::new();
    };
    // With no support on the plate, no raft was generated.
    if zone == 0 || !input.has_support_on(0) {
        return Vec::new();
    }
    let min_area_mm2 = options.min_area_px * input.pixel_area_mm2();
    let spans = input.layers();
    let lift_to_mm = input
        .recommended_lift_mm()
        .unwrap_or(spans[zone - 1].top_mm);
    let cover = input.lift_layers().clamp(zone, input.layer_count());
    let mut clashes = Vec::new();
    for leaf in input.leaves() {
        let mut range: Option<(usize, usize)> = None;
        let mut footprint = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        for (layer, contours) in leaf.stack.layers()[..cover].iter().enumerate() {
            let polygons = contours.polygons();
            if layer_area_mm2(polygons) < min_area_mm2 {
                continue;
            }
            if layer < zone {
                range = Some(range.map_or((layer, layer), |(first, _)| (first, layer)));
            }
            for polygon in polygons {
                for v in polygon.outer().vertices() {
                    footprint[0] = footprint[0].min(v.x);
                    footprint[1] = footprint[1].min(v.y);
                    footprint[2] = footprint[2].max(v.x);
                    footprint[3] = footprint[3].max(v.y);
                }
            }
        }
        let Some((first, last)) = range else {
            continue;
        };
        let focus = PlatePoint::new(
            (footprint[0] + footprint[2]) * 0.5,
            (footprint[1] + footprint[3]) * 0.5,
            spans[first].sample_mm,
        );
        let finding = Finding::new(
            CheckKind::ModelInRaft,
            Severity::Error,
            first,
            last,
            focus,
            RoleScope::Model,
            "Model inside the raft",
            format!(
                "The raft takes {zone} {layers} ({raft_top_mm:.2} mm) and the model reaches down into it from layer {from}, leaving no room for supports under it; lift it to {lift_to_mm:.2} mm or turn the raft off.",
                layers = if zone == 1 { "layer" } else { "layers" },
                from = first + 1,
            ),
        )
        .with_fix(SuggestedFix::ClearRaft { lift_to_mm });
        clashes.push(RaftClash {
            finding,
            cover_layers: cover,
            footprint_mm: footprint,
        });
    }
    clashes
}

/// Whether `finding` is an island the raft finding of one of `clashes`
/// explains: an island, of model or of the supports crammed under it, that
/// starts below the recommended lift, within `margin_mm` of the footprint
/// the model in the raft has there.
pub fn covers(clashes: &[RaftClash], finding: &Finding, margin_mm: f64) -> bool {
    finding.check == CheckKind::Island
        && clashes.iter().any(|clash| {
            let [x0, y0, x1, y1] = clash.footprint_mm;
            let (x, y) = (finding.focus.x_mm, finding.focus.y_mm);
            finding.first_layer < clash.cover_layers
                && x >= x0 - margin_mm
                && x <= x1 + margin_mm
                && y >= y0 - margin_mm
                && y <= y1 + margin_mm
        })
}

/// The area the polygons of one layer cover, in square millimetres.
fn layer_area_mm2(polygons: &[Polygon]) -> f64 {
    polygons
        .iter()
        .map(|polygon| {
            polygon.outer().signed_area().abs()
                - polygon
                    .holes()
                    .iter()
                    .map(|hole| hole.signed_area().abs())
                    .sum::<f64>()
        })
        .sum()
}

#[cfg(test)]
#[path = "tests_raft.rs"]
mod tests;
