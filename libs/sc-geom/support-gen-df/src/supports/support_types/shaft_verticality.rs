//! DragonFruit `src/supports/SupportTypes/shaftVerticality.ts`.
//!
//! Shaft-axis geometry shared by the types that gate on it (sticks, twigs).
//! The RULES live with their types, because the cant a 12mm column tolerates is
//! not the cant a 1mm strut tolerates. This is only the measurement they share.

use crate::supports::types::Segment;
use crate::three::math_utils::js_hypot;

/// |cos| of the first segment's deviation from vertical, 1 = perfectly vertical.
pub fn shaft_vertical_cos(segments: &[Segment]) -> f64 {
    let Some(seg) = segments.first() else {
        return 1.0;
    };
    let (Some(a), Some(b)) = (&seg.bottom_joint, &seg.top_joint) else {
        return 1.0;
    };
    let (a, b) = (a.pos, b.pos);
    let vx = b.x - a.x;
    let vy = b.y - a.y;
    let vz = b.z - a.z;
    let len = js_hypot(&[vx, vy, vz]);
    if len < 1e-6 {
        return 1.0;
    }
    vz.abs() / len
}
