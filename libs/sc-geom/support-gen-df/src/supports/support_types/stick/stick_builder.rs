//! DragonFruit `src/supports/SupportTypes/Stick/stickBuilder.ts`.
//!
//! Settings are passed in (`getSettings()`), ids come from `new_id`
//! (`uuidv4()`), and the mesh is the model's [`ModelMesh`]: rays for the cone
//! gap probe, the distance field for the shaft (`isShaftBlocked`).

use serde::Serialize;

use crate::supports::constants::get_joint_diameter;
use crate::supports::model_mesh::ModelMesh;
use crate::supports::placement_logic::collision_avoidance::is_shaft_blocked;
use crate::supports::placement_logic::collision_utils::check_shaft_collision;
use crate::supports::placement_logic::cone_axis_policy::{
    MAX_CONE_AXIS_DEVIATION_FROM_SURFACE_NORMAL_DEG, clamp_cone_axis_deviation_from_surface_normal,
};
use crate::supports::settings::types::SupportSettings;
use crate::supports::support_primitives::contact_cone::contact_cone_utils::get_socket_position;
use crate::supports::support_primitives::contact_cone::types::{
    ContactCone, SupportTipProfile, TipPrimitive,
};
use crate::supports::support_primitives::contact_disk::contact_disk_utils::calculate_disk_thickness;
use crate::supports::types::{Joint, LimitationCode, Segment, Stick, SupportEntity, Vec3};
use crate::three::math_utils::{js_hypot, js_max, js_min};

#[derive(Clone, Copy)]
pub struct StickBuildInput<'a> {
    pub model_id: &'a str,
    /// Auto-support tier overrides — absent for manual placement.
    pub shaft_diameter_mm: Option<f64>,
    pub tip_contact_diameter_mm: Option<f64>,
    pub a_pos: Vec3,
    pub a_normal: Vec3,
    pub b_pos: Vec3,
    pub b_normal: Vec3,
    pub mesh: Option<ModelMesh<'a>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StickBuildResult {
    pub stick: Stick,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<LimitationCode>,
}

const GEOMETRY_EPSILON: f64 = 0.000001;

/// Clearance kept between a shortened cone and the surface it stops short of.
const CONE_GAP_CLEARANCE_MM: f64 = 0.1;
/// A cone never shortens away to nothing; the disk stands it off the surface.
const MIN_CONE_LENGTH_MM: f64 = 0.1;

/// How far this end's cone may run before it leaves the free gap.
///
/// A stick's cone axis runs along the span, so on a short span the stock cone
/// length carries both sockets past each other and into the model, and the
/// shaft then starts inside material. Cast along the axis and keep the cone
/// inside the room the gap actually has; a span with room keeps the full cone.
fn clamp_cone_length_to_free_gap(
    start: Vec3,
    axis: Vec3,
    length_mm: f64,
    mesh: Option<ModelMesh<'_>>,
) -> f64 {
    let Some(mesh) = mesh else {
        return length_mm;
    };
    if length_mm <= MIN_CONE_LENGTH_MM {
        return length_mm;
    }
    let end = start.add_scaled_vector(axis, length_mm);
    let hit = check_shaft_collision(start, end, 0.0, mesh.rays);
    match (hit.hit, hit.distance) {
        (true, Some(distance)) => js_max(
            MIN_CONE_LENGTH_MM,
            js_min(length_mm, distance - CONE_GAP_CLEARANCE_MM),
        ),
        _ => length_mm,
    }
}

fn clamp_axis(surface_normal: Vec3, axis: Vec3) -> Vec3 {
    clamp_cone_axis_deviation_from_surface_normal(
        surface_normal,
        axis,
        MAX_CONE_AXIS_DEVIATION_FROM_SURFACE_NORMAL_DEG,
    )
}

