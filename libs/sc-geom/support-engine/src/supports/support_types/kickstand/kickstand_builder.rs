//! A kickstand: a column from its own plate root up to a knot on the side
//! of another support. It rises plumb through two joints, then leans in
//! through a third to meet its host.

use super::kickstand_rules::{assert_kickstand_host_kind, clamp_kickstand_host_t};
use super::kickstand_settings::{
    KickstandLayoutOverrides, get_kickstand_knot_diameter_mm, get_kickstand_root_profile,
    resolve_kickstand_layout,
};
use super::types::{KickstandBuildResult, KickstandHostTarget};
use crate::geom::Quaternion;
use crate::geom::math_utils::{clamp, js_acos, js_max, rad_to_deg};
use crate::supports::constants::get_joint_diameter;
use crate::supports::placement_logic::collision_avoidance::{CollisionField, is_shaft_blocked};
use crate::supports::settings::types::SupportSettings;
use crate::supports::types::{
    Joint, Kickstand, KickstandProfile, Knot, LimitationCode, Roots, Segment, SupportEntity,
    Transform, Vec3,
};

/// How far the third joint goes from the column toward the host, as a share
/// of the way across.
const LEAN_IN_SHARE: f64 = 0.65;
/// The steepest a run may lean off plumb when the shaft setting gives no
/// limit, in degrees.
const DEFAULT_MAX_SHAFT_ANGLE_DEG: f64 = 80.0;

#[derive(Clone, Copy, Debug)]
pub struct KickstandBuildInput<'a> {
    pub model_id: &'a str,
    pub root_pos: Vec3,
    pub host: &'a KickstandHostTarget,
    pub layout_overrides: Option<KickstandLayoutOverrides>,
    /// The top of a column already known end to end. Without one the column
    /// takes the three-joint shape.
    pub authored_column_top: Option<Vec3>,
    /// The model the column must stay out of; unchecked without one.
    pub collision_mesh: Option<&'a dyn CollisionField>,
    /// The steepest a run may lean off plumb, from the shaft setting.
    pub max_shaft_angle_deg: Option<f64>,
}

/// Heights of the three column joints over a root whose top is at
/// `root_top_z`.
fn joint_heights(input: &KickstandBuildInput<'_>, root_top_z: f64) -> [f64; 3] {
    let layout = resolve_kickstand_layout(input.layout_overrides.as_ref());
    let spacing = layout.min_joint_spacing_mm;
    let clearance = layout.min_terminal_clearance_mm;
    let host_z = input.host.pos.z;

    // A kickstand stands on its column first: under a very low host the
    // column still rises a little, and the last run leans as it must.
    let rise = js_max(host_z - root_top_z, spacing * 2.0 + clearance + 0.01);
    let mut first = root_top_z + rise * layout.first_joint_height_ratio;
    let mut second = root_top_z + rise * layout.second_joint_height_ratio;

    let lowest_first = root_top_z + 0.01;
    let highest_second = js_max(lowest_first + spacing, host_z - clearance);
    if second > highest_second {
        second = highest_second;
    }
    let highest_first = second - spacing;
    if first > highest_first {
        first = highest_first;
    }
    if first < lowest_first {
        first = lowest_first;
    }
    if second - first < spacing {
        second = first + spacing;
    }

    let highest_third = js_max(second + spacing, host_z - clearance);
    let third = clamp(
        second + js_max(spacing, (host_z - second) * 0.6),
        second + spacing,
        highest_third,
    );
    [first, second, third]
}

/// The column's joints bottom to top: the bottom of the first segment that
/// has one, then every segment's top, then `host` when given.
fn column_points(segments: &[Segment], host: Option<Vec3>) -> Vec<Vec3> {
    let mut points = Vec::new();
    for segment in segments {
        if points.is_empty()
            && let Some(bottom) = &segment.bottom_joint
        {
            points.push(bottom.pos);
        }
        if let Some(top) = &segment.top_joint {
            points.push(top.pos);
        }
    }
    points.extend(host);
    points
}

/// `AngleTooSteep` when a run of the column, the last one up to the host
/// included, leans past the limit: the chain overshot its host and doubled
/// back.
fn lean_error(
    segments: &[Segment],
    host_pos: Vec3,
    max_shaft_angle_deg: Option<f64>,
) -> Option<LimitationCode> {
    let limit_deg = max_shaft_angle_deg.unwrap_or(DEFAULT_MAX_SHAFT_ANGLE_DEG);
    column_points(segments, Some(host_pos))
        .windows(2)
        .any(|run| {
            let rise = Vec3::sub_vectors(run[1], run[0]);
            let length = rise.length();
            length >= 1e-6 && rad_to_deg(js_acos(clamp(rise.z / length, -1.0, 1.0))) > limit_deg
        })
        .then_some(LimitationCode::AngleTooSteep)
}

