//! Which way a tip cone points, from the normal of the surface it touches,
//! and whether that cone can hold the model at all.

use serde::{Deserialize, Serialize};

use crate::geom::math_utils::{
    clamp, deg_to_rad, js_atan2, js_cos, js_hypot, js_max, js_min, js_sin,
};
use crate::supports::types::Vec3;

/// The most a cone's axis may swing away from its surface normal, degrees.
pub const MAX_CONE_AXIS_DEVIATION_FROM_SURFACE_NORMAL_DEG: f64 = 30.0;

/// How a tip cone's axis follows its surface normal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConeAngleMode {
    /// Along the normal; a surface just past vertical tips the cone 10°
    /// downward.
    #[default]
    Normal,
    /// Swung further out the more upright the surface, by up to an offset.
    Adaptive,
}

const UP: Vec3 = Vec3::new(0.0, 0.0, 1.0);

/// `vector` made unit, or `fallback` made unit when `vector` is almost
/// nothing.
pub fn normalize_vector_or_fallback(vector: Vec3, fallback: Vec3) -> Vec3 {
    if vector.length_sq() < 0.000001 {
        fallback.normalize()
    } else {
        vector.normalize()
    }
}

/// As [`normalize_vector_or_fallback`], with a finer cut.
fn unit_or(vector: Vec3, fallback: Vec3) -> Vec3 {
    if vector.length_sq() < 1e-10 {
        fallback.normalize()
    } else {
        vector.normalize()
    }
}

/// `desired_cone_axis`, swung back toward `surface_normal` until it is no
/// further than `max_deviation_deg` from it.
pub fn clamp_cone_axis_deviation_from_surface_normal(
    surface_normal: Vec3,
    desired_cone_axis: Vec3,
    max_deviation_deg: f64,
) -> Vec3 {
    let normal = unit_or(surface_normal, UP);
    let axis = unit_or(desired_cone_axis, surface_normal);
    let limit = deg_to_rad(max_deviation_deg);
    if axis.angle_to(normal) <= limit + 1e-6 {
        return axis;
    }
    let mut pivot = Vec3::cross_vectors(normal, axis);
    if pivot.length_sq() < 1e-10 {
        pivot = UP.cross(normal);
        if pivot.length_sq() < 1e-10 {
            pivot = Vec3::new(1.0, 0.0, 0.0).cross(normal);
        }
    }
    normal
        .apply_axis_angle(pivot.normalize(), limit)
        .normalize()
}

/// What decides a cone's axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConeAxisPolicyInput {
    pub surface_normal: Vec3,
    pub cone_angle_mode: ConeAngleMode,
    /// How far out [`ConeAngleMode::Adaptive`] swings an upright surface's
    /// cone, degrees (30 when unset).
    pub adaptive_cone_angle_offset_deg: Option<f64>,
}

/// A cone's axis, and the angles it came from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConeAxisPolicyResult {
    /// The surface normal's angle from straight up, degrees.
    pub surface_angle_from_up_deg: f64,
    /// The least such angle a surface may have to be touched from below.
    pub min_allowed_surface_angle_from_up_deg: f64,
    pub cone_axis: Vec3,
}

/// The unit direction at `angle_from_up_deg` from straight up, leaning
/// toward `side` (horizontal, unit).
fn leaning(side: Vec3, angle_from_up_deg: f64) -> Vec3 {
    let a = deg_to_rad(angle_from_up_deg);
    side.multiply_scalar(js_sin(a))
        .add(UP.multiply_scalar(js_cos(a)))
        .normalize()
}

