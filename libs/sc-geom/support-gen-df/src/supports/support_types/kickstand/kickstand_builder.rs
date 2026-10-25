//! DragonFruit `src/supports/SupportTypes/Kickstand/kickstandBuilder.ts`.
//!
//! Settings are passed in (`getSettings()`), ids come from `new_id`
//! (`uuidv4()`), and the collision mesh is the model's distance field
//! (`isShaftBlocked`).

use super::kickstand_rules::{assert_kickstand_host_kind, clamp_kickstand_host_t};
use super::kickstand_settings::{
    KickstandLayoutOverrides, get_kickstand_knot_diameter_mm, get_kickstand_root_profile,
    resolve_kickstand_layout,
};
use super::types::{KickstandBuildResult, KickstandHostTarget, KickstandPlacementLayout};
use crate::supports::constants::get_joint_diameter;
use crate::supports::placement_logic::collision_avoidance::{CollisionField, is_shaft_blocked};
use crate::supports::rendering::support_builder::SupportData;
use crate::supports::settings::types::SupportSettings;
use crate::supports::types::{
    Joint, Kickstand, KickstandProfile, Knot, LimitationCode, Roots, Segment, SupportEntity,
    Transform, Vec3,
};
use crate::three::Quaternion;
use crate::three::math_utils::{clamp, js_acos, js_max, rad_to_deg};

#[derive(Clone, Copy)]
pub struct KickstandBuildInput<'a> {
    pub model_id: &'a str,
    pub root_pos: Vec3,
    pub host: &'a KickstandHostTarget,
    pub layout_overrides: Option<KickstandLayoutOverrides>,
    /// A column known end to end (its top); without one the derived
    /// four-segment shape applies.
    pub authored_column_top: Option<Vec3>,
    /// The model the column must not run through. Without one it is not checked.
    pub collision_mesh: Option<&'a dyn CollisionField>,
    /// How far a run may lean from vertical, from the shaft setting.
    pub max_shaft_angle_deg: Option<f64>,
}

fn create_joint(pos: Vec3, diameter: f64, new_id: &mut dyn FnMut() -> String) -> Joint {
    Joint {
        id: new_id(),
        pos,
        diameter,
    }
}

struct JointHeights {
    first_joint_z: f64,
    second_joint_z: f64,
    third_joint_z: f64,
}

fn create_root_and_joint_heights(input: &KickstandBuildInput<'_>, root_top_z: f64) -> JointHeights {
    let layout: KickstandPlacementLayout =
        resolve_kickstand_layout(input.layout_overrides.as_ref());

    let host_z = input.host.pos.z;
    let raw_rise = host_z - root_top_z;

    // Kickstands are grounded columns first. If the host is very low, keep a
    // tiny positive vertical chain and let the terminal segment angle as needed.
    let effective_rise = js_max(
        raw_rise,
        layout.min_joint_spacing_mm * 2.0 + layout.min_terminal_clearance_mm + 0.01,
    );

    let mut first_joint_z = root_top_z + effective_rise * layout.first_joint_height_ratio;
    let mut second_joint_z = root_top_z + effective_rise * layout.second_joint_height_ratio;

    let min_first = root_top_z + 0.01;
    let max_second = js_max(
        min_first + layout.min_joint_spacing_mm,
        host_z - layout.min_terminal_clearance_mm,
    );

    if second_joint_z > max_second {
        second_joint_z = max_second;
    }

    let max_first = second_joint_z - layout.min_joint_spacing_mm;
    if first_joint_z > max_first {
        first_joint_z = max_first;
    }
    if first_joint_z < min_first {
        first_joint_z = min_first;
    }

    if second_joint_z - first_joint_z < layout.min_joint_spacing_mm {
        second_joint_z = first_joint_z + layout.min_joint_spacing_mm;
    }

    let max_third = js_max(
        second_joint_z + layout.min_joint_spacing_mm,
        host_z - layout.min_terminal_clearance_mm,
    );
    let third_joint_z = clamp(
        second_joint_z + js_max(layout.min_joint_spacing_mm, (host_z - second_joint_z) * 0.6),
        second_joint_z + layout.min_joint_spacing_mm,
        max_third,
    );

    JointHeights {
        first_joint_z,
        second_joint_z,
        third_joint_z,
    }
}

