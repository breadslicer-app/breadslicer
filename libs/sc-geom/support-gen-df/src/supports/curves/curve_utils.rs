//! DragonFruit `src/supports/Curves/curveUtils.ts`.
//!
//! Two behaviours of the TypeScript are kept as they show in the result and
//! documented where they cannot be:
//!
//! - `updateCurvesAtJoint` mutates a segment that is already bezier in place
//!   (`ensureBezierSegment` returns the same object). The returned trunk sees
//!   every such write, including an `endTangent`/`startTangent` written before
//!   an unresolvable endpoint skipped the control points. The caller's own
//!   trunk object is not mutated here: Rust trunks are values.
//! - `removeSegmentCurve`/`removeCurveAtJoint` leave the segment's `bias` on the
//!   straight result (it is not in the destructured list). Nothing reads it, but
//!   it is kept (`SegmentShape::Straight::stray_bias`) so the record matches.

use super::bezier_utils::calculate_bezier_control_points;
use crate::supports::support_primitives::contact_cone::contact_cone_utils::get_final_socket_position;
use crate::supports::types::{BezierCurve, Roots, Segment, SegmentShape, Trunk, Vec3};

/// `x || 0` for a number: zero and NaN become the fallback.
fn or_zero(value: f64) -> f64 {
    if value.is_nan() { 0.0 } else { value }
}

/// Converts a segment to a BezierSegment if it isn't already.
/// Preserves existing properties.
pub fn ensure_bezier_segment(segment: &Segment) -> Segment {
    if segment.is_bezier() {
        return segment.clone();
    }

    // Initialize with default Bezier properties
    let mut bezier = segment.clone();
    bezier.shape = SegmentShape::Bezier(BezierCurve {
        control_point1: Vec3::ZERO, // Will be calculated
        control_point2: Vec3::ZERO,
        start_tangent: Vec3::new(0.0, 0.0, 1.0),
        end_tangent: Vec3::new(0.0, 0.0, 1.0),
        tension: 0.5,
        bias: 0.5,
        resolution: 16.0,
    });
    bezier
}

/// Helper to get start/end positions for a segment
fn get_segment_end_points(trunk: &Trunk, seg: &Segment, root: &Roots) -> Option<(Vec3, Vec3)> {
    // Start
    let mut start: Option<Vec3> = None;
    if let Some(bottom) = &seg.bottom_joint {
        start = Some(bottom.pos);
    } else if trunk.segments[0].id == seg.id {
        // Root
        let r_pos = root.transform.pos;
        let r_height = or_zero(root.disk_height) + or_zero(root.cone_height);
        start = Some(Vec3::new(r_pos.x, r_pos.y, r_pos.z + r_height));
    } else {
        // Fallback: find previous segment
        let idx = trunk.segments.iter().position(|s| s.id == seg.id);
        if let Some(idx) = idx.filter(|&i| i > 0) {
            start = trunk.segments[idx - 1].top_joint.as_ref().map(|j| j.pos);
        }
    }

    // End
    let end = if let Some(top) = &seg.top_joint {
        Some(top.pos)
    } else if let Some(cone) = &trunk.contact_cone {
        Some(get_final_socket_position(cone))
    } else {
        // Tip? Fallback
        start.map(|s| Vec3::new(s.x, s.y, s.z + 10.0))
    };

    Some((start?, end?))
}

/// Updates tension for a specific segment and recalculates its curve.
pub fn update_segment_tension(
    trunk: &Trunk,
    segment_id: &str,
    tension: f64,
    root: &Roots,
) -> Trunk {
    let Some(seg_index) = trunk.segments.iter().position(|s| s.id == segment_id) else {
        return trunk.clone();
    };

    let old_seg = &trunk.segments[seg_index];
    let Some(curve) = old_seg.bezier() else {
        return trunk.clone();
    };

    let Some((start, end)) = get_segment_end_points(trunk, old_seg, root) else {
        return trunk.clone();
    };

    // Use existing tangents
    let [cp1, cp2] = calculate_bezier_control_points(
        start,
        end,
        curve.start_tangent,
        curve.end_tangent,
        tension,
        curve.bias,
    );

    let mut new_trunk = trunk.clone();
    let new_curve = new_trunk.segments[seg_index]
        .bezier_mut()
        .expect("checked bezier above");
    new_curve.tension = tension;
    new_curve.control_point1 = cp1;
    new_curve.control_point2 = cp2;
    new_trunk
}

