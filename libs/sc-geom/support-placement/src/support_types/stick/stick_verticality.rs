//! The lean a stick may have and still be kept. A stick bridges two model
//! contacts upright; on sloped surfaces the disks push its sockets sideways,
//! and past this cant it is wedged between the surfaces rather than holding
//! one up, so the build is dropped and the caller tries something else.

use crate::support_types::shaft_verticality::leans_at_most;
use bread_support_model::types::Segment;

/// The steepest lean a stick keeps, in degrees off plumb.
pub const MAX_SHAFT_ANGLE_DEG: f64 = 20.0;

/// Whether a built stick stands close enough to plumb to keep.
pub fn is_shaft_vertical_enough(segments: &[Segment]) -> bool {
    leans_at_most(segments, MAX_SHAFT_ANGLE_DEG)
}
