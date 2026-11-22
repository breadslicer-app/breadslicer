//! How far a twig's end joint stands off the model.

use crate::geom::math_utils::js_max;
use crate::supports::tips::{ContactDiskProfile, nib_length};
use crate::supports::types::Vec3;

/// Air left between the joint ball and the disk's contact face, so the two
/// never share a surface when the disk sits square to the twig.
const JOINT_CLEARANCE_MM: f64 = 0.05;

/// The distance along the surface normal from the contact to the end joint's
/// centre: the disk's own angle-based stand-off, or the joint's radius plus
/// clearance when that is more, so a wide joint never sinks into the model.
pub fn twig_disk_joint_standoff(
    surface_normal: Vec3,
    cone_axis: Vec3,
    profile: &ContactDiskProfile,
    joint_diameter_mm: f64,
) -> f64 {
    js_max(
        nib_length(surface_normal, cone_axis, Some(profile)),
        joint_diameter_mm / 2.0 + JOINT_CLEARANCE_MM,
    )
}
