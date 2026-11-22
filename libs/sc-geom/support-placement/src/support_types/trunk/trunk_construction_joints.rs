//! The joint a straight trunk gets partway up its shaft.

use support_geom::geom::math_utils::{js_max, js_min};
use support_model::types::Vec3;

/// The shortest segment left below the joint, in mm.
const MIN_LOWER_SEGMENT_MM: f64 = 1.0;
/// The shortest segment left above the joint, in mm.
const MIN_UPPER_SEGMENT_MM: f64 = 0.5;
/// How far up the rise the joint goes, when it fits.
const JOINT_HEIGHT_SHARE: f64 = 0.65;

/// The joint 65% of the way from the root's top to the socket, above the
/// base; none when the rise leaves no room for both segments around it.
fn mid_shaft_joint(base_pos: Vec3, root_top_z: f64, socket_pos: Vec3) -> Vec<Vec3> {
    let rise = socket_pos.z - root_top_z;
    if rise <= MIN_UPPER_SEGMENT_MM + MIN_LOWER_SEGMENT_MM {
        return Vec::new();
    }
    let lowest = root_top_z + MIN_LOWER_SEGMENT_MM;
    let highest = socket_pos.z - MIN_UPPER_SEGMENT_MM;
    let z = js_max(
        lowest,
        js_min(root_top_z + rise * JOINT_HEIGHT_SHARE, highest),
    );
    vec![Vec3::new(base_pos.x, base_pos.y, z)]
}

/// The construction joints a trunk is built with: the ones given, or, for a
/// trunk with no route bends either, the mid-shaft joint.
pub fn normalize_first_construction_joint(
    base_pos: Vec3,
    root_top_z: f64,
    socket_pos: Vec3,
    route_joints: &[Vec3],
    construction_joints: &[Vec3],
) -> Vec<Vec3> {
    if !construction_joints.is_empty() {
        construction_joints.to_vec()
    } else if route_joints.is_empty() {
        mid_shaft_joint(base_pos, root_top_z, socket_pos)
    } else {
        Vec::new()
    }
}
