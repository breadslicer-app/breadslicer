//! What auto-placement builds for a contact in the stump's band. A contact
//! gets a trunk by default; contacts close above the plate, the band the
//! stump's descriptor claims by tip height, get a stump instead, checked here
//! against the model since the grid's own trunk checks never see it.

use indexmap::IndexMap;
use serde::Serialize;

use super::stump_builder::{StumpBuildInput, build_stump_data};
use support_geom::geom::math_utils::{js_cos, js_hypot, js_max, js_min, js_sin};
use support_model::collision_avoidance::CollisionField;
use support_model::rendering::support_builder::SupportData;
use support_model::settings::types::SupportSettings;
use support_model::support_kinds::{
    PlacedSupport, SupportEntityValue, SupportTypeId, placement_of_resolved,
};
use support_model::tips::get_final_socket_position;
use support_model::types::{LimitationCode, Stump, Vec3};

/// The closest the stump's surface may come to the model, in mm.
const CLEARANCE_MM: f64 = 0.05;
/// How much of the cone, from its contact end, is not tested: the disk there
/// sits on the model by design.
const CONE_CONTACT_SKIP_MM: f64 = 0.35;
/// The most rings tested along a body.
const MAX_RINGS: f64 = 5.0;
/// Diameters tested around each ring; both ends of each one count.
const RING_DIAMETERS: u32 = 4;

const AXES: [Vec3; 3] = [
    Vec3::new(1.0, 0.0, 0.0),
    Vec3::new(0.0, 1.0, 0.0),
    Vec3::new(0.0, 0.0, 1.0),
];

/// Two unit vectors square to the unit `axis` and to each other.
fn ring_basis(ax: f64, ay: f64, az: f64) -> (Vec3, Vec3) {
    let helper = if az.abs() > 0.9 { AXES[0] } else { AXES[2] };
    let mut u = Vec3::new(
        ay * helper.z - az * helper.y,
        az * helper.x - ax * helper.z,
        ax * helper.y - ay * helper.x,
    );
    let u_length = js_hypot(&[u.x, u.y, u.z]);
    let u_length = if u_length == 0.0 || u_length.is_nan() {
        1.0
    } else {
        u_length
    };
    u.x /= u_length;
    u.y /= u_length;
    u.z /= u_length;
    let v = Vec3::new(
        ay * u.z - az * u.y,
        az * u.x - ax * u.z,
        ax * u.y - ay * u.x,
    );
    (u, v)
}

/// Whether a frustum from `from` to `to` (radius `from_radius` to `to_radius`)
/// comes within the clearance of the model.
///
/// Points on the frustum's surface are tested, not on its axis: a cone under a
/// flat ceiling only touches it, and an axis distance held against the local
/// radius would count that touch as a crossing. The first `skip_mm` along the
/// body are left out. Distances come from the exact signed field, since these
/// margins are finer than the cached grid can tell apart.
fn frustum_touches_model(
    field: &dyn CollisionField,
    from: Vec3,
    to: Vec3,
    from_radius: f64,
    to_radius: f64,
    skip_mm: f64,
) -> bool {
    let (dx, dy, dz) = (to.x - from.x, to.y - from.y, to.z - from.z);
    let length = js_hypot(&[dx, dy, dz]);
    if length <= 1e-6 {
        return field.exact_signed_distance_at(to) < js_max(from_radius, to_radius) + CLEARANCE_MM;
    }

    let (u, v) = ring_basis(dx / length, dy / length, dz / length);
    let start = js_min(1.0, skip_mm / length);
    let span = 1.0 - start;
    let rings = js_max(1.0, js_min(MAX_RINGS, ((span * length) / 0.4).ceil() + 1.0));

    let mut ring = 0.0;
    while ring <= rings {
        let t = start + (span * ring) / rings;
        let radius = from_radius + (to_radius - from_radius) * t;
        let centre = (from.x + dx * t, from.y + dy * t, from.z + dz * t);
        // Both ends of every diameter: a cone under a ceiling meets it with
        // its upper side, one beside a wall with the side facing it.
        for k in 0..RING_DIAMETERS {
            let angle = (f64::from(k) / f64::from(RING_DIAMETERS)) * std::f64::consts::PI;
            let (cos, sin) = (js_cos(angle), js_sin(angle));
            let dir = (
                u.x * cos + v.x * sin,
                u.y * cos + v.y * sin,
                u.z * cos + v.z * sin,
            );
            for sign in [1.0, -1.0] {
                let p = Vec3::new(
                    centre.0 + dir.0 * radius * sign,
                    centre.1 + dir.1 * radius * sign,
                    centre.2 + dir.2 * radius * sign,
                );
                if field.exact_signed_distance_at(p) < CLEARANCE_MM {
                    return true;
                }
            }
        }
        ring += 1.0;
    }
    false
}

