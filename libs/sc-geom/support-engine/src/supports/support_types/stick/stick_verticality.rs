//! DragonFruit `src/supports/SupportTypes/Stick/stickVerticality.ts`.
//!
//! A stick is a vertical bridge between two model contacts. Surface-normal
//! standoffs shove its sockets sideways on sloped surfaces, and past a certain
//! cant the result is a wedged stick rather than a bridge -- so the placement
//! that produced it is rejected and the caller falls back.

use crate::supports::support_types::shaft_verticality::shaft_vertical_cos;
use crate::supports::types::Segment;
use crate::three::math_utils::js_cos;

/// Cant beyond this is a wedged stick, not a bridge (calibration knob).
pub const MAX_SHAFT_ANGLE_DEG: f64 = 20.0;

/// Whether a built stick stands vertical enough to keep.
pub fn is_shaft_vertical_enough(segments: &[Segment]) -> bool {
    shaft_vertical_cos(segments) >= js_cos((MAX_SHAFT_ANGLE_DEG * std::f64::consts::PI) / 180.0)
}
