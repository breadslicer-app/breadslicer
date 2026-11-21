//! DragonFruit `src/supports/PlacementLogic/ConeAxisPolicy.ts`.

use serde::{Deserialize, Serialize};

use crate::supports::types::Vec3;
use crate::three::math_utils::{
    clamp, deg_to_rad, js_atan2, js_cos, js_hypot, js_max, js_min, js_sin,
};

pub const MAX_CONE_AXIS_DEVIATION_FROM_SURFACE_NORMAL_DEG: f64 = 30.0;

/// How a contact cone's axis is chosen from the surface normal (`tip.coneAngleMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConeAngleMode {
    #[default]
    Normal,
    Locked,
    Adaptive,
}

pub fn normalize_vector_or_fallback(vector: Vec3, fallback: Vec3) -> Vec3 {
    if vector.length_sq() < 0.000001 {
        return fallback.normalize();
    }
    vector.normalize()
}

fn normalize_or_fallback(vector: Vec3, fallback: Vec3) -> Vec3 {
    if vector.length_sq() < 1e-10 {
        return fallback.normalize();
    }
    vector.normalize()
}

/// DragonFruit's default `max_deviation_deg` is
/// [`MAX_CONE_AXIS_DEVIATION_FROM_SURFACE_NORMAL_DEG`].
pub fn clamp_cone_axis_deviation_from_surface_normal(
    surface_normal: Vec3,
    desired_cone_axis: Vec3,
    max_deviation_deg: f64,
) -> Vec3 {
    let normal = normalize_or_fallback(surface_normal, Vec3::new(0.0, 0.0, 1.0));
    let mut cone_axis_vec = normalize_or_fallback(desired_cone_axis, surface_normal);

    let max_deviation_rad = deg_to_rad(max_deviation_deg);
    let deviation_rad = cone_axis_vec.angle_to(normal);
    if deviation_rad <= max_deviation_rad + 1e-6 {
        return cone_axis_vec;
    }

    let mut rotation_axis = Vec3::cross_vectors(normal, cone_axis_vec);
    if rotation_axis.length_sq() < 1e-10 {
        rotation_axis = Vec3::new(0.0, 0.0, 1.0).cross(normal);
        if rotation_axis.length_sq() < 1e-10 {
            rotation_axis = Vec3::new(1.0, 0.0, 0.0).cross(normal);
        }
    }
    rotation_axis = rotation_axis.normalize();
    cone_axis_vec = normal
        .apply_axis_angle(rotation_axis, max_deviation_rad)
        .normalize();
    cone_axis_vec
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConeAxisPolicyInput {
    pub surface_normal: Vec3,
    pub cone_angle_mode: ConeAngleMode,
    pub adaptive_cone_angle_offset_deg: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConeAxisPolicyResult {
    pub surface_angle_from_up_deg: f64,
    pub min_allowed_surface_angle_from_up_deg: f64,
    pub cone_axis: Vec3,
}

pub fn resolve_cone_axis_policy(input: ConeAxisPolicyInput) -> ConeAxisPolicyResult {
    let ConeAxisPolicyInput {
        surface_normal,
        cone_angle_mode,
        adaptive_cone_angle_offset_deg,
    } = input;

    let normal = normalize_or_fallback(surface_normal, Vec3::new(0.0, 0.0, 1.0));
    let up = Vec3::new(0.0, 0.0, 1.0);

    let surface_angle_from_up_deg = normal.angle_to(up) * (180.0 / std::f64::consts::PI);

    let min_allowed_surface_angle_from_up_deg = if cone_angle_mode == ConeAngleMode::Normal {
        90.0
    } else {
        85.0
    };

    let mut cone_axis_vec = normal;

    match cone_angle_mode {
        ConeAngleMode::Locked => {
            let mut forced_angle_from_up_deg: Option<f64> = None;

            if (110.0..=140.0).contains(&surface_angle_from_up_deg) {
                forced_angle_from_up_deg = Some(140.0);
            } else if (85.0..110.0).contains(&surface_angle_from_up_deg) {
                forced_angle_from_up_deg = Some(110.0);
            }

            if let Some(forced) = forced_angle_from_up_deg {
                let horizontal_dir = Vec3::new(normal.x, normal.y, 0.0);
                if horizontal_dir.length_sq() > 1e-10 {
                    let horizontal_dir = horizontal_dir.normalize();

                    let forced_rad = deg_to_rad(forced);
                    let horizontal_mag = js_sin(forced_rad);
                    let vertical = js_cos(forced_rad);

                    cone_axis_vec = horizontal_dir
                        .multiply_scalar(horizontal_mag)
                        .add(up.multiply_scalar(vertical))
                        .normalize();
                }
            }
        }
        ConeAngleMode::Adaptive => {
            let horizontal_dir = Vec3::new(normal.x, normal.y, 0.0);
            if horizontal_dir.length_sq() > 1e-10 {
                let horizontal_dir = horizontal_dir.normalize();

                let ramp_start_deg = 90.0;
                let ramp_end_deg = 150.0;
                let raw_offset = adaptive_cone_angle_offset_deg.unwrap_or(30.0);
                let max_offset_deg = clamp(raw_offset, 0.0, 90.0);

                let t = clamp(
                    (surface_angle_from_up_deg - ramp_start_deg) / (ramp_end_deg - ramp_start_deg),
                    0.0,
                    1.0,
                );
                let offset_deg = max_offset_deg * (1.0 - t);
                let forced_angle_from_up_deg =
                    js_min(180.0, surface_angle_from_up_deg + offset_deg);

                let forced_rad = deg_to_rad(forced_angle_from_up_deg);
                let horizontal_mag = js_sin(forced_rad);
                let vertical = js_cos(forced_rad);

                cone_axis_vec = horizontal_dir
                    .multiply_scalar(horizontal_mag)
                    .add(up.multiply_scalar(vertical))
                    .normalize();
            }
        }
        ConeAngleMode::Normal => {
            if (90.0..=95.0).contains(&surface_angle_from_up_deg) {
                let horizontal = Vec3::new(normal.x, normal.y, 0.0);
                if horizontal.length_sq() > 1e-10 {
                    let horizontal = horizontal.normalize();
                    let tilt_rad = deg_to_rad(10.0);
                    let down = Vec3::new(0.0, 0.0, -1.0);
                    cone_axis_vec = horizontal
                        .multiply_scalar(js_cos(tilt_rad))
                        .add(down.multiply_scalar(js_sin(tilt_rad)))
                        .normalize();
                }
            }
        }
    }

    let cone_axis = clamp_cone_axis_deviation_from_surface_normal(
        normal,
        cone_axis_vec,
        MAX_CONE_AXIS_DEVIATION_FROM_SURFACE_NORMAL_DEG,
    );

    // Never let the cone axis point upward — in resin printing the cone must
    // point toward the build plate (negative Z).  If the computed axis has a
    // positive Z component, project it to horizontal with a slight downward tilt.
    let clamped_axis = clamp_axis_downward(cone_axis);

    ConeAxisPolicyResult {
        surface_angle_from_up_deg,
        min_allowed_surface_angle_from_up_deg,
        cone_axis: clamped_axis,
    }
}

/// How far the cone a contact will actually render leans from vertical, in
/// degrees: 0 is a cone pointing straight at the plate, 90 one lying flat
/// sideways.
///
/// Measured on the resolved axis, not on the surface normal, because the policy
/// can rotate the axis away from the normal — `adaptive` ramps it further out
/// past 90° and `locked` forces 110/140 — so a guard reading the normal accepts
/// contacts whose cone renders sideways, and refuses contacts whose cone does not.
pub fn cone_axis_lean_from_vertical_deg(
    surface_normal: Vec3,
    cone_angle_mode: ConeAngleMode,
    adaptive_cone_angle_offset_deg: Option<f64>,
) -> f64 {
    let ConeAxisPolicyResult { cone_axis, .. } = resolve_cone_axis_policy(ConeAxisPolicyInput {
        surface_normal,
        cone_angle_mode,
        adaptive_cone_angle_offset_deg,
    });
    let horizontal = (cone_axis.x * cone_axis.x + cone_axis.y * cone_axis.y).sqrt();
    (js_atan2(horizontal, js_max(0.001, cone_axis.z.abs())) * 180.0) / std::f64::consts::PI
}

/// Steepest lean from vertical a contact's rendered cone may take. Past this the
/// cone lies within 15° of flat: it pushes the model sideways rather than holding
/// it up, and it reads as a near-horizontal whisker off the model.
pub const MAX_SIDE_WALL_CONTACT_LEAN_DEG: f64 = 75.0;

/// Whether a contact's cone renders too close to flat to place on. The contact
/// disk stays on the surface, so a near-vertical face forces a near-horizontal
/// cone whichever way the shaft leaves — the member cannot hold the part up, and
/// the shaft reads as a whisker off the model.
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

/// Whether a cone keeps its contact the highest point of the support.
///
/// A support hangs from the point it touches the model at: nothing it is made of
/// sits above that point. The contact cone is the piece that can break it — the
/// cone ends at its socket, a body of radius `bodyDiameterMm / 2` standing on the
/// axis — so an axis swung far enough off vertical lifts the socket's own rim
/// over the tip, and the member reads as a cone pointing down with the support
/// growing out of the top of it.
pub fn cone_keeps_contact_highest(
    cone_axis: Vec3,
    socket_pos: Vec3,
    tip_pos: Vec3,
    body_diameter_mm: f64,
) -> bool {
    let axis = normalize_or_fallback(cone_axis, Vec3::new(0.0, 0.0, -1.0));
    let socket_rim_rise_mm = (body_diameter_mm / 2.0) * js_hypot(&[axis.x, axis.y]);
    socket_pos.z + socket_rim_rise_mm <= tip_pos.z + 1e-6
}

/// Ensures a cone axis never points upward (positive Z).
/// Projects upward-pointing axes to horizontal with a 5° downward tilt.
fn clamp_axis_downward(axis: Vec3) -> Vec3 {
    if axis.z <= 0.0 {
        return axis;
    }

    let h_len = (axis.x * axis.x + axis.y * axis.y).sqrt();
    if h_len < 0.001 {
        return Vec3::new(0.0, 0.0, -1.0);
    }
    let tilt_rad = std::f64::consts::PI / 36.0; // 5°
    let scale = 1.0 / h_len;
    Vec3::new(
        axis.x * scale * js_cos(tilt_rad),
        axis.y * scale * js_cos(tilt_rad),
        -js_sin(tilt_rad),
    )
}