/// The axis a cone touching a surface of `surface_normal` takes: by its
/// mode, then no further than [`MAX_CONE_AXIS_DEVIATION_FROM_SURFACE_NORMAL_DEG`]
/// from the normal, and never pointing up.
pub fn resolve_cone_axis_policy(input: ConeAxisPolicyInput) -> ConeAxisPolicyResult {
    let normal = unit_or(input.surface_normal, UP);
    let from_up = normal.angle_to(UP) * (180.0 / std::f64::consts::PI);
    let side = Vec3::new(normal.x, normal.y, 0.0);
    let has_side = side.length_sq() > 1e-10;
    let wanted = match input.cone_angle_mode {
        ConeAngleMode::Adaptive if has_side => {
            // Upright surfaces (90° from up) swing out by the whole offset,
            // ceilings (150° and over) not at all.
            let offset = clamp(
                input.adaptive_cone_angle_offset_deg.unwrap_or(30.0),
                0.0,
                90.0,
            );
            let t = clamp((from_up - 90.0) / (150.0 - 90.0), 0.0, 1.0);
            leaning(
                side.normalize(),
                js_min(180.0, from_up + offset * (1.0 - t)),
            )
        }
        ConeAngleMode::Normal if has_side && (90.0..=95.0).contains(&from_up) => {
            let tilt = deg_to_rad(10.0);
            side.normalize()
                .multiply_scalar(js_cos(tilt))
                .add(Vec3::new(0.0, 0.0, -1.0).multiply_scalar(js_sin(tilt)))
                .normalize()
        }
        _ => normal,
    };
    let axis = clamp_cone_axis_deviation_from_surface_normal(
        normal,
        wanted,
        MAX_CONE_AXIS_DEVIATION_FROM_SURFACE_NORMAL_DEG,
    );
    ConeAxisPolicyResult {
        surface_angle_from_up_deg: from_up,
        min_allowed_surface_angle_from_up_deg: match input.cone_angle_mode {
            ConeAngleMode::Normal => 90.0,
            ConeAngleMode::Adaptive => 85.0,
        },
        cone_axis: pointing_down(axis),
    }
}

/// How far the cone a contact gets leans from vertical, degrees: 0 points
/// straight at the plate, 90 lies flat. Measured on the axis the cone gets,
/// which its mode may swing off the normal.
fn cone_axis_lean_from_vertical_deg(
    surface_normal: Vec3,
    cone_angle_mode: ConeAngleMode,
    adaptive_cone_angle_offset_deg: Option<f64>,
) -> f64 {
    let axis = resolve_cone_axis_policy(ConeAxisPolicyInput {
        surface_normal,
        cone_angle_mode,
        adaptive_cone_angle_offset_deg,
    })
    .cone_axis;
    let across = (axis.x * axis.x + axis.y * axis.y).sqrt();
    (js_atan2(across, js_max(0.001, axis.z.abs())) * 180.0) / std::f64::consts::PI
}

/// Past this lean from vertical a contact's cone lies within 15° of flat: it
/// pushes the model sideways instead of holding it up.
const MAX_SIDE_WALL_CONTACT_LEAN_DEG: f64 = 75.0;

/// Whether a contact on a surface of `surface_normal` gets a cone too close
/// to flat to hold anything (a wall).
pub fn is_side_wall_contact(
    surface_normal: Vec3,
    cone_angle_mode: ConeAngleMode,
    adaptive_cone_angle_offset_deg: Option<f64>,
) -> bool {
    cone_axis_lean_from_vertical_deg(
        surface_normal,
        cone_angle_mode,
        adaptive_cone_angle_offset_deg,
    ) > MAX_SIDE_WALL_CONTACT_LEAN_DEG
}

/// Whether the contact stays the highest point of its support: a cone swung
/// far off vertical lifts the rim of its socket above the tip.
pub fn cone_keeps_contact_highest(
    cone_axis: Vec3,
    socket_pos: Vec3,
    tip_pos: Vec3,
    body_diameter_mm: f64,
) -> bool {
    let axis = unit_or(cone_axis, Vec3::new(0.0, 0.0, -1.0));
    let rim_rise = (body_diameter_mm / 2.0) * js_hypot(&[axis.x, axis.y]);
    socket_pos.z + rim_rise <= tip_pos.z + 1e-6
}

/// `axis`, or, when it points up, laid out flat with a 5° droop (straight
/// down when it has no horizontal part): a cone always points toward the
/// plate.
fn pointing_down(axis: Vec3) -> Vec3 {
    if axis.z <= 0.0 {
        return axis;
    }
    let across = (axis.x * axis.x + axis.y * axis.y).sqrt();
    if across < 0.001 {
        return Vec3::new(0.0, 0.0, -1.0);
    }
    let droop = std::f64::consts::PI / 36.0;
    let scale = 1.0 / across;
    Vec3::new(
        axis.x * scale * js_cos(droop),
        axis.y * scale * js_cos(droop),
        -js_sin(droop),
    )
}