/// Updates bias for a specific segment and recalculates its curve.
pub fn update_segment_bias(trunk: &Trunk, segment_id: &str, bias: f64, root: &Roots) -> Trunk {
    let Some(seg_index) = trunk.segments.iter().position(|s| s.id == segment_id) else {
        return trunk.clone();
    };

    let old_seg = &trunk.segments[seg_index];
    let Some(curve) = old_seg.bezier() else {
        return trunk.clone();
    };

    let Some((start, end)) = get_segment_end_points(trunk, old_seg, root) else {
        return trunk.clone();
    };

    let [cp1, cp2] = calculate_bezier_control_points(
        start,
        end,
        curve.start_tangent,
        curve.end_tangent,
        curve.tension,
        bias,
    );

    let mut new_trunk = trunk.clone();
    let new_curve = new_trunk.segments[seg_index]
        .bezier_mut()
        .expect("checked bezier above");
    new_curve.bias = bias;
    new_curve.control_point1 = cp1;
    new_curve.control_point2 = cp2;
    new_trunk
}

/// The straight segment a curve reverts to (`{ ...base, type: 'straight' }`),
/// keeping the `bias` the destructuring leaves in `base`.
fn straightened(segment: &Segment) -> Segment {
    let mut straight = segment.clone();
    straight.shape = SegmentShape::Straight {
        explicit: true,
        stray_bias: segment.bezier().map(|c| c.bias),
    };
    straight
}

/// Removes curve from a specific segment (reverts to straight).
pub fn remove_segment_curve(trunk: &Trunk, segment_id: &str) -> Trunk {
    let Some(seg_index) = trunk.segments.iter().position(|s| s.id == segment_id) else {
        return trunk.clone();
    };
    if !trunk.segments[seg_index].is_bezier() {
        return trunk.clone();
    }
    let mut new_trunk = trunk.clone();
    new_trunk.segments[seg_index] = straightened(&trunk.segments[seg_index]);
    new_trunk
}

