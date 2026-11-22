/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Force-based margins for islands and suction cups (#129).
//!
//! Reports [`CheckKind::TipLoad`] and grades the
//! [`CheckKind::SuctionCup`] findings of [`crate::checks::traps`], from the
//! peel loads of [`crate::loads`].
//!
//! # Parts pulled harder than their tips hold
//!
//! An island the islands check finds held by a tip is held by area: any tip
//! under its first pixels holds it. What decides whether it stays is force:
//! every layer of the part, and of everything that grows on it before it
//! joins something the plate holds, pulls on the film, and its tips share
//! that pull ([`crate::loads::BodyPeak`]). A part whose most loaded tip takes
//! more than it holds, at the middle of both bands, is a warning; one whose
//! tip takes more even at the low end of the pull and the high end of the
//! strength is an error. The fix adds a tip on the part's first surfaces,
//! where they come nearest to where the pull centres: the tip that takes the
//! most of it.
//!
//! # Suction cups by force
//!
//! A cup's severity comes from its area (see [`crate::checks::traps`]); its
//! finding now says what it pulls on the film with, and a cup in a part that
//! hangs from tips already pulled harder than they hold on its layers is an
//! error, whatever its area.
//!
//! The numbers are estimates: nothing is calibrated with prints yet, and the
//! findings say so.

use crate::finding::{CheckKind, Finding, PlatePoint, RoleScope, Severity, SuggestedFix};
use crate::loads::{BodyPeak, PlateLoads};
use crate::runner::ReviewContext;
use bread_peel::PeelModel;

/// Default of [`TipLoadOptions::warn_utilization`].
pub const DEFAULT_WARN_UTILIZATION: f64 = 1.0;

/// What every finding of this check ends with.
pub const UNCALIBRATED: &str = "An estimate, not yet calibrated with prints.";

/// Settings of the tip-load check.
#[derive(Clone, Debug, PartialEq)]
pub struct TipLoadOptions {
    /// The load over what a tip holds, at the middle of both bands, from
    /// which a part is listed: 1.
    pub warn_utilization: f64,
}

impl Default for TipLoadOptions {
    fn default() -> Self {
        Self {
            warn_utilization: DEFAULT_WARN_UTILIZATION,
        }
    }
}

/// The parts of the plate whose tips are pulled harder than they hold.
pub fn run(ctx: &ReviewContext<'_>, loads: &PlateLoads, options: &TipLoadOptions) -> Vec<Finding> {
    loads
        .bodies
        .iter()
        .filter(|peak| peak.utilization >= options.warn_utilization)
        .map(|peak| body_finding(ctx, loads, peak))
        .collect()
}

fn body_finding(ctx: &ReviewContext<'_>, loads: &PlateLoads, peak: &BodyPeak) -> Finding {
    let tip = &loads.tips[peak.worst_tip];
    let severity = if peak.worst_n.low > tip.capacity_n.high {
        Severity::Error
    } else {
        Severity::Warning
    };
    let first = peak
        .starts
        .iter()
        .map(|(layer, _)| *layer)
        .min()
        .unwrap_or(tip.layer)
        .min(peak.layer);
    let held_by = if peak.tips == 1 {
        "1 tip".to_owned()
    } else {
        format!("{} tips", peak.tips)
    };
    let detail = format!(
        "Peeling layer {} pulls about {:.1} N on this part, held by {held_by}: the most loaded takes about {:.2} N, {:.1} times what a {:.2} mm tip holds. {UNCALIBRATED}",
        peak.layer + 1,
        peak.force_n.mid,
        peak.worst_n.mid,
        peak.utilization,
        2.0 * tip.radius_mm,
    );
    let focus = PlatePoint::new(tip.at_mm[0], tip.at_mm[1], tip.at_mm[2]);
    let finding = Finding::new(
        CheckKind::TipLoad,
        severity,
        first,
        peak.layer,
        focus,
        RoleScope::Both,
        "Tips pulled harder than they hold",
        detail,
    );
    match new_tip(ctx, peak) {
        Some(at) => finding.with_fix(SuggestedFix::AddSupport { at }),
        None => finding,
    }
}

/// Where one more tip takes the most of `peak`'s pull: the pixel of the
/// part's first surfaces nearest to where the pull centres, at the bottom
/// of its layer.
fn new_tip(ctx: &ReviewContext<'_>, peak: &BodyPeak) -> Option<PlatePoint> {
    let input = ctx.input;
    let [pw, ph] = input.pixel_size_mm();
    let (tx, ty) = input
        .pixel_map()
        .mm_to_pixel(sc_sl1cer::geom2d::Point2D::new(
            peak.at_mm[0],
            peak.at_mm[1],
        ));
    let mut best: Option<(f64, usize, [f64; 2])> = None;
    for &(layer, region) in &peak.starts {
        let Some(start) = ctx.regions.layer(layer).start_of(region) else {
            continue;
        };
        for run in &start.runs {
            let x = (tx - 0.5)
                .clamp(f64::from(run.x0), f64::from(run.x1 - 1))
                .round()
                + 0.5;
            let y = f64::from(run.row) + 0.5;
            let distance = ((x - tx) * pw).hypot((y - ty) * ph);
            if best.is_none_or(|(d, _, _)| distance < d) {
                best = Some((distance, layer, [x, y]));
            }
        }
    }
    let (_, layer, pixel) = best?;
    Some(input.underside(layer, input.focus(layer, pixel)))
}

/// Says what each suction cup of `findings` pulls on the film with, and makes
/// an error of one in a part whose tips are pulled harder than they hold on
/// its layers.
pub fn grade_cups(
    findings: &mut [Finding],
    loads: &PlateLoads,
    model: &PeelModel,
    options: &TipLoadOptions,
) {
    for finding in findings
        .iter_mut()
        .filter(|finding| finding.check == CheckKind::SuctionCup)
    {
        let mut largest: f64 = 0.0;
        let mut worst: f64 = 0.0;
        for cup in &loads.cups {
            if cup.layer < finding.first_layer || cup.layer > finding.last_layer {
                continue;
            }
            let reach = 2.0 * (cup.cup_mm2 / std::f64::consts::PI).sqrt() + 1.0;
            let off = (cup.at_mm[0] - finding.focus.x_mm).hypot(cup.at_mm[1] - finding.focus.y_mm);
            if off > reach {
                continue;
            }
            largest = largest.max(cup.cup_mm2);
            if cup.on_tips {
                worst = worst.max(cup.utilization);
            }
        }
        if largest <= 0.0 {
            continue;
        }
        let pull = model.cup_force_n(largest).mid;
        let mut detail = format!(
            "{} At its widest it pulls about {pull:.1} N on the film at each peel.",
            finding.detail
        );
        if worst >= options.warn_utilization {
            finding.severity = Severity::Error;
            detail.push_str(&format!(
                " The tips holding this part take up to {worst:.1} times what they hold then."
            ));
        }
        detail.push(' ');
        detail.push_str(UNCALIBRATED);
        finding.detail = detail;
    }
}

#[cfg(test)]
#[path = "tests_tip_load.rs"]
mod tests;
