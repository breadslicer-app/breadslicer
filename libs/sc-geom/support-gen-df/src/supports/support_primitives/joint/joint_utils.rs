//! DragonFruit `src/supports/SupportPrimitives/Joint/jointUtils.ts`.
//!
//! DragonFruit mints ids with `uuidv4()`; callers pass the generator, so a run
//! can be deterministic. `moveJoint` reads `settings.shaft.maxAngleDeg` and the
//! knots from the store; callers pass both, and the model's collision field
//! where DragonFruit passes the mesh.

use indexmap::IndexMap;

use crate::supports::constants::get_joint_diameter;
use crate::supports::curves::bezier_utils::{get_bezier_point_at_t, subdivide_cubic_bezier};
use crate::supports::curves::curve_utils::update_curves_at_joint;
use crate::supports::placement_logic::collision_avoidance::{
    CollisionField, CollisionFrustumProfile, calculate_safe_offset,
};
use crate::supports::placement_logic::joint_constraint_solver::{
    JointStructure, solve_joint_constraint,
};
use crate::supports::support_primitives::contact_cone::contact_cone_utils::get_final_socket_position;
use crate::supports::support_primitives::contact_disk::contact_disk_utils::calculate_disk_thickness;
use crate::supports::support_primitives::knot::knot_utils::{
    KnotSplitRemap, remap_knot_across_split,
};
use crate::supports::support_primitives::knot::segment_endpoints::{ContactRef, ShaftEntity};
use crate::supports::support_type_registry::{SupportEndpointKind, get_support_type_descriptor};
use crate::supports::types::{
    Branch, Joint, Kickstand, Knot, Roots, Segment, SegmentShape, Stick, Stump, Trunk, Twig, Vec3,
};
use crate::three::Line3;
use crate::three::math_utils::js_max;

/// A shafted entity that can be rebuilt with new segments.
pub trait ShaftedEntity: Clone {
    fn as_shaft(&self) -> ShaftEntity<'_>;
    fn segments_mut(&mut self) -> &mut Vec<Segment>;
}

macro_rules! shafted {
    ($($ty:ident),*) => {$(
        impl ShaftedEntity for $ty {
            fn as_shaft(&self) -> ShaftEntity<'_> {
                ShaftEntity::$ty(self)
            }
            fn segments_mut(&mut self) -> &mut Vec<Segment> {
                &mut self.segments
            }
        }
    )*};
}
shafted!(Trunk, Branch, Twig, Stick, Stump, Kickstand);

fn remap_knots_for_split(
    knots: Option<&IndexMap<String, Knot>>,
    original_segment_id: &str,
    top_segment_id: &str,
    split_t: Option<f64>,
) -> Vec<KnotSplitRemap> {
    let (Some(knots), Some(split_t)) = (knots, split_t) else {
        return Vec::new();
    };
    knots
        .values()
        .filter_map(|knot| {
            remap_knot_across_split(
                knot,
                original_segment_id,
                original_segment_id,
                top_segment_id,
                split_t,
            )
        })
        .collect()
}

struct SplitResult {
    new_segments: Vec<Segment>,
    top_segment_id: String,
}

