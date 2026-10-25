//! DragonFruit `src/supports/PlacementLogic/JointConstraintSolver.ts`.
//!
//! DragonFruit reads the support store (`getKnotById`, `getSupportEntities`)
//! from module state; here the caller passes the collections it would read.

use indexmap::IndexMap;

use super::shaft_angle_constraint::{Z_UP, clamp_shaft_angle};
use crate::supports::support_primitives::contact_cone::contact_cone_utils::get_final_socket_position;
use crate::supports::support_primitives::contact_cone::types::ContactCone;
use crate::supports::support_type_registry::{SupportTypeId, flexing_knot_host_types};
use crate::supports::types::{Branch, Knot, Roots, Segment, SupportCollections, Trunk, Vec3};
use crate::three::math_utils::rad_to_deg;

const Z_DOWN: Vec3 = Vec3::new(0.0, 0.0, -1.0);

/// `Trunk | Branch`: what `solveJointConstraint` constrains.
#[derive(Debug, Clone, Copy)]
pub enum JointStructure<'a> {
    Trunk(&'a Trunk),
    Branch(&'a Branch),
}

impl<'a> JointStructure<'a> {
    fn segments(&self) -> &'a [Segment] {
        match self {
            JointStructure::Trunk(t) => &t.segments,
            JointStructure::Branch(b) => &b.segments,
        }
    }

    fn contact_cone(&self) -> Option<&'a ContactCone> {
        match self {
            JointStructure::Trunk(t) => t.contact_cone.as_ref(),
            JointStructure::Branch(b) => b.contact_cone.as_ref(),
        }
    }
}

/// `root.coneHeight || 1.5`: zero and NaN fall back, as JavaScript's `||` does.
fn or_default(value: f64, fallback: f64) -> f64 {
    if value == 0.0 || value.is_nan() {
        fallback
    } else {
        value
    }
}

