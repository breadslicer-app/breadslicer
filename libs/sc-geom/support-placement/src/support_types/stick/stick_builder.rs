//! A stick: a straight shaft between two model contacts, with a disk and a
//! cone at each end. The cones run along the shaft and the disks stay square
//! to their surfaces.

use serde::Serialize;

use crate::placement_logic::collision_utils::check_shaft_collision;
use support_geom::geom::math_utils::{js_hypot, js_max, js_min};
use support_model::collision_avoidance::is_shaft_blocked;
use support_model::cone_axis_policy::{
    MAX_CONE_AXIS_DEVIATION_FROM_SURFACE_NORMAL_DEG, clamp_cone_axis_deviation_from_surface_normal,
};
use support_model::constants::get_joint_diameter;
use support_model::model_mesh::ModelMesh;
use support_model::settings::types::SupportSettings;
use support_model::tips::{
    ContactCone, ContactDiskProfile, SupportTipProfile, TipPrimitive, get_socket_position,
    nib_length,
};
use support_model::types::{Joint, LimitationCode, Segment, Stick, SupportEntity, Vec3};

#[derive(Clone, Copy, Debug)]
pub struct StickBuildInput<'a> {
    pub model_id: &'a str,
    /// Sizes of the auto-support tier; manual placement leaves them unset and
    /// takes the settings'.
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

/// Lengths (and squared lengths) below this count as zero.
const EPSILON: f64 = 0.000001;
/// Air kept between a shortened cone's end and the surface it stops at.
const CONE_GAP_CLEARANCE_MM: f64 = 0.1;
/// The shortest a cone gets; the disk keeps it off the surface below that.
const MIN_CONE_LENGTH_MM: f64 = 0.1;

/// The cone length that still fits in the free gap ahead of `start`.
///
/// The cones point along the span, so across a short gap stock-length cones
/// would carry both sockets past each other and start the shaft inside the
/// model. A ray along the axis finds how much room there is; a gap with room
/// keeps the full length.
fn cone_length_within_gap(
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
    let probe = check_shaft_collision(
        start,
        start.add_scaled_vector(axis, length_mm),
        0.0,
        mesh.rays,
    );
    match probe.distance.filter(|_| probe.hit) {
        Some(distance) => js_max(
            MIN_CONE_LENGTH_MM,
            js_min(length_mm, distance - CONE_GAP_CLEARANCE_MM),
        ),
        None => length_mm,
    }
}

fn limit_to_surface(surface_normal: Vec3, axis: Vec3) -> Vec3 {
    clamp_cone_axis_deviation_from_surface_normal(
        surface_normal,
        axis,
        MAX_CONE_AXIS_DEVIATION_FROM_SURFACE_NORMAL_DEG,
    )
}

/// Where a cone starts on top of its disk, and how thick that disk is.
#[derive(Clone, Copy)]
struct Seat {
    disk_thickness: f64,
    cone_start: Vec3,
}

fn seat(pos: Vec3, surface_normal: Vec3, axis: Vec3, disk: &ContactDiskProfile) -> Seat {
    let disk_thickness = nib_length(surface_normal, axis, Some(disk));
    Seat {
        disk_thickness,
        cone_start: pos.add_scaled_vector(surface_normal, disk_thickness),
    }
}

/// Whether `a` sorts after `b` bottom to top (then by y, then by x).
fn sorts_after(a: Vec3, b: Vec3) -> bool {
    a.z > b.z || (a.z == b.z && (a.y > b.y || (a.y == b.y && a.x > b.x)))
}

