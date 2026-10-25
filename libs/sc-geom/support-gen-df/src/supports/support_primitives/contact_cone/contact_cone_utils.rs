//! DragonFruit `src/supports/SupportPrimitives/ContactCone/contactConeUtils.ts`.

use super::types::{ContactCone, DEFAULT_TIP_PROFILE, SupportTipProfile};
use crate::supports::support_primitives::contact_disk::contact_disk_utils::calculate_disk_thickness;
use crate::supports::types::Vec3;
use crate::three::Quaternion;
use crate::three::math_utils::{js_max, js_min};

/// Calculate the socket position (where the cone connects to the joint).
/// The socket is at pos + normal * length.
pub fn get_socket_position(
    contact_pos: Vec3,
    normal: Vec3,
    profile: Option<&SupportTipProfile>,
) -> Vec3 {
    let profile = profile.unwrap_or(&DEFAULT_TIP_PROFILE);
    Vec3::new(
        contact_pos.x + normal.x * profile.length_mm,
        contact_pos.y + normal.y * profile.length_mm,
        contact_pos.z + normal.z * profile.length_mm,
    )
}

/// Calculates the FINAL, effective socket position for a Contact Cone.
/// This accounts for:
/// 1. The base surface position.
/// 2. The primitive offset (Contact Disk thickness) along the surface normal.
/// 3. The cone body length along the cone axis.
pub fn get_final_socket_position(cone: &ContactCone) -> Vec3 {
    let ContactCone {
        pos,
        normal,
        surface_normal,
        profile,
        ..
    } = cone;

    // Fallback surface normal
    let effective_surface_normal = surface_normal.unwrap_or(*normal);

    // 1. Calculate Primitive Thickness (Offset)
    let mut offset = 0.0;
    if profile.is_disk() {
        offset = match cone.disk_length_override {
            Some(length) => length,
            None => calculate_disk_thickness(
                effective_surface_normal,
                *normal,
                Some(&profile.as_disk_profile()),
            ),
        };
    }

    // 2. Calculate Start of Cone Body (offset from surface)
    // The disk pushes OUT along the surface normal.
    let start_pos = Vec3::new(
        pos.x + effective_surface_normal.x * offset,
        pos.y + effective_surface_normal.y * offset,
        pos.z + effective_surface_normal.z * offset,
    );

    // 3. Calculate End of Cone Body (Socket)
    // The cone extends along the cone axis (normal).
    get_socket_position(start_pos, *normal, Some(profile))
}

/// Lowest world Z of the authored body, round tip and (when present) disk.
pub fn get_contact_cone_lowest_z(cone: &ContactCone) -> f64 {
    let ContactCone {
        pos,
        normal,
        profile,
        ..
    } = cone;
    let surface_normal = cone.surface_normal.unwrap_or(*normal);
    let thickness = if profile.is_disk() {
        cone.disk_length_override.unwrap_or_else(|| {
            calculate_disk_thickness(surface_normal, *normal, Some(&profile.as_disk_profile()))
        })
    } else {
        0.0
    };
    let start_z = pos.z + surface_normal.z * thickness;
    let contact_radius = profile.contact_diameter_mm / 2.0;
    let body_radius = profile.body_diameter_mm / 2.0;
    // A circular end face tilted off vertical extends below its centre.
    // The frustum's minimum is attained on one of its two end rims.
    let axis_radial_z = js_max(0.0, 1.0 - normal.z * normal.z).sqrt();
    let mut lowest_z = js_min(
        js_min(
            start_z - contact_radius,
            start_z - contact_radius * axis_radial_z,
        ),
        start_z + normal.z * profile.length_mm - body_radius * axis_radial_z,
    );
    if profile.is_disk() {
        let surface_radial_z = js_max(0.0, 1.0 - surface_normal.z * surface_normal.z).sqrt();
        let disk_back_z = pos.z - surface_normal.z * js_max(0.0, profile.penetration_mm);
        lowest_z = js_min(
            lowest_z,
            js_min(disk_back_z, start_z) - contact_radius * surface_radial_z,
        );
    }
    lowest_z
}

/// Calculate the cone's center position (midpoint between contact and socket).
pub fn get_cone_center_position(
    contact_pos: Vec3,
    normal: Vec3,
    profile: Option<&SupportTipProfile>,
) -> Vec3 {
    let profile = profile.unwrap_or(&DEFAULT_TIP_PROFILE);
    let half_length = profile.length_mm / 2.0;
    Vec3::new(
        contact_pos.x + normal.x * half_length,
        contact_pos.y + normal.y * half_length,
        contact_pos.z + normal.z * half_length,
    )
}

/// Calculate the quaternion to orient the cone.
/// Three.js CylinderGeometry is Y-up by default.
/// We need to rotate so Y aligns with -normal (cone points into model).
pub fn get_cone_quaternion(normal: Vec3) -> Quaternion {
    let up = Vec3::new(0.0, 1.0, 0.0);
    let dir = normal.negate();
    Quaternion::set_from_unit_vectors(up, dir)
}

/// Get the penetration-adjusted contact position.
/// Pushes the contact face slightly into the model along the axis.
pub fn get_penetrated_contact_position(
    contact_pos: Vec3,
    normal: Vec3,
    profile: Option<&SupportTipProfile>,
) -> Vec3 {
    let profile = profile.unwrap_or(&DEFAULT_TIP_PROFILE);
    let pen = profile.penetration_mm;
    Vec3::new(
        contact_pos.x - normal.x * pen,
        contact_pos.y - normal.y * pen,
        contact_pos.z - normal.z * pen,
    )
}
