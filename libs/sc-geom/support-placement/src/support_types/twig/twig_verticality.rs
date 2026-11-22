//! The lean a twig may have and still be kept. It is looser than a stick's,
//! from measured builds: twigs across thin gaps and off floors lean 17° or
//! less, props from a pointed tip to a nearby wall 23° to 43°, and the ones
//! that only graze a surface, which read as stray whiskers, start at 48°.
//! The contact bridge applies it, so every automatic path is held to it.

use crate::support_types::shaft_verticality::leans_at_most;
use bread_support_model::types::Segment;

pub const MAX_TWIG_SHAFT_ANGLE_DEG: f64 = 45.0;

/// Whether a built twig stands close enough to plumb to keep.
pub fn is_twig_shaft_vertical_enough(segments: &[Segment]) -> bool {
    leans_at_most(segments, MAX_TWIG_SHAFT_ANGLE_DEG)
}