pub fn build_stick(
    input: &StickBuildInput<'_>,
    settings: &SupportSettings,
    new_id: &mut dyn FnMut() -> String,
) -> StickBuildResult {
    let StickBuildInput {
        a_pos, b_pos, mesh, ..
    } = *input;
    let tip = &settings.tip;

    // The tier sizes an auto stick, never the active preset.
    let shaft_diameter = input
        .shaft_diameter_mm
        .unwrap_or(settings.shaft.diameter_mm);
    let tip_profile = SupportTipProfile {
        kind: Some(TipPrimitive::Disk),
        contact_diameter_mm: input
            .tip_contact_diameter_mm
            .unwrap_or(tip.contact_diameter_mm),
        body_diameter_mm: tip.body_diameter_mm,
        length_mm: tip.length_mm,
        penetration_mm: tip.penetration_mm,
        disk_thickness_mm: Some(tip.disk_thickness_mm.unwrap_or(0.1)),
        max_standoff_mm: Some(tip.max_standoff_mm.unwrap_or(1.5)),
        standoff_angle_threshold: Some(
            tip.standoff_angle_threshold
                .unwrap_or(std::f64::consts::FRAC_PI_4),
        ),
        break_point_mm: tip.breakpoint_mm.filter(|b| *b > 0.0),
    };
    let disk = tip_profile.as_disk_profile();
    let joint_diameter = get_joint_diameter(shaft_diameter);

    let unit_or_up = |v: Vec3| {
        if v.length_sq() < EPSILON {
            Vec3::new(0.0, 0.0, 1.0)
        } else {
            v.normalize()
        }
    };
    let normal_a = unit_or_up(input.a_normal);
    let normal_b = unit_or_up(input.b_normal);

    let mut span = b_pos.sub(a_pos);
    if span.length_sq() < EPSILON {
        span = normal_a;
    }
    let mut axis_a = span.normalize();
    let mut axis_b = axis_a.multiply_scalar(-1.0);

    // The disks stay on their surfaces, but each cone leans toward the line
    // between the two cone starts. Moving the axes moves the disks' stand-off,
    // and so the starts, which is why this settles over two passes.
    let mut seats;
    let mut passes = 0;
    loop {
        axis_a = limit_to_surface(normal_a, axis_a);
        axis_b = limit_to_surface(normal_b, axis_b);
        seats = (
            seat(a_pos, normal_a, axis_a, &disk),
            seat(b_pos, normal_b, axis_b, &disk),
        );
        passes += 1;
        let between = seats.1.cone_start.sub(seats.0.cone_start);
        if between.length_sq() < EPSILON {
            break;
        }
        axis_a = between.normalize();
        axis_b = axis_a.multiply_scalar(-1.0);
        if passes == 2 {
            break;
        }
    }
    let axis_a = limit_to_surface(normal_a, axis_a);
    let axis_b = limit_to_surface(normal_b, axis_b);
    let (seat_a, seat_b) = seats;

    // The lower end may carry a tip of its own (#360).
    let tip_for = |lower: bool| match settings.mesh_to_mesh.base_tip {
        Some(base) if lower => SupportTipProfile {
            contact_diameter_mm: base.diameter_mm,
            length_mm: base.length_mm,
            ..tip_profile
        },
        _ => tip_profile,
    };
    let a_is_lower = a_pos.z <= b_pos.z;
    let fit = |seat: Seat, axis: Vec3, tip: SupportTipProfile| SupportTipProfile {
        length_mm: cone_length_within_gap(seat.cone_start, axis, tip.length_mm, mesh),
        ..tip
    };
    let profile_a = fit(seat_a, axis_a, tip_for(a_is_lower));
    let profile_b = fit(seat_b, axis_b, tip_for(!a_is_lower));
    let socket_a = get_socket_position(seat_a.cone_start, axis_a, Some(&profile_a));
    let socket_b = get_socket_position(seat_b.cone_start, axis_b, Some(&profile_b));

    let joint_a = Joint {
        id: new_id(),
        pos: socket_a,
        diameter: joint_diameter,
    };
    let joint_b = Joint {
        id: new_id(),
        pos: socket_b,
        diameter: joint_diameter,
    };
    let shaft = Segment {
        bottom_joint: Some(joint_a.clone()),
        top_joint: Some(joint_b.clone()),
        ..Segment::straight(new_id(), shaft_diameter)
    };
    let mut cone =
        |pos: Vec3, normal: Vec3, axis: Vec3, seat: Seat, profile, joint: &Joint| ContactCone {
            id: new_id(),
            pos,
            normal: axis,
            surface_normal: Some(normal),
            disk_length_override: Some(seat.disk_thickness),
            placement_surface: None,
            profile,
            socket_joint_id: Some(joint.id.clone()),
        };
    let cone_a = cone(a_pos, normal_a, axis_a, seat_a, profile_a, &joint_a);
    let cone_b = cone(b_pos, normal_b, axis_b, seat_b, profile_b, &joint_b);

    // The cones are stored in a fixed order whichever end was given first.
    let (contact_cone_a, contact_cone_b) = if sorts_after(a_pos, b_pos) {
        (cone_b, cone_a)
    } else {
        (cone_a, cone_b)
    };

    let stick = Stick {
        base: SupportEntity {
            id: new_id(),
            model_id: input.model_id.to_owned(),
            ..SupportEntity::default()
        },
        segments: vec![shaft],
        contact_cone_a,
        contact_cone_b,
    };

    let error = mesh
        .filter(|mesh| free_span_blocked(socket_a, socket_b, shaft_diameter / 2.0, mesh))
        .map(|_| LimitationCode::CollisionWithModel);

    StickBuildResult { stick, error }
}

/// Whether the shaft's free span runs into the model.
///
/// Both shaft ends sit on the surfaces the stick bridges, within a radius of
/// material by construction, so only the span between them is tested, inset
/// from each socket. The cones are left out, as for every other type.
fn free_span_blocked(socket_a: Vec3, socket_b: Vec3, radius: f64, mesh: &ModelMesh<'_>) -> bool {
    let (dx, dy, dz) = (
        socket_b.x - socket_a.x,
        socket_b.y - socket_a.y,
        socket_b.z - socket_a.z,
    );
    let span = js_hypot(&[dx, dy, dz]);
    if span.is_nan() || span <= EPSILON {
        return false;
    }
    let inset = js_min(radius + 0.15, span * 0.4);
    let (ux, uy, uz) = (dx / span, dy / span, dz / span);
    is_shaft_blocked(
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
        radius,
        mesh.field,
    )
}
