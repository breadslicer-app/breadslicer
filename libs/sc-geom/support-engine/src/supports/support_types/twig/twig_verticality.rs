//! DragonFruit `src/supports/SupportTypes/Twig/twigVerticality.ts`.
//!
//! The verticality gate a twig must pass to be worth placing. Looser than the
//! stick's 20°, for a measured reason: thin-gap and floor twigs build at ≤17°,
//! pointed-tip props off a nearby wall land 23–43°, and the grazers — the ones
//! that read as useless whiskers — start at 48°. Enforced in the twig's
//! registered builder (`contact_bridge`), so every automatic path is covered.

use crate::supports::support_types::shaft_verticality::shaft_vertical_cos;
use crate::supports::types::Segment;
use crate::three::math_utils::js_cos;

pub const MAX_TWIG_SHAFT_ANGLE_DEG: f64 = 45.0;

/// Whether a built twig stands vertical enough to keep.
pub fn is_twig_shaft_vertical_enough(segments: &[Segment]) -> bool {
    shaft_vertical_cos(segments)
        >= js_cos((MAX_TWIG_SHAFT_ANGLE_DEG * std::f64::consts::PI) / 180.0)
}