/// Solves the position constraints for a moving joint within a support structure.
/// Enforces shaft angle limits relative to connected segments and the contact cone.
///
/// - `structure` The support structure (Trunk or Branch) containing the joint
/// - `joint_id` The ID of the joint being moved
/// - `candidate_pos` The proposed new position for the joint
/// - `max_angle_deg` The maximum allowed angle from vertical (or parent axis)
/// - `root` Optional root (if structure is a Trunk)
/// - `parent_start_pos` Optional explicit start position (e.g. from parent knot)
/// - `knots` The store's knots, for a branch's parent knot (`getKnotById`)
///
/// Returns the constrained position.
pub fn solve_joint_constraint(
    structure: JointStructure<'_>,
    joint_id: &str,
    candidate_pos: Vec3,
    max_angle_deg: f64,
    root: Option<&Roots>,
    parent_start_pos: Option<Vec3>,
    knots: &IndexMap<String, Knot>,
) -> Vec3 {
    let mut clamped_pos = candidate_pos;

    // Find connected segments
    let segments = structure.segments();
    let bottom_seg_index = segments
        .iter()
        .position(|s| s.top_joint.as_ref().is_some_and(|j| j.id == joint_id));
    let top_seg_index = segments
        .iter()
        .position(|s| s.bottom_joint.as_ref().is_some_and(|j| j.id == joint_id));

    // 1. Bottom Constraint (Vector from Below -> Joint)
    // Prevents the segment below the joint from becoming too horizontal or pointing downwards.
    if let Some(bottom_seg_index) = bottom_seg_index {
        let seg = &segments[bottom_seg_index];
        let mut start: Option<Vec3> = None;

        if let Some(bottom) = &seg.bottom_joint {
            start = Some(bottom.pos);
        } else if let Some(parent_start_pos) = parent_start_pos {
            // Use provided context
            start = Some(parent_start_pos);
        } else if let Some(root) = root.filter(|_| bottom_seg_index == 0) {
            // Fallback if root provided but parentStartPos not
            let r_pos = root.transform.pos;
            let h = or_default(root.cone_height, 1.5) + 0.5;
            start = Some(Vec3::new(r_pos.x, r_pos.y, r_pos.z + h));
        } else if let JointStructure::Branch(branch) = structure
            && bottom_seg_index == 0
        {
            // Fallback: Branch with no parentStartPos provided - Try global store lookup
            if let Some(knot) = knots.get(&branch.parent_knot_id) {
                start = Some(knot.pos);
            }
        }

        if let Some(start) = start {
            let res = clamp_shaft_angle(start, clamped_pos, max_angle_deg, Z_UP);
            if res.is_clamped {
                clamped_pos = res.clamped_pos;
            }
        }
    }

    // 2. Top Constraint (Vector from Above -> Joint)
    // Prevents the segment above the joint from becoming too horizontal or pointing upwards (relative to the top).
    // We treat the joint as the 'Moving End' relative to the 'Fixed Start' (Above).
    // Vector is (Fixed -> Joint). This should point DOWN. Axis = {0,0,-1}.
    if let Some(top_seg_index) = top_seg_index {
        let seg = &segments[top_seg_index];
        // This is the 'Fixed Start' for our vector
        let end = if let Some(top) = &seg.top_joint {
            Some(top.pos)
        } else {
            // If segment ends at the cone
            structure.contact_cone().map(get_final_socket_position)
        };

        if let Some(end) = end {
            let res = clamp_shaft_angle(end, clamped_pos, max_angle_deg, Z_DOWN);
            if res.is_clamped {
                clamped_pos = res.clamped_pos;
            }
        }
    }

    // 3. Contact Cone Constraint (Socket Joint)
    // If we are moving the joint that connects directly to the Contact Cone,
    // we must ensure the cone itself respects the angle constraint.
    if let Some(cone) = structure.contact_cone()
        && cone
            .socket_joint_id
            .as_deref()
            .is_some_and(|id| !id.is_empty() && id == joint_id)
    {
        // Fixed Point: Tip (Contact Pos)
        // Moving Point: Socket Joint (clampedPos)
        // Vector: Tip -> Socket (Should point DOWN, roughly)
        // Axis: {0,0,-1}
        let tip_pos = cone.pos;
        let res = clamp_shaft_angle(tip_pos, clamped_pos, max_angle_deg, Z_DOWN);
        if res.is_clamped {
            clamped_pos = res.clamped_pos;
        }
    }

    clamped_pos
}

/// Which side of its first joint a branch's knot sits on, at drag start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BranchTopology {
    /// Knot Below Joint.
    Up,
    /// Knot Above Joint.
    Down,
}