/// Generic segment splitter that preserves bezier geometry via De Casteljau
/// subdivision and falls back to a linear split. Start and end resolution are
/// supplied by split_support_shaft, which reads them from the registry.
fn split_segment_array(
    segments: &[Segment],
    segment_id: &str,
    split_point: Vec3,
    split_t: Option<f64>,
    resolve_start: &dyn Fn(usize, &Segment) -> Option<Vec3>,
    resolve_end: &dyn Fn(Vec3, &Segment) -> Option<Vec3>,
    new_id: &mut dyn FnMut() -> String,
) -> Option<SplitResult> {
    let seg_index = segments.iter().position(|s| s.id == segment_id)?;
    let original_segment = &segments[seg_index];
    let joint = Joint {
        id: new_id(),
        pos: split_point,
        diameter: get_joint_diameter(original_segment.diameter),
    };

    let splice = |bottom: Segment, top: Segment| {
        let mut new_segments = Vec::with_capacity(segments.len() + 1);
        new_segments.extend_from_slice(&segments[..seg_index]);
        new_segments.push(bottom);
        new_segments.push(top);
        new_segments.extend_from_slice(&segments[seg_index + 1..]);
        new_segments
    };

    if let (SegmentShape::Bezier(curve), Some(split_t)) = (&original_segment.shape, split_t) {
        let start_pos = resolve_start(seg_index, original_segment);
        let end_pos = start_pos.and_then(|start| resolve_end(start, original_segment));
        if let (Some(start_pos), Some(end_pos)) = (start_pos, end_pos) {
            let [left_curve, right_curve] = subdivide_cubic_bezier(
                start_pos,
                curve.control_point1,
                curve.control_point2,
                end_pos,
                split_t,
            );
            let mut bottom_segment = original_segment.clone();
            bottom_segment.top_joint = Some(joint.clone());
            if let Some(bottom_curve) = bottom_segment.bezier_mut() {
                bottom_curve.control_point1 = left_curve[1];
                bottom_curve.control_point2 = left_curve[2];
            }
            let mut top_segment = original_segment.clone();
            top_segment.id = new_id();
            top_segment.bottom_joint = Some(joint.clone());
            top_segment.top_joint = original_segment.top_joint.clone();
            if let Some(top_curve) = top_segment.bezier_mut() {
                top_curve.control_point1 = right_curve[1];
                top_curve.control_point2 = right_curve[2];
            }
            let top_segment_id = top_segment.id.clone();
            return Some(SplitResult {
                new_segments: splice(bottom_segment, top_segment),
                top_segment_id,
            });
        }
    }

    let mut bottom_segment = original_segment.clone();
    bottom_segment.top_joint = Some(joint.clone());
    let mut top_segment = original_segment.clone();
    top_segment.id = new_id();
    top_segment.bottom_joint = Some(joint);
    top_segment.top_joint = original_segment.top_joint.clone();
    let top_segment_id = top_segment.id.clone();
    Some(SplitResult {
        new_segments: splice(bottom_segment, top_segment),
        top_segment_id,
    })
}

/// The host a type's lower endpoint needs, when it needs one.
#[derive(Debug, Clone, Copy, Default)]
pub struct SplitHosts<'a> {
    pub root: Option<&'a Roots>,
    pub host_knot: Option<&'a Knot>,
}

/// Splits a shaft segment on any type, inserting a joint at `split_point`.
/// Start and end come from the declared endpoints; `shaft_fallback` covers
/// what to do when neither resolves.
///
/// An unknown segment leaves the entity unchanged (DragonFruit warns
/// `[JointUtils] Segment not found`).
pub fn split_support_shaft<T: ShaftedEntity>(
    entity: &T,
    segment_id: &str,
    split_point: Vec3,
    split_t: Option<f64>,
    hosts: SplitHosts<'_>,
    knots: Option<&IndexMap<String, Knot>>,
    new_id: &mut dyn FnMut() -> String,
) -> (T, Vec<KnotSplitRemap>) {
    let shaft = entity.as_shaft();
    let descriptor = get_support_type_descriptor(shaft.type_id());
    let stub_length_mm = descriptor.shaft_fallback.stub_length_mm;
    let unresolved = descriptor
        .shaft_fallback
        .start_falls_back_to_split_point
        .then_some(split_point);
    let segments = shaft.segments();

    let resolve_start = |seg_index: usize, seg: &Segment| -> Option<Vec3> {
        // A knot-hosted shaft starts at its host; the others read the joint
        // first, matching resolve_segment_endpoints.
        if descriptor.lower.kind != SupportEndpointKind::Knot
            && let Some(bottom) = &seg.bottom_joint
        {
            return Some(bottom.pos);
        }
        if seg_index == 0 {
            if descriptor.lower.kind == SupportEndpointKind::PlateRoot
                && let Some(root) = hosts.root
            {
                let r_pos = root.transform.pos;
                return Some(Vec3::new(
                    r_pos.x,
                    r_pos.y,
                    r_pos.z + root.disk_height + root.cone_height,
                ));
            }
            if descriptor.lower.kind == SupportEndpointKind::Knot {
                return hosts.host_knot.map(|knot| knot.pos).or(unresolved);
            }
            return seg.bottom_joint.as_ref().map(|j| j.pos).or(unresolved);
        }
        segments[seg_index - 1]
            .top_joint
            .as_ref()
            .map(|j| j.pos)
            .or(unresolved)
    };

    let resolve_end = |start_pos: Vec3, seg: &Segment| -> Option<Vec3> {
        if let Some(top) = &seg.top_joint {
            return Some(top.pos);
        }
        let upper = descriptor.upper;
        if let Some(field) = upper.field
            && matches!(
                upper.kind,
                SupportEndpointKind::Cone | SupportEndpointKind::Disk
            )
            && let Some(ContactRef::Cone(cone)) = shaft.contact(field)
        {
            return Some(get_final_socket_position(cone));
        }
        Some(Vec3::new(
            start_pos.x,
            start_pos.y,
            start_pos.z + stub_length_mm,
        ))
    };

    let Some(res) = split_segment_array(
        segments,
        segment_id,
        split_point,
        split_t,
        &resolve_start,
        &resolve_end,
        new_id,
    ) else {
        return (entity.clone(), Vec::new());
    };

    let mut next = entity.clone();
    *next.segments_mut() = res.new_segments;
    let knot_remaps = remap_knots_for_split(knots, segment_id, &res.top_segment_id, split_t);
    (next, knot_remaps)
}

