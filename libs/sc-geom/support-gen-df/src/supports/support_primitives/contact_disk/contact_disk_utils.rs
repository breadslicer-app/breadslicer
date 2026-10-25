//! DragonFruit `src/supports/SupportPrimitives/ContactDisk/contactDiskUtils.ts`.

use crate::supports::support_primitives::contact_cone::types::ContactDiskProfile;
use crate::supports::types::Vec3;
use crate::three::Quaternion;
use crate::three::math_utils::{js_acos, js_max, js_min};

const DEFAULT_MIN_DISK_THICKNESS_MM: f64 = 0.1;
const DEFAULT_STANDOFF_ANGLE_THRESHOLD_RAD: f64 = std::f64::consts::PI / 4.0;
const DEFAULT_LEGACY_MAX_STANDOFF_MM: f64 = 1.5;
const DEFAULT_LEGACY_CLAMPED_MAX_STANDOFF_MM: f64 = 0.35;
/// ~81°
const MAX_STANDOFF_ANGLE_RAD: f64 = std::f64::consts::PI * 0.5 * 0.9;
const EPS: f64 = 1e-8;

fn clamp01(value: f64) -> f64 {
    js_max(0.0, js_min(1.0, value))
}

/// Calculates the thickness of the contact disk ("nib") based on cone angle relative to surface.
///
/// Logic:
/// - Ideally perpendicular (Cone Axis aligned with Surface Normal) -> Min thickness.
/// - Steeper angle -> Thicker disk to prevent cone body from clipping into wall.
/// - Never shorter than the round tip's radius: the ball is centered on the nib's
///   top face, so a shorter nib leaves the ball's underside below the flat contact
///   face, poking out through it into the model. Only the nib's diameter used to
///   follow the contact diameter, so that gap grew with the contact diameter —
///   the floor grows the nib in length with it. Matches the twig rule
///   (`twigDiskJointStandoff`), which keeps its joint sphere off the model.
///
/// `cone_axis` is the direction the cone is pointing (usually towards the socket).
/// A `None` profile is DragonFruit's legacy safety fallback.
pub fn calculate_disk_thickness(
    surface_normal: Vec3,
    cone_axis: Vec3,
    profile: Option<&ContactDiskProfile>,
) -> f64 {
    // SAFETY CHECK: Fallback for legacy profiles or missing props
    let Some(profile) = profile else {
        return DEFAULT_MIN_DISK_THICKNESS_MM;
    };

    let threshold = profile
        .standoff_angle_threshold
        .unwrap_or(DEFAULT_STANDOFF_ANGLE_THRESHOLD_RAD);
    // The round tip's own radius is a floor on the nib: the ball's underside is
    // inside the nib only while the nib is at least this tall.
    let min_thickness = js_max(
        profile
            .disk_thickness_mm
            .unwrap_or(DEFAULT_MIN_DISK_THICKNESS_MM),
        js_max(0.0, profile.contact_diameter_mm.unwrap_or(0.0)) / 2.0,
    );

    // SMART LEGACY FIX:
    // If maxStandoff is exactly 1.5 (old default), clamp it to 0.35.
    // If it's anything else (user customized), use it directly.
    let raw_max = profile
        .max_standoff_mm
        .unwrap_or(DEFAULT_LEGACY_MAX_STANDOFF_MM);
    let max_standoff = if raw_max == DEFAULT_LEGACY_MAX_STANDOFF_MM {
        DEFAULT_LEGACY_CLAMPED_MAX_STANDOFF_MM
    } else {
        raw_max
    };
    let max_thickness = js_max(min_thickness, max_standoff);

    let (nx, ny, nz) = (surface_normal.x, surface_normal.y, surface_normal.z);
    let (ax, ay, az) = (cone_axis.x, cone_axis.y, cone_axis.z);

    let n_len_sq = nx * nx + ny * ny + nz * nz;
    let a_len_sq = ax * ax + ay * ay + az * az;

    if n_len_sq < EPS || a_len_sq < EPS {
        return min_thickness;
    }

    // Angle between Surface Normal and Cone Axis
    // 0 = Perfectly Perpendicular (Cone pointing straight out)
    // 90 = Parallel to surface (Bad)
    let inv_mag = 1.0 / (n_len_sq * a_len_sq).sqrt();
    let dot = (nx * ax + ny * ay + nz * az) * inv_mag;
    let clamped_dot = js_max(-1.0, js_min(1.0, dot));
    let angle = js_acos(clamped_dot);

    if angle <= threshold {
        return min_thickness;
    }

    // Interpolate
    let max_angle = MAX_STANDOFF_ANGLE_RAD;

    // Clamp angle
    let effective_angle = js_min(angle, max_angle);

    let denom = js_max(EPS, max_angle - threshold);
    let t = (effective_angle - threshold) / denom;
    let factor = clamp01(t);

    // Lerp
    min_thickness + factor * (max_thickness - min_thickness)
}

/// Get the center position for the disk.
/// The disk is centered at: pos + (normal * thickness/2)
pub fn get_disk_center(pos: Vec3, normal: Vec3, thickness: f64) -> Vec3 {
    Vec3::new(
        pos.x + normal.x * (thickness / 2.0),
        pos.y + normal.y * (thickness / 2.0),
        pos.z + normal.z * (thickness / 2.0),
    )
}

/// Get Quaternion to align cylinder Y-axis with Normal.
pub fn get_disk_rotation(normal: Vec3) -> Quaternion {
    let align_vector = normal.normalize();
    // Cylinder default axis
    let default_up = Vec3::new(0.0, 1.0, 0.0);
    Quaternion::set_from_unit_vectors(default_up, align_vector)
}