/// Updates the curves of segments connected to a specific joint.
/// Ensures C1 continuity at the joint. DragonFruit's default `force_curve` is false.
pub fn update_curves_at_joint(
    trunk: &Trunk,
    joint_id: &str,
    root: &Roots,
    force_curve: bool,
) -> Trunk {
    let touches = |s: &Segment| {
        s.top_joint.as_ref().is_some_and(|j| j.id == joint_id)
            || s.bottom_joint.as_ref().is_some_and(|j| j.id == joint_id)
    };

    // 1. Find segments connected to this joint
    if !trunk.segments.iter().any(touches) {
        return trunk.clone();
    }

    // 2. We need to look at the local neighborhood to calculate smooth tangents.

    // Find segment where joint is TOP (incoming)
    let incoming_idx = trunk
        .segments
        .iter()
        .position(|s| s.top_joint.as_ref().is_some_and(|j| j.id == joint_id));

    // Find segment where joint is BOTTOM (outgoing)
    let mut outgoing_idx = trunk
        .segments
        .iter()
        .position(|s| s.bottom_joint.as_ref().is_some_and(|j| j.id == joint_id));

    // Fallback
    if let Some(incoming) = incoming_idx
        && outgoing_idx.is_none()
        && incoming + 1 < trunk.segments.len()
    {
        outgoing_idx = Some(incoming + 1);
    }

    // The segments are edited in place on this copy, so a write to a segment
    // that is already bezier is visible however the update ends.
    let mut new_segments = trunk.segments.clone();

    // Helper to get position of a joint or end
    let get_top = |seg: &Segment| -> Option<Vec3> {
        if let Some(top) = &seg.top_joint {
            return Some(top.pos);
        }
        trunk.contact_cone.as_ref().map(get_final_socket_position)
    };
    let get_bottom = |seg: &Segment| -> Option<Vec3> {
        if let Some(bottom) = &seg.bottom_joint {
            return Some(bottom.pos);
        }
        if trunk.segments[0].id == seg.id {
            let r_pos = root.transform.pos;
            let disk_height = or_zero(root.disk_height);
            let cone_height = or_zero(root.cone_height);
            return Some(Vec3::new(
                r_pos.x,
                r_pos.y,
                r_pos.z + disk_height + cone_height,
            ));
        }
        None
    };

    // Calculate Smooth Tangent at the Joint
    let joint_pos = match incoming_idx {
        Some(i) => {
            trunk.segments[i]
                .top_joint
                .as_ref()
                .expect("incoming has the joint on top")
                .pos
        }
        None => {
            trunk.segments[outgoing_idx.expect("a connected segment")]
                .bottom_joint
                .as_ref()
                .expect("outgoing has the joint at the bottom")
                .pos
        }
    };
    let mut tangent = Vec3::new(0.0, 0.0, 1.0);

    let incoming_seg = incoming_idx.map(|i| &trunk.segments[i]);
    let outgoing_seg = outgoing_idx.map(|i| &trunk.segments[i]);

    match (incoming_seg, outgoing_seg) {
        (Some(incoming), Some(outgoing)) => {
            let p_prev = get_bottom(incoming);
            let p_next = get_top(outgoing);

            if let (Some(p_prev), Some(p_next)) = (p_prev, p_next) {
                let v1 = joint_pos.sub(p_prev).normalize();
                let v2 = p_next.sub(joint_pos).normalize();
                tangent = v1.add(v2).normalize();
            } else if let Some(p_prev) = p_prev {
                tangent = joint_pos.sub(p_prev).normalize();
            } else if let Some(p_next) = p_next {
                tangent = p_next.sub(joint_pos).normalize();
            }
        }
        (Some(incoming), None) => {
            if let Some(p_prev) = get_bottom(incoming) {
                tangent = joint_pos.sub(p_prev).normalize();
            }
        }
        (None, Some(outgoing)) => {
            if let Some(p_next) = get_top(outgoing) {
                tangent = p_next.sub(joint_pos).normalize();
            }
        }
        (None, None) => {}
    }

    let tangent_vec = tangent;
    let has_direction = |t: Vec3| t.x.abs() > 0.001 || t.y.abs() > 0.001 || t.z.abs() > 0.001;

    // Update Incoming Segment
    if let Some(incoming) = incoming_seg
        && (incoming.is_bezier() || force_curve)
    {
        let idx = new_segments
            .iter()
            .position(|s| s.id == incoming.id)
            .expect("incoming is in the trunk");
        let was_bezier = incoming.is_bezier();
        let mut seg = ensure_bezier_segment(&new_segments[idx]);
        seg.bezier_mut().expect("ensured bezier").end_tangent = tangent_vec;

        if let Some(start_pos) = get_bottom(&seg) {
            let curve = seg.bezier_mut().expect("ensured bezier");
            let dir = joint_pos.sub(start_pos).normalize();
            let final_start_tangent = if has_direction(curve.start_tangent) {
                curve.start_tangent
            } else {
                dir
            };

            let [cp1, cp2] = calculate_bezier_control_points(
                start_pos,
                joint_pos,
                final_start_tangent,
                tangent_vec,
                curve.tension,
                curve.bias,
            );

            curve.control_point1 = cp1;
            curve.control_point2 = cp2;
            curve.start_tangent = final_start_tangent;
            new_segments[idx] = seg;
        } else if was_bezier {
            // The tangent was written onto the shared object.
            new_segments[idx] = seg;
        }
    }

    // Update Outgoing Segment
    if let Some(outgoing) = outgoing_seg
        && (outgoing.is_bezier() || force_curve)
    {
        let idx = new_segments
            .iter()
            .position(|s| s.id == outgoing.id)
            .expect("outgoing is in the trunk");
        let was_bezier = outgoing.is_bezier();
        let mut seg = ensure_bezier_segment(&new_segments[idx]);
        seg.bezier_mut().expect("ensured bezier").start_tangent = tangent_vec;

        if let Some(end_pos) = get_top(&seg) {
            let curve = seg.bezier_mut().expect("ensured bezier");
            let dir = end_pos.sub(joint_pos).normalize();
            let final_end_tangent = if has_direction(curve.end_tangent) {
                curve.end_tangent
            } else {
                dir
            };

            let [cp1, cp2] = calculate_bezier_control_points(
                joint_pos,
                end_pos,
                tangent_vec,
                final_end_tangent,
                curve.tension,
                curve.bias,
            );

            curve.control_point1 = cp1;
            curve.control_point2 = cp2;
            curve.end_tangent = final_end_tangent;
            new_segments[idx] = seg;
        } else if was_bezier {
            new_segments[idx] = seg;
        }
    }

    Trunk {
        segments: new_segments,
        ..trunk.clone()
    }
}

/// Updates tension for curves connected to a joint
pub fn update_curve_tension(trunk: &Trunk, joint_id: &str, tension: f64, root: &Roots) -> Trunk {
    let mut temp_trunk = trunk.clone();
    for seg in &mut temp_trunk.segments {
        let touches = seg.top_joint.as_ref().is_some_and(|j| j.id == joint_id)
            || seg.bottom_joint.as_ref().is_some_and(|j| j.id == joint_id);
        if touches && let Some(curve) = seg.bezier_mut() {
            curve.tension = tension;
        }
    }
    update_curves_at_joint(&temp_trunk, joint_id, root, false)
}

/// Removes curve properties from segments connected to a joint, reverting them to straight lines.
pub fn remove_curve_at_joint(trunk: &Trunk, joint_id: &str) -> Trunk {
    let mut new_trunk = trunk.clone();
    for seg in &mut new_trunk.segments {
        let touches = seg.top_joint.as_ref().is_some_and(|j| j.id == joint_id)
            || seg.bottom_joint.as_ref().is_some_and(|j| j.id == joint_id);
        if touches && seg.is_bezier() {
            *seg = straightened(seg);
        }
    }
    new_trunk
}