/// The segment of a trunk closest to `point`, its `t` and the closest point on it.
#[derive(Debug, Clone, PartialEq)]
pub struct ClosestSegment<'a> {
    pub segment: &'a Segment,
    pub t: f64,
    pub point_on_line: Vec3,
}

pub fn find_closest_segment<'a>(
    trunk: &'a Trunk,
    root: &Roots,
    point: Vec3,
) -> Option<ClosestSegment<'a>> {
    // Reconstruct skeleton start
    let start_z_offset = root.disk_height + root.cone_height;

    let mut current_start = Vec3::new(
        root.transform.pos.x,
        root.transform.pos.y,
        root.transform.pos.z + start_z_offset,
    );

    let mut closest_seg: Option<&Segment> = None;
    let mut min_dist = f64::INFINITY;
    let mut best_t = 0.0;
    let mut best_point = Vec3::ZERO;

    for seg in &trunk.segments {
        let end_point = if let Some(top) = &seg.top_joint {
            top.pos
        } else if let Some(cone) = &trunk.contact_cone {
            get_final_socket_position(cone)
        } else {
            current_start.add(Vec3::new(0.0, 0.0, 10.0))
        };

        let dist;
        let closest;
        let t_val;

        if let SegmentShape::Bezier(curve) = &seg.shape {
            // Bezier Segment Projection
            // Approximate by sampling (e.g., 20 steps) or using a robust projector.
            // For snapping, 20-30 steps is plenty fast and accurate enough.
            const STEPS: usize = 30;
            let mut local_min_dist = f64::INFINITY;
            let mut local_best_t = 0.0;
            let mut local_best_point = Vec3::ZERO;

            for i in 0..=STEPS {
                let t = i as f64 / STEPS as f64;
                let p = get_bezier_point_at_t(
                    current_start,
                    curve.control_point1,
                    curve.control_point2,
                    end_point,
                    t,
                );
                let d = p.distance_to(point);
                if d < local_min_dist {
                    local_min_dist = d;
                    local_best_t = t;
                    local_best_point = p;
                }
            }

            dist = local_min_dist;
            closest = local_best_point;
            t_val = local_best_t;
        } else {
            // Straight Segment Projection (Legacy)
            let line = Line3::new(current_start, end_point);
            closest = line.closest_point_to_point(point, true);
            dist = point.distance_to(closest);
            t_val = current_start.distance_to(closest) / current_start.distance_to(end_point);
        }

        if dist < min_dist {
            min_dist = dist;
            closest_seg = Some(seg);
            best_point = closest;
            best_t = t_val;
        }

        current_start = end_point;
    }

    closest_seg.map(|segment| ClosestSegment {
        segment,
        t: best_t,
        point_on_line: best_point,
    })
}

/// What `moveJoint` needs beyond the trunk, joint and position.
#[derive(Clone, Copy, Default)]
pub struct MoveJointContext<'a> {
    /// The model, for collision-based thickness adjustment (DragonFruit's `mesh`).
    pub mesh: Option<&'a dyn CollisionField>,
    /// If true, converts connected segments to Bezier curves and maintains C1 continuity.
    pub is_curve_mode: bool,
    /// The trunk's root (for bezier updates).
    pub root: Option<&'a Roots>,
    /// Start position of the parent chain (Root pos or Knot pos) for constraint calculation.
    pub parent_start_pos: Option<Vec3>,
    pub skip_contact_cone_solve: bool,
    /// `getSettings().shaft.maxAngleDeg`; `None` takes DragonFruit's 80.
    pub max_angle_deg: Option<f64>,
    /// The store's knots, which the constraint solver may read.
    pub knots: Option<&'a IndexMap<String, Knot>>,
}

