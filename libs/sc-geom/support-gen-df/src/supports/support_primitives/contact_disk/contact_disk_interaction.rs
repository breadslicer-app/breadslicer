//! DragonFruit `src/supports/SupportPrimitives/ContactDisk/ContactDiskInteraction.ts`.
//!
//! `collision_mesh` becomes the model's [`CollisionField`]; see
//! `placement_logic::collision_avoidance`.

use super::contact_disk_utils::calculate_disk_thickness;
use crate::supports::placement_logic::collision_avoidance::{
    CollisionField, CollisionFrustumProfile, calculate_safe_offset,
};
use crate::supports::support_primitives::contact_cone::contact_cone_utils::get_final_socket_position;
use crate::supports::support_primitives::contact_cone::types::ContactCone;
use crate::supports::types::{ContactDisk, Vec3};
use crate::three::math_utils::js_max;

// Contact cones need to respect the model, but they can sit much closer than
// shafts/roots without causing the oversized detours that a full 0.8mm margin induces.
const CONTACT_CONE_COLLISION_SAFETY_MM: f64 = 0.3;
const LEGACY_MAX_STANDOFF_MM: f64 = 1.5;
const LEGACY_CLAMPED_MAX_STANDOFF_MM: f64 = 0.35;

fn resolve_collision_aware_disk_thickness(
    cone: &ContactCone,
    surface_normal: Vec3,
    cone_axis: Vec3,
    socket_target: Vec3,
    collision_mesh: Option<&dyn CollisionField>,
) -> f64 {
    let angle_thickness = if cone.profile.is_disk() {
        calculate_disk_thickness(
            surface_normal,
            cone_axis,
            Some(&cone.profile.as_disk_profile()),
        )
    } else {
        0.0
    };

    let Some(collision_mesh) = collision_mesh.filter(|_| cone.profile.is_disk()) else {
        return angle_thickness;
    };

    let body_radius = cone.profile.body_diameter_mm / 2.0;
    let raw_max_standoff = cone
        .profile
        .max_standoff_mm
        .unwrap_or(LEGACY_MAX_STANDOFF_MM);
    let max_standoff = if raw_max_standoff == LEGACY_MAX_STANDOFF_MM {
        LEGACY_CLAMPED_MAX_STANDOFF_MM
    } else {
        raw_max_standoff
    };

    if max_standoff <= angle_thickness + 0.000001 {
        return angle_thickness;
    }

    let safe_thickness = calculate_safe_offset(
        cone.pos,
        surface_normal,
        socket_target,
        body_radius + CONTACT_CONE_COLLISION_SAFETY_MM,
        collision_mesh,
        angle_thickness,
        max_standoff,
        0.2,
        Some(CollisionFrustumProfile {
            start_radius: (cone.profile.contact_diameter_mm / 2.0)
                + CONTACT_CONE_COLLISION_SAFETY_MM,
            end_radius: body_radius + CONTACT_CONE_COLLISION_SAFETY_MM,
            segment_count: None,
        }),
    );

    let cap_eps = 1e-6;
    if safe_thickness >= (max_standoff - cap_eps) {
        angle_thickness
    } else {
        safe_thickness
    }
}

pub fn recompute_contact_cone_for_moved_disk(
    cone: &ContactCone,
    next_contact_pos: Vec3,
    next_surface_normal: Vec3,
    fixed_socket_pos: Option<Vec3>,
    collision_mesh: Option<&dyn CollisionField>,
) -> ContactCone {
    let socket_target = fixed_socket_pos.unwrap_or_else(|| get_final_socket_position(cone));

    let contact_pos = next_contact_pos;
    let mut surface_normal = next_surface_normal;
    if surface_normal.length_sq() < 0.000001 {
        surface_normal = Vec3::new(0.0, 0.0, 1.0);
    }
    surface_normal = surface_normal.normalize();

    // Pass 1: approximate axis from contact point to socket for disk thickness calc
    let mut approx_axis = socket_target.sub(contact_pos);
    if approx_axis.length_sq() < 0.000001 {
        approx_axis = cone.normal;
    }
    if approx_axis.length_sq() < 0.000001 {
        approx_axis = surface_normal;
    }
    approx_axis = approx_axis.normalize();

    let thickness = resolve_collision_aware_disk_thickness(
        cone,
        surface_normal,
        approx_axis,
        socket_target,
        collision_mesh,
    );

    // Cone body starts after the disk offset along surface normal
    let cone_start = contact_pos.add(surface_normal.multiply_scalar(thickness));

    // Pass 2: final axis from coneStart to socket (matches how the renderer applies it)
    let mut final_axis = socket_target.sub(cone_start);
    if final_axis.length_sq() < 0.000001 {
        final_axis = approx_axis;
    }
    let length_mm = js_max(0.05, final_axis.length());
    final_axis = final_axis.normalize();

    let mut next = cone.clone();
    next.pos = next_contact_pos;
    next.surface_normal = Some(next_surface_normal);
    next.normal = final_axis;
    next.profile.length_mm = length_mm;
    next.disk_length_override = Some(thickness);
    next
}

pub fn move_disk_keeping_cone_connection(
    disk: &ContactDisk,
    next_contact_pos: Vec3,
    next_surface_normal: Vec3,
) -> ContactDisk {
    let mut surface_normal = next_surface_normal;
    if surface_normal.length_sq() < 0.000001 {
        surface_normal = Vec3::new(0.0, 0.0, 1.0);
    }
    surface_normal = surface_normal.normalize();

    let mut tip_axis = disk.cone_axis;
    if tip_axis.length_sq() < 0.000001 {
        tip_axis = surface_normal;
    }
    tip_axis = tip_axis.normalize();

    ContactDisk {
        pos: next_contact_pos,
        surface_normal,
        cone_axis: tip_axis,
        disk_length_override: None,
        ..disk.clone()
    }
}