/// At least this far from horizontal, matching the shaft constraint's 80deg default.
const DEFAULT_MAX_SHAFT_ANGLE_DEG: f64 = 80.0;

/// A descending run means the chain overshot its host and doubled back.
fn column_shape_error(
    segments: &[Segment],
    host_pos: Vec3,
    max_shaft_angle_deg: Option<f64>,
) -> Option<LimitationCode> {
    let limit_deg = max_shaft_angle_deg.unwrap_or(DEFAULT_MAX_SHAFT_ANGLE_DEG);
    let points = column_points(segments, Some(host_pos));

    for pair in points.windows(2) {
        let run = Vec3::sub_vectors(pair[1], pair[0]);
        let span = run.length();
        if span < 1e-6 {
            continue;
        }
        let angle_from_up_deg = rad_to_deg(js_acos(clamp(run.z / span, -1.0, 1.0)));
        if angle_from_up_deg > limit_deg {
            return Some(LimitationCode::AngleTooSteep);
        }
    }
    None
}

/// The column's joints bottom to top, ending at the host it reaches.
fn column_points(segments: &[Segment], host_pos: Option<Vec3>) -> Vec<Vec3> {
    let mut points = Vec::new();
    for segment in segments {
        if let Some(bottom) = &segment.bottom_joint
            && points.is_empty()
        {
            points.push(bottom.pos);
        }
        if let Some(top) = &segment.top_joint {
            points.push(top.pos);
        }
    }
    if let Some(host) = host_pos {
        points.push(host);
    }
    points
}

/// Does the column run through the model? The top run insets off its host.
fn column_collision(
    segments: &[Segment],
    body_diameter_mm: f64,
    mesh: Option<&dyn CollisionField>,
) -> Option<LimitationCode> {
    let mesh = mesh?;

    let radius = js_max(0.001, body_diameter_mm / 2.0);
    let points = column_points(segments, None);

    for i in 0..points.len().saturating_sub(1) {
        let start = points[i];
        let end = points[i + 1];
        let probe_end = if i == points.len() - 2 {
            inset_towards(end, start, radius)
        } else {
            end
        };
        if is_shaft_blocked(start, probe_end, radius, mesh) {
            return Some(LimitationCode::CollisionWithModel);
        }
    }
    None
}

/// `from` pulled back toward `towards`, or unchanged when the run is shorter.
fn inset_towards(from: Vec3, towards: Vec3, distance_mm: f64) -> Vec3 {
    let run = Vec3::sub_vectors(towards, from);
    let length = run.length();
    if length <= distance_mm {
        return from;
    }
    from.add_scaled_vector(run.divide_scalar(length), distance_mm)
}

