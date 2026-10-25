//! DragonFruit `src/supports/SupportTypes/Trunk/trunkConstructionJoints.ts`.

use crate::supports::types::Vec3;
use crate::three::math_utils::{js_max, js_min};

const MIN_INSERTED_BASE_SEGMENT_MM: f64 = 1.0;
const MIN_INSERTED_TRANSITION_SEGMENT_MM: f64 = 0.5;

/// The joint a straight support gets 65% of the way up its rise, if it has room.
fn central_joint(base_pos: Vec3, root_top_z: f64, socket_pos: Vec3) -> Vec<Vec3> {
    let available_rise = socket_pos.z - root_top_z;
    if available_rise <= MIN_INSERTED_TRANSITION_SEGMENT_MM + MIN_INSERTED_BASE_SEGMENT_MM {
        return Vec::new();
    }

    let min_joint_z = root_top_z + MIN_INSERTED_BASE_SEGMENT_MM;
    let max_joint_z = socket_pos.z - MIN_INSERTED_TRANSITION_SEGMENT_MM;
    let preferred_joint_z = root_top_z + available_rise * 0.65;
    let joint_z = js_max(min_joint_z, js_min(preferred_joint_z, max_joint_z));

    vec![Vec3::new(base_pos.x, base_pos.y, joint_z)]
}

pub fn with_central_straight_support_joint(
    base_pos: Vec3,
    root_top_z: f64,
    socket_pos: Vec3,
) -> Vec<Vec3> {
    central_joint(base_pos, root_top_z, socket_pos)
}

pub fn normalize_first_construction_joint(
    base_pos: Vec3,
    root_top_z: f64,
    socket_pos: Vec3,
    route_joints: &[Vec3],
    construction_joints: &[Vec3],
) -> Vec<Vec3> {
    if !construction_joints.is_empty() {
        return construction_joints.to_vec();
    }

    if !route_joints.is_empty() {
        return Vec::new();
    }

    central_joint(base_pos, root_top_z, socket_pos)
}