pub fn build_stick(
    input: &StickBuildInput<'_>,
    settings: &SupportSettings,
    new_id: &mut dyn FnMut() -> String,
) -> StickBuildResult {
    let StickBuildInput {
        model_id,
        a_pos,
        a_normal,
        b_pos,
        b_normal,
        mesh,
        ..
    } = *input;

    // Auto-support tier overrides (absent for manual placement). The tier band
    // sizes the stick; the active Support Studio preset must not.
    let tip_contact_diameter_mm = input
        .tip_contact_diameter_mm
        .unwrap_or(settings.tip.contact_diameter_mm);
    let shaft_diameter = input
        .shaft_diameter_mm
        .unwrap_or(settings.shaft.diameter_mm);

    let tip_profile = SupportTipProfile {
        kind: Some(TipPrimitive::Disk),
        contact_diameter_mm: tip_contact_diameter_mm,
        body_diameter_mm: settings.tip.body_diameter_mm,
        length_mm: settings.tip.length_mm,
        penetration_mm: settings.tip.penetration_mm,
        disk_thickness_mm: Some(settings.tip.disk_thickness_mm.unwrap_or(0.1)),
        max_standoff_mm: Some(settings.tip.max_standoff_mm.unwrap_or(1.5)),
        standoff_angle_threshold: Some(
            settings
                .tip
                .standoff_angle_threshold
                .unwrap_or(std::f64::consts::PI / 4.0),
        ),
        sphere_radius_ratio: None,
    };

    let joint_diameter = get_joint_diameter(shaft_diameter);

    let unit_or_up = |v: Vec3| {
        if v.length_sq() < GEOMETRY_EPSILON {
            Vec3::new(0.0, 0.0, 1.0).normalize()
        } else {
            v.normalize()
        }
    };
    let surface_normal_a = unit_or_up(a_normal);
    let surface_normal_b = unit_or_up(b_normal);

    let mut cone_axis_a = Vec3::new(b_pos.x - a_pos.x, b_pos.y - a_pos.y, b_pos.z - a_pos.z);
    if cone_axis_a.length_sq() < GEOMETRY_EPSILON {
        cone_axis_a = surface_normal_a;
    }
    cone_axis_a = cone_axis_a.normalize();

    let mut cone_axis_b = cone_axis_a.multiply_scalar(-1.0);
    let mut cone_start_a = Vec3::ZERO;
    let mut cone_start_b = Vec3::ZERO;
    let mut disk_thickness_a = 0.0;
    let mut disk_thickness_b = 0.0;
    let disk_profile = tip_profile.as_disk_profile();

    // Match trunk behavior more closely: the disk stays glued to the local
    // surface normal, but the cone body is allowed to cant toward the bridge.
    for _pass in 0..2 {
        cone_axis_a = clamp_axis(surface_normal_a, cone_axis_a);
        cone_axis_b = clamp_axis(surface_normal_b, cone_axis_b);

        disk_thickness_a =
            calculate_disk_thickness(surface_normal_a, cone_axis_a, Some(&disk_profile));
        disk_thickness_b =
            calculate_disk_thickness(surface_normal_b, cone_axis_b, Some(&disk_profile));

        cone_start_a = a_pos.add_scaled_vector(surface_normal_a, disk_thickness_a);
        cone_start_b = b_pos.add_scaled_vector(surface_normal_b, disk_thickness_b);

        let bridge_axis = cone_start_b.sub(cone_start_a);
        if bridge_axis.length_sq() < GEOMETRY_EPSILON {
            break;
        }

        let bridge_axis = bridge_axis.normalize();
        cone_axis_a = bridge_axis;
        cone_axis_b = bridge_axis.multiply_scalar(-1.0);
    }

    let cone_axis_a = clamp_axis(surface_normal_a, cone_axis_a);
    let cone_axis_b = clamp_axis(surface_normal_b, cone_axis_b);

    let cone_length_a =
        clamp_cone_length_to_free_gap(cone_start_a, cone_axis_a, tip_profile.length_mm, mesh);
    let cone_length_b =
        clamp_cone_length_to_free_gap(cone_start_b, cone_axis_b, tip_profile.length_mm, mesh);
    let profile_a = SupportTipProfile {
        length_mm: cone_length_a,
        ..tip_profile
    };
    let profile_b = SupportTipProfile {
        length_mm: cone_length_b,
        ..tip_profile
    };

    let socket_a = get_socket_position(cone_start_a, cone_axis_a, Some(&profile_a));
    let socket_b = get_socket_position(cone_start_b, cone_axis_b, Some(&profile_b));

    let socket_joint_a = Joint {
        id: new_id(),
        pos: socket_a,
        diameter: joint_diameter,
    };
    let socket_joint_b = Joint {
        id: new_id(),
        pos: socket_b,
        diameter: joint_diameter,
    };

    let segment = Segment {
        bottom_joint: Some(socket_joint_a.clone()),
        top_joint: Some(socket_joint_b.clone()),
        ..Segment::straight(new_id(), shaft_diameter)
    };

    let contact_cone_a = ContactCone {
        id: new_id(),
        pos: a_pos,
        normal: cone_axis_a,
        surface_normal: Some(surface_normal_a),
        disk_length_override: Some(disk_thickness_a),
        placement_surface: None,
        profile: profile_a,
        socket_joint_id: Some(socket_joint_a.id.clone()),
    };
    let contact_cone_b = ContactCone {
        id: new_id(),
        pos: b_pos,
        normal: cone_axis_b,
        surface_normal: Some(surface_normal_b),
        disk_length_override: Some(disk_thickness_b),
        placement_surface: None,
        profile: profile_b,
        socket_joint_id: Some(socket_joint_b.id.clone()),
    };

    // Normalize ordering so the ID/joint ordering is deterministic across
    // equivalent inputs.
    let (a, b) = (a_pos, b_pos);
    let swap = a.z > b.z || (a.z == b.z && (a.y > b.y || (a.y == b.y && a.x > b.x)));
    let (contact_cone_a, contact_cone_b) = if swap {
        (contact_cone_b, contact_cone_a)
    } else {
        (contact_cone_a, contact_cone_b)
    };

    let stick = Stick {
        base: SupportEntity {
            id: new_id(),
            model_id: model_id.to_owned(),
            ..SupportEntity::default()
        },
        segments: vec![segment],
        contact_cone_a,
        contact_cone_b,
    };

    let mut error = None;
    if let Some(mesh) = mesh {
        let shaft_radius = shaft_diameter / 2.0;
        // A stick is anchored on the two surfaces it bridges, so the shaft's
        // own ends always sit within a shaft radius of material, by
        // construction. Test the free span instead, the part that can foul.
        // The tip cones are left out, as every other type leaves its own.
        let span = js_hypot(&[
            socket_b.x - socket_a.x,
            socket_b.y - socket_a.y,
            socket_b.z - socket_a.z,
        ]);
        if span > GEOMETRY_EPSILON {
            let inset = js_min(shaft_radius + 0.15, span * 0.4);
            let ux = (socket_b.x - socket_a.x) / span;
            let uy = (socket_b.y - socket_a.y) / span;
            let uz = (socket_b.z - socket_a.z) / span;
            let blocked = is_shaft_blocked(
                Vec3::new(
                    socket_a.x + ux * inset,
                    socket_a.y + uy * inset,
                    socket_a.z + uz * inset,
                ),
                Vec3::new(
                    socket_b.x - ux * inset,
                    socket_b.y - uy * inset,
                    socket_b.z - uz * inset,
                ),
                shaft_radius,
                mesh.field,
            );
            if blocked {
                error = Some(LimitationCode::CollisionWithModel);
            }
        }
    }

    StickBuildResult { stick, error }
}