pub fn build_kickstand_data(
    input: &KickstandBuildInput<'_>,
    settings: &SupportSettings,
    new_id: &mut dyn FnMut() -> String,
) -> Result<KickstandBuildResult, String> {
    let host = input.host;
    assert_kickstand_host_kind(host.support_kind.as_str())?;

    let (root_diameter, root_disk_height, root_cone_height) = get_kickstand_root_profile(settings);
    let body_diameter_mm = js_max(0.001, host.diameter_mm);
    let joint_diameter_mm = get_joint_diameter(body_diameter_mm);

    let root_id = new_id();
    let host_knot_id = new_id();
    let kickstand_id = new_id();

    let root = Roots {
        base: SupportEntity {
            id: root_id.clone(),
            model_id: input.model_id.to_owned(),
            ..SupportEntity::default()
        },
        transform: Transform {
            pos: input.root_pos,
            rot: Quaternion::IDENTITY,
            scale: None,
        },
        diameter: root_diameter,
        disk_height: root_disk_height,
        cone_height: root_cone_height,
        height: None,
    };

    let root_top_z = input.root_pos.z + root.disk_height + root.cone_height;
    let heights = create_root_and_joint_heights(input, root_top_z);

    let joint1 = create_joint(
        Vec3::new(input.root_pos.x, input.root_pos.y, heights.first_joint_z),
        joint_diameter_mm,
        new_id,
    );
    let joint2 = create_joint(
        Vec3::new(input.root_pos.x, input.root_pos.y, heights.second_joint_z),
        joint_diameter_mm,
        new_id,
    );
    let top_blend = 0.65;
    let joint3 = create_joint(
        Vec3::new(
            input.root_pos.x + (host.pos.x - input.root_pos.x) * top_blend,
            input.root_pos.y + (host.pos.y - input.root_pos.y) * top_blend,
            heights.third_joint_z,
        ),
        joint_diameter_mm,
        new_id,
    );

    let segment1 = Segment {
        top_joint: Some(joint1.clone()),
        ..Segment::straight(new_id(), body_diameter_mm)
    };
    let segment2 = Segment {
        bottom_joint: Some(joint1),
        top_joint: Some(joint2.clone()),
        ..Segment::straight(new_id(), body_diameter_mm)
    };
    let terminal_segment = Segment {
        bottom_joint: Some(joint3.clone()),
        ..Segment::straight(new_id(), body_diameter_mm)
    };
    let upper_segment = Segment {
        bottom_joint: Some(joint2),
        top_joint: Some(joint3),
        ..Segment::straight(new_id(), body_diameter_mm)
    };

    let host_min_t = host.min_t.unwrap_or(0.0);
    let host_t = clamp_kickstand_host_t(host.t, host_min_t);

    let host_knot = Knot {
        id: host_knot_id.clone(),
        parent_shaft_id: host.segment_id.clone(),
        t: Some(host_t),
        pos: host.pos,
        diameter: Some(get_kickstand_knot_diameter_mm(host.diameter_mm)),
        ..Knot::default()
    };

    // An authored column is already straight, so it keeps that shape: one
    // column segment and one terminal. The derived joints exist to give a
    // placed kickstand something to drag, and would bend it.
    let segments = match input.authored_column_top {
        Some(top_pos) => {
            let column_top = create_joint(top_pos, joint_diameter_mm, new_id);
            vec![
                Segment {
                    top_joint: Some(column_top.clone()),
                    ..Segment::straight(new_id(), body_diameter_mm)
                },
                Segment {
                    bottom_joint: Some(column_top),
                    ..Segment::straight(new_id(), body_diameter_mm)
                },
            ]
        }
        None => vec![segment1, segment2, upper_segment, terminal_segment],
    };

    let error = column_shape_error(&segments, host_knot.pos, input.max_shaft_angle_deg)
        .or_else(|| column_collision(&segments, body_diameter_mm, input.collision_mesh));

    let kickstand = Kickstand {
        base: SupportEntity {
            id: kickstand_id,
            model_id: input.model_id.to_owned(),
            ..SupportEntity::default()
        },
        root_id,
        host_knot_id,
        host_segment_id: host.segment_id.clone(),
        host_min_t,
        auto_bracing_generated: None,
        segments,
        profile: KickstandProfile {
            body_diameter_mm,
            terminal_start_diameter_mm: body_diameter_mm,
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

/// The preview: the terminal segment closes on the host knot.
pub fn to_kickstand_preview_data(build: &KickstandBuildResult) -> SupportData {
    let last_index = build.kickstand.segments.len().wrapping_sub(1);
    let preview_segments = build
        .kickstand
        .segments
        .iter()
        .enumerate()
        .map(|(index, segment)| {
            if index != last_index {
                return segment.clone();
            }
            Segment {
                top_joint: Some(Joint {
                    id: format!("preview-terminal-{}", build.kickstand.base.id),
                    pos: build.host_knot.pos,
                    diameter: build
                        .host_knot
                        .diameter
                        .unwrap_or(build.kickstand.profile.terminal_end_diameter_mm),
                }),
                ..segment.clone()
            }
        })
        .collect();

    SupportData {
        id: build.kickstand.base.id.clone(),
        roots: Some(build.root.clone()),
        segments: preview_segments,
        knot: Some(build.host_knot.clone()),
        error: build.error,
        ..SupportData::default()
    }
}