/// Updates the position of a joint within a trunk.
/// Handles updating all references to the joint (topJoint/bottomJoint) in connected segments.
///
/// Returns a new Trunk with the updated joint. DragonFruit warns
/// `CurveMode active but NO ROOT provided to moveJoint` when curves would be
/// updated without a root, and leaves them.
pub fn move_joint(
    trunk: &Trunk,
    joint_id: &str,
    new_pos: Vec3,
    context: MoveJointContext<'_>,
) -> Trunk {
    let empty_knots = IndexMap::new();
    let skip_contact_cone_solve = context.skip_contact_cone_solve;
    // 0. Apply Shaft Angle Constraints
    let max_angle_deg = context.max_angle_deg.unwrap_or(80.0);

    // Use isolated Constraint Solver
    let clamped_pos = solve_joint_constraint(
        JointStructure::Trunk(trunk),
        joint_id,
        new_pos,
        max_angle_deg,
        context.root,
        context.parent_start_pos,
        context.knots.unwrap_or(&empty_knots),
    );

    // Update newPos to the clamped result
    let new_pos = clamped_pos;

    // Fast path: if constraint solving snaps to the already-committed joint position,
    // skip all downstream cone/curve work to avoid no-op drag churn.
    let mut current_joint_pos: Option<Vec3> = None;
    for seg in &trunk.segments {
        if let Some(top) = seg.top_joint.as_ref().filter(|j| j.id == joint_id) {
            current_joint_pos = Some(top.pos);
            break;
        }
        if let Some(bottom) = seg.bottom_joint.as_ref().filter(|j| j.id == joint_id) {
            current_joint_pos = Some(bottom.pos);
            break;
        }
    }

    if let Some(current) = current_joint_pos {
        let dx = current.x - new_pos.x;
        let dy = current.y - new_pos.y;
        let dz = current.z - new_pos.z;
        let same_pos_eps_sq = 1e-10;
        if (dx * dx) + (dy * dy) + (dz * dz) <= same_pos_eps_sq {
            return trunk.clone();
        }
    }

    // Check if we are moving the Contact Cone's Socket Joint
    let mut new_contact_cone = trunk.contact_cone.clone();

    if let Some(cone) = trunk.contact_cone.as_ref().filter(|c| {
        c.socket_joint_id
            .as_deref()
            .is_some_and(|id| !id.is_empty() && id == joint_id)
    }) {
        let contact_pos = cone.pos;
        let socket_pos = new_pos;

        if skip_contact_cone_solve {
            let axis = Vec3::sub_vectors(socket_pos, contact_pos);
            let axis_len = axis.length();
            if axis_len > 0.001 {
                let mut next = cone.clone();
                next.normal = axis.normalize();
                next.disk_length_override = None;
                next.profile.length_mm = js_max(0.1, axis_len);
                new_contact_cone = Some(next);
            }
        } else {
            // 1. Calculate Vector from Contact (Surface) -> New Socket Position
            let to_socket = Vec3::sub_vectors(socket_pos, contact_pos);
            let total_distance_to_socket = to_socket.length();

            if total_distance_to_socket > 0.001 {
                // ITERATIVE SOLVER for Disk Thickness & Cone Normal
                // Circular Dependency: Thickness -> StartPos -> Normal -> Thickness
                // We iterate to converge on a stable Normal that produces the matching Thickness.
                let surface_normal = cone.surface_normal.unwrap_or(cone.normal);
                let sn = surface_normal;
                let disk_profile = cone.profile.as_disk_profile();

                // Initial guess: Axis = Surface -> Socket
                let mut current_axis = Vec3::sub_vectors(socket_pos, contact_pos).normalize();
                let mut final_normal = current_axis;
                let mut final_length = total_distance_to_socket;
                let mut final_thickness = 0.0;

                // Iterate 3 times (usually converges in 2)
                for _ in 0..3 {
                    let cone_axis = current_axis;

                    // 1. Calculate Thickness using current axis guess
                    let angle_thickness = if cone.profile.is_disk() {
                        calculate_disk_thickness(surface_normal, cone_axis, Some(&disk_profile))
                    } else {
                        0.0
                    };

                    let mut thickness = angle_thickness;

                    // COLLISION CHECK OVERRIDE
                    // If mesh is provided, ensure we have physical clearance.
                    if let Some(mesh) = context.mesh.filter(|_| cone.profile.is_disk()) {
                        let body_radius = cone.profile.body_diameter_mm / 2.0;
                        let safety_margin = 0.8;
                        let test_radius = body_radius + safety_margin;

                        // Legacy Fix: Treat 1.5 as 0.35, else respect user value.
                        // An absent value is `undefined` in DragonFruit, which turns
                        // the search and the thickness into NaN; NaN keeps that.
                        let raw_max = cone.profile.max_standoff_mm.unwrap_or(f64::NAN);
                        let max_standoff = if raw_max == 1.5 { 0.35 } else { raw_max };

                        // Use reusable solver
                        let safe_thickness = calculate_safe_offset(
                            contact_pos,
                            sn,
                            socket_pos,
                            test_radius,
                            mesh,
                            angle_thickness, // minOffset (start at angle-based)
                            max_standoff,
                            0.2,
                            Some(CollisionFrustumProfile {
                                start_radius: (cone.profile.contact_diameter_mm / 2.0)
                                    + safety_margin,
                                end_radius: test_radius,
                                segment_count: None,
                            }),
                        );

                        // If we hit the cap, treat it as "solver couldn't prove clearance" rather than
                        // a real requirement to max-extend the disk.
                        // This prevents snapping to full length on drag end due to false positives.
                        let cap_eps = 1e-6;
                        thickness = if safe_thickness >= (max_standoff - cap_eps) {
                            angle_thickness
                        } else {
                            safe_thickness
                        };
                    }

                    final_thickness = thickness;

                    // 2. Calculate Start Pos
                    let cone_start_pos = contact_pos.add(sn.multiply_scalar(thickness));

                    // 3. Calculate New Axis (Start -> Socket)
                    let cone_vector = Vec3::sub_vectors(socket_pos, cone_start_pos);
                    let len = cone_vector.length();

                    if len > 0.0001 {
                        current_axis = cone_vector.normalize(); // Update for next loop
                        final_normal = current_axis;
                        final_length = js_max(0.1, len);
                    }
                }

                // Only persist a diskLengthOverride when collision avoidance truly required extra standoff.
                // Otherwise, leave it undefined so rendering stays purely angle-based and stable.
                let mut final_axis_for_compare = final_normal;
                if final_axis_for_compare.length_sq() < 0.000001 {
                    final_axis_for_compare = Vec3::new(0.0, 0.0, 1.0);
                }
                final_axis_for_compare = final_axis_for_compare.normalize();

                let angle_thickness_at_final = if cone.profile.is_disk() {
                    calculate_disk_thickness(
                        surface_normal,
                        final_axis_for_compare,
                        Some(&disk_profile),
                    )
                } else {
                    0.0
                };

                const EPS: f64 = 1e-4;
                let should_override =
                    cone.profile.is_disk() && final_thickness > angle_thickness_at_final + EPS;

                let mut next = cone.clone();
                next.normal = final_normal;
                next.disk_length_override = should_override.then_some(final_thickness);
                next.profile.length_mm = final_length;
                new_contact_cone = Some(next);
            }
        }
    }

    // We must iterate segments and update any reference to this joint
    let mut trunk_with_moved_joint = trunk.clone();
    for seg in &mut trunk_with_moved_joint.segments {
        if let Some(top) = seg.top_joint.as_mut().filter(|j| j.id == joint_id) {
            top.pos = new_pos;
        }
        if let Some(bottom) = seg.bottom_joint.as_mut().filter(|j| j.id == joint_id) {
            bottom.pos = new_pos;
        }
    }
    trunk_with_moved_joint.contact_cone = new_contact_cone;

    // UX Improvement: If segments were already bezier, keep them bezier (Sticky Mode)
    // OR if global Curve Mode is active.
    let connected_are_bezier = trunk.segments.iter().any(|s| {
        (s.top_joint.as_ref().is_some_and(|j| j.id == joint_id)
            || s.bottom_joint.as_ref().is_some_and(|j| j.id == joint_id))
            && s.is_bezier()
    });

    let should_update_curves = context.is_curve_mode || connected_are_bezier;

    if should_update_curves && let Some(root) = context.root {
        return update_curves_at_joint(
            &trunk_with_moved_joint,
            joint_id,
            root,
            context.is_curve_mode,
        );
    }

    trunk_with_moved_joint
}
