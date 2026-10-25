//! DragonFruit `src/supports/PlacementLogic/ShaftAngleConstraint.ts`.
//!
//! Angle limit from vertical (0 degrees = vertical, 90 degrees = horizontal).
//! We want to prevent supports from being too flat (horizontal) or pointing upwards (negative).
//! Max 80 degrees means the support must be at least 10 degrees away from horizontal.
//! Note: This default is now 80, but callers should usually provide the user setting.

use crate::supports::types::Vec3;
use crate::three::math_utils::{js_acos, js_cos, js_max, js_min, js_sin};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShaftClampResult {
    pub clamped_pos: Vec3,
    pub is_clamped: bool,
    pub angle_deg: f64,
}

/// DragonFruit's defaults: `maxAngleDeg = 80`, `referenceAxis = { 0, 0, 1 }`.
pub const DEFAULT_MAX_SHAFT_ANGLE_DEG: f64 = 80.0;
pub const Z_UP: Vec3 = Vec3::new(0.0, 0.0, 1.0);

/// Clamps the end position of a shaft such that the angle between the shaft and the reference axis
/// does not exceed `max_angle_deg`.
///
/// - `start` The fixed start point of the vector (anchor)
/// - `desired_end` The desired end point (moving)
/// - `max_angle_deg` Maximum allowed angle in degrees from the reference axis
/// - `reference_axis` The axis to measure against
pub fn clamp_shaft_angle(
    start: Vec3,
    desired_end: Vec3,
    max_angle_deg: f64,
    reference_axis: Vec3,
) -> ShaftClampResult {
    let vx = desired_end.x - start.x;
    let vy = desired_end.y - start.y;
    let vz = desired_end.z - start.z;
    let len_sq = (vx * vx) + (vy * vy) + (vz * vz);
    let len = len_sq.sqrt();

    if len < 0.001 {
        return ShaftClampResult {
            clamped_pos: desired_end,
            is_clamped: false,
            angle_deg: 0.0,
        };
    }

    let ax_raw = reference_axis.x;
    let ay_raw = reference_axis.y;
    let az_raw = reference_axis.z;
    let axis_len = ((ax_raw * ax_raw) + (ay_raw * ay_raw) + (az_raw * az_raw)).sqrt();

    if axis_len < 1e-8 {
        return ShaftClampResult {
            clamped_pos: desired_end,
            is_clamped: false,
            angle_deg: 0.0,
        };
    }

    let ax = ax_raw / axis_len;
    let ay = ay_raw / axis_len;
    let az = az_raw / axis_len;

    let dot = (vx * ax) + (vy * ay) + (vz * az);
    let cos_theta = js_max(-1.0, js_min(1.0, dot / len));
    let angle_rad = js_acos(cos_theta);
    let angle_deg = angle_rad * (180.0 / std::f64::consts::PI);

    if angle_deg <= max_angle_deg {
        return ShaftClampResult {
            clamped_pos: desired_end,
            is_clamped: false,
            angle_deg,
        };
    }

    // Clamp required: preserve vector length and azimuth around the axis.
    let mut perp_x = vx - (ax * dot);
    let mut perp_y = vy - (ay * dot);
    let mut perp_z = vz - (az * dot);
    let mut perp_len = ((perp_x * perp_x) + (perp_y * perp_y) + (perp_z * perp_z)).sqrt();

    // Degenerate case: choose a stable arbitrary axis-perpendicular direction.
    if perp_len < 1e-8 {
        if az.abs() < 0.9 {
            // cross(axis, z-up)
            perp_x = ay;
            perp_y = -ax;
            perp_z = 0.0;
        } else {
            // cross(axis, x-right)
            perp_x = 0.0;
            perp_y = az;
            perp_z = -ay;
        }

        perp_len = ((perp_x * perp_x) + (perp_y * perp_y) + (perp_z * perp_z)).sqrt();
        if perp_len < 1e-8 {
            return ShaftClampResult {
                clamped_pos: desired_end,
                is_clamped: false,
                angle_deg,
            };
        }
    }

    let inv_perp_len = 1.0 / perp_len;
    let perp_dir_x = perp_x * inv_perp_len;
    let perp_dir_y = perp_y * inv_perp_len;
    let perp_dir_z = perp_z * inv_perp_len;

    let limit_rad = (max_angle_deg * std::f64::consts::PI) / 180.0;
    let new_parallel_len = js_cos(limit_rad) * len;
    let new_perp_len = js_sin(limit_rad) * len;

    let v_new_x = (ax * new_parallel_len) + (perp_dir_x * new_perp_len);
    let v_new_y = (ay * new_parallel_len) + (perp_dir_y * new_perp_len);
    let v_new_z = (az * new_parallel_len) + (perp_dir_z * new_perp_len);

    ShaftClampResult {
        clamped_pos: Vec3::new(start.x + v_new_x, start.y + v_new_y, start.z + v_new_z),
        is_clamped: true,
        angle_deg: max_angle_deg,
    }
}
