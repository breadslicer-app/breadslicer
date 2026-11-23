/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Support tips thinner than the screen draws (#724).
//!
//! Reports [`CheckKind::ThinTip`]. A tip touches the model with a disc of its
//! contact diameter, and the screen lights that disc pixel by pixel: a disc
//! under 2.5 pixels across is never fully lit with antialiasing on, wherever
//! it falls on the grid, and one under 1.5 pixels can vanish with it off.
//! Such a tip looks fine in the view and lets go in the print. Supports made
//! for the printer are never that thin; a set made for another printer, or
//! imported, can be.

use std::collections::HashSet;

use crate::finding::{CheckKind, Finding, PlatePoint, RoleScope, Severity};
use crate::runner::ReviewContext;
use crate::tips::TipCone;

/// The narrowest tip, in pixels of the printer's screen: past the 2.83 a
/// disc needs to light a full pixel wherever it falls, with some room. The
/// support engine makes no tip thinner.
pub const MIN_TIP_PIXELS: f64 = 3.0;

/// One finding for the tips of the plate thinner than [`MIN_TIP_PIXELS`] of
/// its screen, focused on the thinnest; none when there are none.
pub fn run(ctx: &ReviewContext<'_>) -> Vec<Finding> {
    let input = ctx.input;
    let [pw, ph] = input.pixel_size_mm();
    let narrowest = MIN_TIP_PIXELS * pw.max(ph);
    let thin: Vec<&TipCone> = contacts(input.tips())
        .filter(|tip| 2.0 * tip.tip_radius_mm < narrowest - 1e-9)
        .collect();
    let Some(thinnest) = thin
        .iter()
        .min_by(|a, b| a.tip_radius_mm.total_cmp(&b.tip_radius_mm))
    else {
        return Vec::new();
    };
    let layer_of = |z: f64| {
        input
            .layers()
            .iter()
            .position(|layer| layer.top_mm >= z)
            .unwrap_or(input.layer_count().saturating_sub(1))
    };
    let layers = thin.iter().map(|tip| layer_of(tip.tip_mm[2]));
    let first = layers.clone().min().unwrap_or(0);
    let last = layers.max().unwrap_or(first);
    let thinnest_mm = 2.0 * thinnest.tip_radius_mm;
    let detail = format!(
        "{count} touch the model with less than {narrowest:.2} mm, three pixels of this printer's screen; the thinnest is {thinnest_mm:.2} mm, {pixels:.1} pixels. The screen cannot light so small a contact fully, so it holds less than it looks. Generate the supports again for this printer, or widen these tips.",
        count = if thin.len() == 1 {
            "1 tip".to_owned()
        } else {
            format!("{} tips", thin.len())
        },
        pixels = thinnest_mm / pw.max(ph),
    );
    let [x, y, z] = thinnest.tip_mm;
    vec![Finding::new(
        CheckKind::ThinTip,
        Severity::Warning,
        first,
        last,
        PlatePoint::new(x, y, z),
        RoleScope::Both,
        "Tips thinner than the screen draws",
        detail,
    )]
}

/// The stretch of each tip that touches the model: a tip with a neck is two
/// stretches, and the cone above the neck starts where the neck ends.
fn contacts(tips: &[TipCone]) -> impl Iterator<Item = &TipCone> {
    let key = |p: [f64; 3]| p.map(f64::to_bits);
    let ends: HashSet<[u64; 3]> = tips.iter().map(|tip| key(tip.base_mm)).collect();
    tips.iter()
        .filter(move |tip| !ends.contains(&key(tip.tip_mm)))
}