/// Whether a ball of `radius` at `centre` comes within the clearance of the
/// model: its centre, then its six poles.
fn ball_touches_model(field: &dyn CollisionField, centre: Vec3, radius: f64) -> bool {
    if field.exact_signed_distance_at(centre) < radius + CLEARANCE_MM {
        return true;
    }
    AXES.iter().any(|axis| {
        [1.0, -1.0].into_iter().any(|sign| {
            let p = Vec3::new(
                centre.x + axis.x * radius * sign,
                centre.y + axis.y * radius * sign,
                centre.z + axis.z * radius * sign,
            );
            field.exact_signed_distance_at(p) < CLEARANCE_MM
        })
    })
}

/// Whether a built stump runs into the model: its cone, its joint, then its
/// root. The root is tested from the plate up rather than taken as clear,
/// since a model standing on the plate can overhang it.
pub fn stump_model_collision(
    stump: &Stump,
    mesh: Option<&dyn CollisionField>,
) -> Option<LimitationCode> {
    let field = mesh?;
    let radius = |diameter: f64| js_max(0.001, diameter / 2.0);
    let profile = &stump.contact_cone.profile;
    let socket = stump.joint.pos;

    let touches = frustum_touches_model(
        field,
        stump.contact_cone.pos,
        socket,
        radius(profile.contact_diameter_mm),
        radius(profile.body_diameter_mm),
        CONE_CONTACT_SKIP_MM,
    ) || ball_touches_model(field, socket, radius(stump.joint.diameter))
        || frustum_touches_model(
            field,
            Vec3::new(socket.x, socket.y, 0.0),
            socket,
            radius(stump.root_base_diameter),
            radius(stump.root_top_diameter),
            0.0,
        );
    touches.then_some(LimitationCode::CollisionWithModel)
}

/// A contact handed to a type that builds its own support for it.
#[derive(Clone, Copy, Debug)]
pub struct ContactOverrideRequest<'a> {
    pub tip_pos: Vec3,
    pub tip_normal: Vec3,
    pub model_id: &'a str,
    /// Checked for clearance when given.
    pub mesh: Option<&'a dyn CollisionField>,
}

/// The stump built for a contact.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StumpOverrideResult {
    /// The stump, with no separate parts: its frustum root is the support.
    pub placed: PlacedSupport,
    /// Why the stump turns this contact down, when it does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<LimitationCode>,
    /// The preview, carrying the refusal as its error so it can be explained.
    pub support_data: SupportData,
}

pub fn build_stump_contact_override(
    request: &ContactOverrideRequest<'_>,
    settings: &SupportSettings,
    new_id: &mut dyn FnMut() -> String,
) -> StumpOverrideResult {
    let built = build_stump_data(
        &StumpBuildInput {
            tip_pos: request.tip_pos,
            tip_normal: request.tip_normal,
            model_id: request.model_id,
            mesh: request.mesh,
        },
        settings,
        new_id,
    );
    let mut support_data = built.support_data;
    let refusal = cone_dips_below_joint(&built.stump)
        .then_some(LimitationCode::StumpBelowRoot)
        .or_else(|| stump_model_collision(&built.stump, request.mesh));
    if refusal.is_some() {
        support_data.error = refusal;
    }

    StumpOverrideResult {
        placed: placement_of_resolved(
            SupportTypeId::Stump,
            SupportEntityValue::Stump(built.stump),
            IndexMap::new(),
            None,
        ),
        refusal,
        support_data,
    }
}

/// Whether the cone reaches below the root's joint: a long cone on a falling
/// axis would carry the shaft under the root, below the plate.
fn cone_dips_below_joint(stump: &Stump) -> bool {
    let cone = &stump.contact_cone;
    let lowest = js_min(cone.pos.z, get_final_socket_position(cone).z);
    lowest < stump.joint.pos.z - 1e-3
}