/// `CollisionWithModel` when the column runs into the model. The top run
/// stops a radius short of its end, where it meets the host.
fn collision_error(
    segments: &[Segment],
    body_diameter_mm: f64,
    mesh: Option<&dyn CollisionField>,
) -> Option<LimitationCode> {
    let mesh = mesh?;
    let radius = js_max(0.001, body_diameter_mm / 2.0);
    let points = column_points(segments, None);
    let runs = points.len().saturating_sub(1);
    (0..runs)
        .any(|i| {
            let (start, end) = (points[i], points[i + 1]);
            let end = if i + 1 == runs {
                pulled_back(end, start, radius)
            } else {
                end
            };
            is_shaft_blocked(start, end, radius, mesh)
        })
        .then_some(LimitationCode::CollisionWithModel)
}

/// `point` moved `distance_mm` toward `toward`; unmoved when they are closer
/// than that.
fn pulled_back(point: Vec3, toward: Vec3, distance_mm: f64) -> Vec3 {
    let run = Vec3::sub_vectors(toward, point);
    let length = run.length();
    if length <= distance_mm {
        return point;
    }
    point.add_scaled_vector(run.divide_scalar(length), distance_mm)
}

pub fn build_kickstand_data(
    input: &KickstandBuildInput<'_>,
    settings: &SupportSettings,
    new_id: &mut dyn FnMut() -> String,
) -> Result<KickstandBuildResult, String> {
    let host = input.host;
    assert_kickstand_host_kind(host.support_kind.as_str())?;

    let (root_diameter, disk_height, cone_height) = get_kickstand_root_profile(settings);
    let body_diameter = js_max(0.001, host.diameter_mm);
    let joint_diameter = get_joint_diameter(body_diameter);
    let entity = |id: String| SupportEntity {
        id,
        model_id: input.model_id.to_owned(),
        ..SupportEntity::default()
    };

    let root_id = new_id();
    let host_knot_id = new_id();
    let kickstand_id = new_id();
    let root_pos = input.root_pos;
    let root = Roots {
        base: entity(root_id.clone()),
        transform: Transform {
            pos: root_pos,
            rot: Quaternion::IDENTITY,
            scale: None,
        },
        diameter: root_diameter,
        disk_height,
        cone_height,
        height: None,
        join_cone_mm: None,
    };

    // The three-joint column is always drawn, even when an authored column
    // replaces it, so the ids handed out do not depend on which is kept.
    let [first_z, second_z, third_z] = joint_heights(input, root_pos.z + disk_height + cone_height);
    let joint = |id: String, pos: Vec3| Joint {
        id,
        pos,
        diameter: joint_diameter,
    };
    let run = |id: String, bottom: Option<Joint>, top: Option<Joint>| Segment {
        bottom_joint: bottom,
        top_joint: top,
        ..Segment::straight(id, body_diameter)
    };
    let first = joint(new_id(), Vec3::new(root_pos.x, root_pos.y, first_z));
    let second = joint(new_id(), Vec3::new(root_pos.x, root_pos.y, second_z));
    let third = joint(
        new_id(),
        Vec3::new(
            root_pos.x + (host.pos.x - root_pos.x) * LEAN_IN_SHARE,
            root_pos.y + (host.pos.y - root_pos.y) * LEAN_IN_SHARE,
            third_z,
        ),
    );
    let foot = run(new_id(), None, Some(first.clone()));
    let middle = run(new_id(), Some(first), Some(second.clone()));
    let to_host = run(new_id(), Some(third.clone()), None);
    let lean_in = run(new_id(), Some(second), Some(third));

    // An authored column is straight already, so it keeps that shape: one
    // column segment and the run to the host. The derived joints only give
    // a placed kickstand something to drag, and would bend it.
    let segments = match input.authored_column_top {
        Some(top) => {
            let top = joint(new_id(), top);
            let column = run(new_id(), None, Some(top.clone()));
            vec![column, run(new_id(), Some(top), None)]
        }
        None => vec![foot, middle, lean_in, to_host],
    };

    let host_min_t = host.min_t.unwrap_or(0.0);
    let host_knot = Knot {
        id: host_knot_id.clone(),
        parent_shaft_id: host.segment_id.clone(),
        t: Some(clamp_kickstand_host_t(host.t, host_min_t)),
        pos: host.pos,
        diameter: Some(get_kickstand_knot_diameter_mm(host.diameter_mm)),
    };
    let error = lean_error(&segments, host_knot.pos, input.max_shaft_angle_deg)
        .or_else(|| collision_error(&segments, body_diameter, input.collision_mesh));

    let kickstand = Kickstand {
        base: entity(kickstand_id),
        root_id,
        host_knot_id,
        host_segment_id: host.segment_id.clone(),
        host_min_t,
        segments,
        profile: KickstandProfile {
            body_diameter_mm: body_diameter,
            terminal_start_diameter_mm: body_diameter,
            terminal_end_diameter_mm: js_max(0.001, host.diameter_mm),
        },
    };

    Ok(KickstandBuildResult {
        root,
        host_knot,
        kickstand,
        error,
    })
}