/// Solves the position constraints for a moving Knot.
/// Ensures that any branches attached to this knot do not exceed the max angle.
///
/// - `knot` The knot being moved
/// - `candidate_pos` The proposed new position for the knot (already projected to parent shaft)
/// - `max_angle_deg` The maximum allowed angle
/// - `topology_map` Optional map of BranchID -> Up (Knot Below Joint) or Down (Knot Above Joint)
/// - `ignored_branch_ids` Optional list of Branch IDs to skip (e.g. handled by ElasticChain solver)
/// - `store` The store's collections (`getSupportEntities`)
///
/// Returns the constrained position.
pub fn solve_knot_constraint(
    knot: &Knot,
    candidate_pos: Vec3,
    max_angle_deg: f64,
    topology_map: Option<&IndexMap<String, BranchTopology>>,
    ignored_branch_ids: Option<&[String]>,
    store: &SupportCollections,
) -> Vec3 {
    let mut clamped_pos = candidate_pos;

    // Find all branches attached to this knot, over every type declaring a
    // flexing host-knot edge.
    let mut attached_branches: Vec<&Branch> = Vec::new();
    for (type_id, knot_fields) in flexing_knot_host_types() {
        assert_eq!(
            type_id,
            SupportTypeId::Branch,
            "a flexing knot host type other than branch needs its own walk"
        );
        for branch in store.branches.values() {
            if knot_fields
                .iter()
                .any(|field| *field == "parentKnotId" && branch.parent_knot_id == knot.id)
            {
                attached_branches.push(branch);
            }
        }
    }

    for branch in attached_branches {
        if ignored_branch_ids.is_some_and(|ids| ids.contains(&branch.base.id)) {
            continue;
        }
        let Some(first_seg) = branch.segments.first() else {
            continue;
        };

        // The branch starts at the Knot and ends at its first joint (or contact socket)
        let branch_end = if let Some(top) = &first_seg.top_joint {
            top.pos
        } else if let Some(cone) = &branch.contact_cone {
            // Branch with single segment ending in cone
            get_final_socket_position(cone)
        } else {
            continue;
        };

        // 0. TOPOLOGY LOCK (Prevent Flip-Over)
        let topology = topology_map
            .and_then(|map| map.get(&branch.base.id))
            .copied();
        if let Some(initial_type) = topology {
            // Hard Limit at Joint Z-Level (+/- epsilon)
            // If initially UP (Knot Below Joint), Knot must stay Below Joint.
            // If initially DOWN (Knot Above Joint), Knot must stay Above Joint.

            // Note: We use a small epsilon to prevent Z-fighting or precision issues at the boundary.
            let limit_z = branch_end.z;
            let epsilon = 0.001;

            if initial_type == BranchTopology::Up {
                // Knot must be BELOW Joint (z < limitZ)
                if clamped_pos.z >= limit_z - epsilon {
                    clamped_pos.z = limit_z - epsilon;
                }
            } else {
                // Knot must be ABOVE Joint (z > limitZ)
                if clamped_pos.z <= limit_z + epsilon {
                    clamped_pos.z = limit_z + epsilon;
                }
            }
        }

        // Calculate Vector from Joint (Fixed) -> Knot (Moving)
        // We use Joint -> Knot because we want to see where the Knot is relative to the Joint.
        let v = Vec3::sub_vectors(clamped_pos, branch_end);

        let len = v.length();
        if len < 0.001 {
            continue; // Coincident, ignore
        }

        // Calculate Angle relative to World UP (0,0,1)
        let angle_rad = v.angle_to(Z_UP);
        let angle_deg = rad_to_deg(angle_rad);

        // Defined Forbidden Zones relative to Vertical:
        // MaxAngle = 80.
        // Valid Up-Pointing Branch (Knot below Joint):
        //    Vector Joint->Knot points DOWN (approx 180 deg).
        //    Valid Range: [180 - MaxAngle, 180] -> [100, 180].
        // Valid Down-Pointing Branch (Knot above Joint):
        //    Vector Joint->Knot points UP (approx 0 deg).
        //    Valid Range: [0, MaxAngle] -> [0, 80].
        // Forbidden Zone: (MaxAngle, 180 - MaxAngle) -> (80, 100).
        let min_valid = max_angle_deg;
        let max_valid = 180.0 - max_angle_deg;

        if angle_deg > min_valid && angle_deg < max_valid {
            // VIOLATION: We are in the horizontal dead zone.
            // Clamp to the NEAREST valid boundary.
            let dist_to_up = (angle_deg - min_valid).abs(); // Distance to 80
            let dist_to_down = (angle_deg - max_valid).abs(); // Distance to 100

            // 1 for UP, -1 for DOWN reference logic
            let ref_axis_id = match topology {
                // Use Topology to decide direction!
                // Down should act like UP Vector (0..80 range); Up like DOWN (100..180).
                Some(BranchTopology::Down) => 1.0,
                Some(BranchTopology::Up) => -1.0,
                // Fallback to proximity if no topology map
                None => {
                    if dist_to_up < dist_to_down {
                        1.0
                    } else {
                        -1.0
                    }
                }
            };

            let ref_axis = Vec3::new(0.0, 0.0, ref_axis_id);
            let res = clamp_shaft_angle(branch_end, clamped_pos, max_angle_deg, ref_axis);

            if res.is_clamped {
                clamped_pos = res.clamped_pos;
            }
        }
    }

    clamped_pos
}
