/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Islands (#84): solid regions that start in mid-air, with too little cured
//! resin below them to hold them at the next peel.
//!
//! Reports [`CheckKind::Island`] findings.
//!
//! # Rule
//!
//! On every layer `k > 0`, a solid [`Region`] is an *island layer* when the
//! pixels of it that rest on solid in layer `k - 1` (its `below_px`) are
//! fewer than [`MIN_SUPPORTED_FRACTION`] of its area. Layer 0 rests on the
//! build plate and is never an island.
//!
//! Solid is model and support merged, so support pixels below a region hold
//! it exactly as model pixels do: a part of the model that a support tip
//! touches from below is supported, which is the point of supports. What the
//! check adds over a pixel-only view is the role of what is held:
//!
//! - a **model island** (one with model pixels) is a part of the model that
//!   prints in mid-air: an [`Severity::Error`] when nothing at all is below
//!   it, a [`Severity::Warning`] when something is below it but less than the
//!   margin. It suggests a support at the island's lowest layer;
//! - a **support island** (support pixels only) is a support that floats or
//!   barely rests on anything, a defect of the supports themselves, with the
//!   same severities. It suggests no fix: a support's tip goes on the model,
//!   and a floating support is mended by regenerating or editing the
//!   supports.
//!
//! # One finding per physical island
//!
//! A part that grows from nothing stays under the margin for a layer or
//! two: a cone tip is an island on exactly its first two layers. Island
//! layers of consecutive layers that overlap are joined with
//! [`PlateRegions::group_solid`], so one physical island is one finding over
//! its layer range. Island layers that do not overlap stay apart: two tips
//! side by side are two islands, each needing its support.
//!
//! The finding's numbers and severity come from the island's weakest layer
//! (the lowest supported fraction; on a tie the lowest layer). Its focus and
//! suggested support are at the pixel nearest the centroid of the largest
//! region of its lowest layer, where it starts.
//!
//! # Noise
//!
//! An island whose every layer has fewer than [`MIN_ISLAND_AREA_PX`] pixels
//! is not reported: rasterising a surface that grazes a layer's cutting
//! height can light a stray pixel or two. The size of an island is that of
//! its largest island layer, not of its first one, so a real tip that starts
//! with a single pixel is still reported once it grows.

use crate::finding::{CheckKind, Finding, RoleScope, Severity, SuggestedFix};
use crate::regions::{PlateRegions, Region};
use crate::runner::ReviewContext;
use crate::union_find::RegionRef;

/// The fraction of a region's area that must rest on cured resin in the
/// layer below for the region to count as held: 25 %.
///
/// Peel force grows with the area of the new layer, and what resists it is
/// the cured area below, so the ratio of the two is the margin. A quarter is
/// the usual starting point for resin island detection. It also matches the
/// commonest real island, a cone tip pointing down: its area grows with the
/// square of its height, so its second layer rests on at most a quarter of
/// its area and its third on more than a quarter, and the island is its
/// first two layers.
pub const MIN_SUPPORTED_FRACTION: f64 = 0.25;

/// The smallest island reported, in pixels of its largest island layer: 4,
/// a 2 x 2 pixel square (38 x 48 µm, 0.0018 mm² on a 19 x 24 µm screen).
///
/// Measured on a 1.9 M-triangle miniature with generated supports (Saturn 3
/// Ultra, 0.1 mm layers): with no minimum at all, the smallest island found
/// had 67 pixels (a support branch starting in mid-air) and the smallest
/// model island 397 pixels, so the minimum costs no real finding there. By
/// geometry, a cone tip's largest island layer, its second, stays under 4
/// pixels on that screen only when the cone is sharper than 7 to 13° from
/// its axis (depending on where the layers cut it): a needle whose first
/// 0.2 mm the printer does not reproduce anyway.
pub const MIN_ISLAND_AREA_PX: u64 = 4;

/// Settings of the island check.
#[derive(Clone, Debug, PartialEq)]
pub struct IslandOptions {
    /// The fraction of a region's area, from 0 to 1, that must rest on cured
    /// resin below it. Default [`MIN_SUPPORTED_FRACTION`].
    pub min_supported_fraction: f64,
    /// The smallest island reported, in pixels of its largest island layer.
    /// Default [`MIN_ISLAND_AREA_PX`].
    pub min_area_px: u64,
}

impl Default for IslandOptions {
    fn default() -> Self {
        Self {
            min_supported_fraction: MIN_SUPPORTED_FRACTION,
            min_area_px: MIN_ISLAND_AREA_PX,
        }
    }
}

impl IslandOptions {
    /// Whether `region`, on a layer above the first, rests on too little.
    fn is_island(&self, region: &Region) -> bool {
        (region.below_px as f64) < self.min_supported_fraction * region.area_px as f64
    }
}

/// Finds the islands of the plate.
pub fn run(ctx: &ReviewContext<'_>, options: &IslandOptions) -> Vec<Finding> {
    let regions = ctx.regions;
    let flags = island_flags(regions, options);
    if flags.iter().all(|layer| !layer.contains(&true)) {
        return Vec::new();
    }
    // Only island layers join, so every other region stays a group of its
    // own and an island's group holds island layers only.
    let groups = regions.group_solid(|upper, overlap| {
        flags[upper - 1][overlap.lower as usize] && flags[upper][overlap.upper as usize]
    });
    groups
        .iter()
        .filter(|members| flags[members[0].layer][members[0].index as usize])
        .filter_map(|members| island_finding(ctx, members, options))
        .collect()
}

/// Whether each solid region of each layer is an island layer.
fn island_flags(regions: &PlateRegions, options: &IslandOptions) -> Vec<Vec<bool>> {
    regions
        .layers()
        .iter()
        .enumerate()
        .map(|(layer, layer_regions)| {
            layer_regions
                .solid
                .iter()
                .map(|region| layer > 0 && options.is_island(region))
                .collect()
        })
        .collect()
}

fn region_of(regions: &PlateRegions, member: RegionRef) -> &Region {
    &regions.layer(member.layer).solid[member.index as usize]
}

/// The supported fraction of a solid region, from 0 to 1.
fn supported_fraction(region: &Region) -> f64 {
    region.below_px as f64 / region.area_px.max(1) as f64
}

/// The finding for one island, made of the island layers `members` (lowest
/// first), or `None` when it is too small to report.
fn island_finding(
    ctx: &ReviewContext<'_>,
    members: &[RegionRef],
    options: &IslandOptions,
) -> Option<Finding> {
    let regions = ctx.regions;
    let input = ctx.input;
    let largest_px = members
        .iter()
        .map(|&m| region_of(regions, m).area_px)
        .max()?;
    if largest_px < options.min_area_px {
        return None;
    }
    let is_model = members.iter().any(|&m| region_of(regions, m).model_px > 0);
    let weakest = *members.iter().min_by(|&&a, &&b| {
        let (ra, rb) = (region_of(regions, a), region_of(regions, b));
        supported_fraction(ra)
            .total_cmp(&supported_fraction(rb))
            .then(a.layer.cmp(&b.layer))
            .then(rb.area_px.cmp(&ra.area_px))
            .then(a.index.cmp(&b.index))
    })?;
    let first_layer = members[0].layer;
    let last_layer = members[members.len() - 1].layer;
    let start = *members
        .iter()
        .take_while(|m| m.layer == first_layer)
        .min_by(|&&a, &&b| {
            region_of(regions, b)
                .area_px
                .cmp(&region_of(regions, a).area_px)
                .then(a.index.cmp(&b.index))
        })?;
    let focus = input.focus(start.layer, region_of(regions, start).inside_point());

    let weakest_region = region_of(regions, weakest);
    let pixel_mm2 = input.pixel_area_mm2();
    let area = significant(weakest_region.area_px as f64 * pixel_mm2);
    let what = if is_model { "model" } else { "support" };
    let on_layer = if weakest.layer == first_layer {
        String::new()
    } else {
        format!("On layer {}, ", weakest.layer + 1)
    };
    let (severity, title, detail) = if weakest_region.below_px == 0 {
        let title = if is_model {
            "Unsupported island"
        } else {
            "Floating support"
        };
        let detail = format!(
            "{on_layer}{area} mm² of {what} starts with nothing cured below it (0 % supported)."
        );
        (Severity::Error, title, detail)
    } else {
        let title = if is_model {
            "Weakly supported island"
        } else {
            "Weakly held support"
        };
        let detail = format!(
            "{on_layer}{area} mm² of {what} rests on only {} mm² of cured resin ({} supported, under the {:.0} % margin).",
            significant(weakest_region.below_px as f64 * pixel_mm2),
            percent(weakest_region.below_px, weakest_region.area_px),
            options.min_supported_fraction * 100.0,
        );
        (Severity::Warning, title, detail)
    };
    let scope = if is_model {
        RoleScope::Model
    } else {
        RoleScope::Support
    };
    let finding = Finding::new(
        CheckKind::Island,
        severity,
        first_layer,
        last_layer,
        focus,
        scope,
        title,
        detail,
    );
    Some(if is_model {
        finding.with_fix(SuggestedFix::AddSupport { at: focus })
    } else {
        finding
    })
}

/// `value` with three significant digits and no exponent: `0.00456`,
/// `0.123`, `1.23`, `12.3`, `123`.
fn significant(value: f64) -> String {
    if value <= 0.0 || !value.is_finite() {
        return "0".to_string();
    }
    let magnitude = value.log10().floor() as i32;
    let decimals = (2 - magnitude).max(0) as usize;
    format!("{value:.decimals$}")
}

/// `part` of `whole` as a percentage rounded down, so a fraction just under
/// the margin never prints as the margin itself; `"<1 %"` for a non-zero
/// part under one percent.
fn percent(part: u64, whole: u64) -> String {
    let pct = part * 100 / whole.max(1);
    if pct == 0 && part > 0 {
        "<1 %".to_string()
    } else {
        format!("{pct} %")
    }
}

#[cfg(test)]
#[path = "tests_islands.rs"]
mod tests;
