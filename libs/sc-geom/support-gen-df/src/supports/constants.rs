//! DragonFruit `src/supports/constants.ts`.
//!
//! Derived constants for support geometry.
//!
//! Note: User-adjustable defaults are in Settings/defaults.ts
//! This file contains only derived/calculated values.

use crate::three::math_utils::js_max;

// --- Joint Sizing ---
/// How much larger the joint diameter is compared to the shaft/body diameter
pub const JOINT_DIAMETER_OFFSET_MM: f64 = 0.1;

/// Calculate joint diameter from shaft/body diameter.
pub fn get_joint_diameter(shaft_diameter: f64) -> f64 {
    shaft_diameter + JOINT_DIAMETER_OFFSET_MM
}

/// Calculate joint radius from shaft/body diameter.
pub fn get_joint_radius(shaft_diameter: f64) -> f64 {
    get_joint_diameter(shaft_diameter) / 2.0
}

// --- Member sizing ---
/// The shaft a hosted member is built with: the profile band, floored at
/// `host_ratio` of the host shaft it sprouts from.
///
/// A host trunk's diameter rides the model factors (height, print scale, mass
/// per support) while a hosted member is built from the profile band, so on a
/// scaled part a branch came out at the band beside a markedly fatter trunk —
/// the "supports read thin next to their hosts" report. The two meet at the
/// knot, so the member is floored here: a step between them is right (a member
/// carries a fraction of the host's load) but not a needle. The floor is a
/// maximum with the band, so a host at the band is untouched, and it binds only
/// past `band / hostRatio` (~1.43 × band at the default 0.7).
///
/// `host_ratio` is the `autoSupport.memberHostShaftRatio` setting (default 0.7,
/// measured rather than chosen: at ×1.6 — host 1.62 mm — the member/host ratio
/// runs 0.62 with no floor, called *a little thin*, and 0.80 with the first
/// attempt at this value, called *a little thick*, so the published value sits
/// between them at a 1.13 mm branch). The callers pair it with a band fallback —
/// a thicker member that fails its clearance gate is rebuilt at `bandShaftMm`
/// (`buildHostedBranch`, `autoPlace.ts`) — because the floor is a fit rule, not a
/// licence to lose a link.
pub fn member_shaft_diameter_mm(band_shaft_mm: f64, host_diameter_mm: f64, host_ratio: f64) -> f64 {
    js_max(band_shaft_mm, host_diameter_mm * host_ratio)
}
