//! DragonFruit `src/supports/SupportTypes/Twig/twigJointStandoff.ts`.

use crate::supports::support_primitives::contact_cone::types::ContactDiskProfile;
use crate::supports::support_primitives::contact_disk::contact_disk_utils::calculate_disk_thickness;
use crate::supports::types::Vec3;
use crate::three::math_utils::js_max;

// Small extra distance between the joint sphere's surface and the disk's
// contact face — keeps the joint visually separate from the disk and prevents
// any z-fighting on perpendicular placements.
const TWIG_JOINT_SURFACE_CLEARANCE_MM: f64 = 0.05;

/// Twig-only: how far the disk-end joint sits along the disk's surface normal,
/// away from the model contact face.
///
/// At least the angle-based stand-off the disk would normally use, and at
/// least the joint sphere's own radius (plus a small clearance), so a big joint
/// never punches into the model when the disk is large.
pub fn twig_disk_joint_standoff(
    surface_normal: Vec3,
    cone_axis: Vec3,
    profile: &ContactDiskProfile,
    joint_diameter_mm: f64,
) -> f64 {
    let angle_based = calculate_disk_thickness(surface_normal, cone_axis, Some(profile));
    let radius_based = joint_diameter_mm / 2.0 + TWIG_JOINT_SURFACE_CLEARANCE_MM;
    js_max(angle_based, radius_based)
}
