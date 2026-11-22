//! How far a shaft leans off plumb. Sticks and twigs hold their shafts to
//! limits of their own (a thick column and a thin strut tolerate different
//! cants); this is the measurement those limits read.

use crate::geom::math_utils::{js_cos, js_hypot};
use crate::supports::types::Segment;

/// The unsigned cosine between the first segment and the Z axis: 1 for a
/// plumb shaft, 0 for a level one. A shaft with no first segment, a missing
/// end or no length counts as plumb.
pub fn shaft_vertical_cos(segments: &[Segment]) -> f64 {
    let ends = segments.first().and_then(|segment| {
        Some((
            segment.bottom_joint.as_ref()?.pos,
            segment.top_joint.as_ref()?.pos,
        ))
    });
    let Some((bottom, top)) = ends else {
        return 1.0;
    };
    let (dx, dy, dz) = (top.x - bottom.x, top.y - bottom.y, top.z - bottom.z);
    let length = js_hypot(&[dx, dy, dz]);
    if length < 1e-6 {
        1.0
    } else {
        dz.abs() / length
    }
}

/// Whether the shaft leans no more than `max_lean_deg` off plumb.
pub(crate) fn leans_at_most(segments: &[Segment], max_lean_deg: f64) -> bool {
    shaft_vertical_cos(segments) >= js_cos((max_lean_deg * std::f64::consts::PI) / 180.0)
}
