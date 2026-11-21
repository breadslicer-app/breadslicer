//! DragonFruit `src/supports/SupportPrimitives/Knot/knotUtils.ts`.
//!
//! `knotMoveDescription` (an undo-history label, a translated UI string) is
//! not ported here: it belongs to the history labels, which the GUI owns.

use crate::supports::curves::bezier_utils::get_bezier_point_at_t;
use crate::supports::types::{Knot, Segment, SegmentShape, Vec3};
use crate::geom::Ray;
use crate::geom::math_utils::{js_max, js_min};

/// The closest point on a segment to a pointer ray, and its `t` along the segment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentProjection {
    pub point: Vec3,
    pub t: f64,
}

pub fn project_onto_segment(ray: Ray, start: Vec3, end: Vec3) -> SegmentProjection {
    let point_on_segment = ray.distance_sq_to_segment(start, end).point_on_segment;

    let seg_length = start.distance_to(end);
    let t = if seg_length > 0.0 {
        start.distance_to(point_on_segment) / seg_length
    } else {
        0.0
    };

    SegmentProjection {
        point: point_on_segment,
        t: js_min(1.0, js_max(0.0, t)),
    }
}

/// A minimal patch describing how a knot re-anchors when its host segment is
/// split in two by an inserted joint. `parent_shaft_id` moves to the top segment
/// only when the knot sat above the split; `t` is always rescaled onto whichever
/// half the knot now lives on so its absolute world position is preserved.
#[derive(Debug, Clone, PartialEq)]
pub struct KnotSplitRemap {
    pub knot_id: String,
    pub parent_shaft_id: String,
    pub t: f64,
}

/// Re-anchor a knot across a segment split so it stays at the same world point.
///
/// When a joint is inserted at parametric position `split_t` on the original
/// segment, that segment becomes a bottom half (keeps the original id) and a top
/// half (new id). A knot's stored `t` is expressed against the WHOLE original
/// segment, so after the split it must be converted to the sub-segment it now
/// belongs to. De Casteljau subdivision guarantees the original curve at `t`
/// equals the left sub-curve at `t / splitT` (for t <= splitT) and the right
/// sub-curve at `(t - splitT) / (1 - splitT)` (for t >= splitT). The same
/// reparametrization holds for straight segments (the degenerate cubic case), so
/// one formula covers both.
///
/// Returns `None` when the knot is not attached to the split segment, has no `t`,
/// or the split is too degenerate to remap safely (the caller leaves it as-is).
pub fn remap_knot_across_split(
    knot: &Knot,
    original_segment_id: &str,
    bottom_segment_id: &str,
    top_segment_id: &str,
    split_t: f64,
) -> Option<KnotSplitRemap> {
    if knot.parent_shaft_id != original_segment_id {
        return None;
    }
    let knot_t = knot.t?;
    const EPS: f64 = 1e-6;
    // DragonFruit's `!(splitT > EPS) || !(splitT < 1 - EPS)`, which also rejects NaN.
    if split_t.is_nan() || split_t <= EPS || split_t >= 1.0 - EPS {
        return None;
    }
    let t = js_min(1.0, js_max(0.0, knot_t));
    if t <= split_t {
        return Some(KnotSplitRemap {
            knot_id: knot.id.clone(),
            parent_shaft_id: bottom_segment_id.to_owned(),
            t: t / split_t,
        });
    }
    Some(KnotSplitRemap {
        knot_id: knot.id.clone(),
        parent_shaft_id: top_segment_id.to_owned(),
        t: (t - split_t) / (1.0 - split_t),
    })
}

/// Calculate the position of a knot along a segment using its t parameter (0-1).
/// This is used to update knot positions when the parent segment moves.
pub fn calculate_knot_position_from_t(start: Vec3, end: Vec3, t: f64) -> Vec3 {
    let clamped_t = js_min(1.0, js_max(0.0, t));
    Vec3::new(
        start.x + (end.x - start.x) * clamped_t,
        start.y + (end.y - start.y) * clamped_t,
        start.z + (end.z - start.z) * clamped_t,
    )
}

/// Calculate the position of a knot along a specific segment.
/// For straight segments we interpolate between endpoints.
/// For bezier segments we evaluate the cubic curve using the segment control points.
pub fn calculate_knot_position_on_segment_from_t(
    start: Vec3,
    end: Vec3,
    segment: &Segment,
    t: f64,
) -> Vec3 {
    let clamped_t = js_min(1.0, js_max(0.0, t));

    if let SegmentShape::Bezier(curve) = &segment.shape {
        return get_bezier_point_at_t(
            start,
            curve.control_point1,
            curve.control_point2,
            end,
            clamped_t,
        );
    }

    calculate_knot_position_from_t(start, end, clamped_t)
}
