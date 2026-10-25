//! DragonFruit `src/supports/SupportTypes/Stump/stumpAutoPlacement.ts`.
//!
//! The stump's build, overriding auto-placement's default for its band
//! (`registerContactOverride('stump', ...)`). Auto-placement stands a trunk on
//! a contact by default; a stump claims the near-plate band through the
//! `tipHeight` rule on its descriptor and puts a stub there instead.

use indexmap::IndexMap;
use serde::Serialize;

use super::stump_builder::{StumpBuildInput, build_stump_data};
use crate::supports::placement_logic::collision_avoidance::CollisionField;
use crate::supports::rendering::support_builder::SupportData;
use crate::supports::settings::types::SupportSettings;
use crate::supports::support_primitives::contact_cone::contact_cone_utils::get_final_socket_position;
use crate::supports::support_type_registry::{
    PlacedSupport, SupportEntityValue, SupportTypeId, placement_of_resolved,
};
use crate::supports::types::{LimitationCode, Stump, Vec3};
use crate::three::math_utils::{js_cos, js_hypot, js_max, js_min, js_sin};

/// How close to the model a stump's body may come, in mm.
const STUMP_COLLISION_SAFETY_MM: f64 = 0.05;
/// Length of the cone's tip region skipped when sampling, in mm.
const STUMP_TIP_SKIP_MM: f64 = 0.35;
/// Samples along a body axis, and directions around it.
const STUMP_BODY_SAMPLES: f64 = 5.0;
const STUMP_BODY_DIRECTIONS: u32 = 4;

/// Does a tapered body between two world points reach the model?
///
/// The body's *surface* is sampled, not its axis: a cone standing under a flat
/// ceiling is tangent there, and comparing an axis sample's distance against
/// the local radius reads that tangency as an intersection. `skip_mm` steps
/// past the body's own attachment region — the contact disk is *on* the model.
///
/// Distances come from the exact signed field: the stump's margins are finer
/// than the quantized grid's cell-centre substitution error.
fn body_reaches_model(
    sdf: &dyn CollisionField,
    from: Vec3,
    to: Vec3,
    from_radius: f64,
    to_radius: f64,
    skip_mm: f64,
) -> bool {
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    let dz = to.z - from.z;
    let length = js_hypot(&[dx, dy, dz]);
    if length <= 1e-6 {
        return sdf.exact_signed_distance_at(to)
            < js_max(from_radius, to_radius) + STUMP_COLLISION_SAFETY_MM;
    }

    // Two axes perpendicular to the body's own, for sampling around it.
    let ax = dx / length;
    let ay = dy / length;
    let az = dz / length;
    let helper = if az.abs() > 0.9 {
        Vec3::new(1.0, 0.0, 0.0)
    } else {
        Vec3::new(0.0, 0.0, 1.0)
    };
    let mut side1 = Vec3::new(
        ay * helper.z - az * helper.y,
        az * helper.x - ax * helper.z,
        ax * helper.y - ay * helper.x,
    );
    let side1_length = match js_hypot(&[side1.x, side1.y, side1.z]) {
        l if l == 0.0 || l.is_nan() => 1.0,
        l => l,
    };
    side1.x /= side1_length;
    side1.y /= side1_length;
    side1.z /= side1_length;
    let side2 = Vec3::new(
        ay * side1.z - az * side1.y,
        az * side1.x - ax * side1.z,
        ax * side1.y - ay * side1.x,
    );

    let first = js_min(1.0, skip_mm / length);
    let span = 1.0 - first;
    let steps = js_max(
        1.0,
        js_min(STUMP_BODY_SAMPLES, ((span * length) / 0.4).ceil() + 1.0),
    );

    let mut i = 0.0;
    while i <= steps {
        let t = first + (span * i) / steps;
        let radius = from_radius + (to_radius - from_radius) * t;
        let cx = from.x + dx * t;
        let cy = from.y + dy * t;
        let cz = from.z + dz * t;

        // Both ends of each direction: a cone under a ceiling is caught by its
        // upper side, one beside a wall by the side that faces it.
        for d in 0..STUMP_BODY_DIRECTIONS {
            let angle = (f64::from(d) / f64::from(STUMP_BODY_DIRECTIONS)) * std::f64::consts::PI;
            let ux = side1.x * js_cos(angle) + side2.x * js_sin(angle);
            let uy = side1.y * js_cos(angle) + side2.y * js_sin(angle);
            let uz = side1.z * js_cos(angle) + side2.z * js_sin(angle);
            for sign in [1.0, -1.0] {
                let p = Vec3::new(
                    cx + ux * radius * sign,
                    cy + uy * radius * sign,
                    cz + uz * radius * sign,
                );
                if sdf.exact_signed_distance_at(p) < STUMP_COLLISION_SAFETY_MM {
                    return true;
                }
            }
        }
        i += 1.0;
    }

    false
}

/// Does the joint ball at the socket reach the model?
fn joint_reaches_model(sdf: &dyn CollisionField, socket: Vec3, radius: f64) -> bool {
    let margin = radius + STUMP_COLLISION_SAFETY_MM;
    if sdf.exact_signed_distance_at(socket) < margin {
        return true;
    }

    for axis in [
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(0.0, 0.0, 1.0),
    ] {
        for sign in [1.0, -1.0] {
            let p = Vec3::new(
                socket.x + axis.x * radius * sign,
                socket.y + axis.y * radius * sign,
                socket.z + axis.z * radius * sign,
            );
            if sdf.exact_signed_distance_at(p) < STUMP_COLLISION_SAFETY_MM {
                return true;
            }
        }
    }

    false
}

/// Whether a built stump's body runs into the model.
///
/// The grid's own collision gate never sees a stump (a type with its own
/// contact override returns before the trunk checks), so the check lives here.
/// The root column is checked from the plate to the joint rather than assumed
/// clear: a model on the plate can overhang the root's own column.
pub fn stump_model_collision(
    stump: &Stump,
    mesh: Option<&dyn CollisionField>,
) -> Option<LimitationCode> {
    let sdf = mesh?;

    let contact = stump.contact_cone.pos;
    let socket = stump.joint.pos;

    let cone_reaches = body_reaches_model(
        sdf,
        contact,
        socket,
        js_max(0.001, stump.contact_cone.profile.contact_diameter_mm / 2.0),
        js_max(0.001, stump.contact_cone.profile.body_diameter_mm / 2.0),
        STUMP_TIP_SKIP_MM,
    );
    if cone_reaches {
        return Some(LimitationCode::CollisionWithModel);
    }

    if joint_reaches_model(sdf, socket, js_max(0.001, stump.joint.diameter / 2.0)) {
        return Some(LimitationCode::CollisionWithModel);
    }

    let foot = Vec3::new(socket.x, socket.y, 0.0);
    let root_reaches = body_reaches_model(
        sdf,
        foot,
        socket,
        js_max(0.001, stump.root_base_diameter / 2.0),
        js_max(0.001, stump.root_top_diameter / 2.0),
        0.0,
    );
    root_reaches.then_some(LimitationCode::CollisionWithModel)
}

/// `ContactOverrideRequest`.
#[derive(Clone, Copy)]
pub struct ContactOverrideRequest<'a> {
    pub tip_pos: Vec3,
    pub tip_normal: Vec3,
    pub model_id: &'a str,
    /// Checked for clearance when given.
    pub mesh: Option<&'a dyn CollisionField>,
}

/// `ContactOverrideResult` for the stump.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StumpOverrideResult {
    /// A stump declares no `edges`: its frustum root IS the support, so it
    /// carries no separate primitive into the draft (`supplied: {}`).
    pub placed: PlacedSupport,
    /// Set when the stump refuses this contact; passed through as the rejection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<LimitationCode>,
    /// The ghost, with the refusal as its `error` so the tooltip explains it.
    pub support_data: SupportData,
}

fn placed(stump: Stump) -> PlacedSupport {
    placement_of_resolved(
        SupportTypeId::Stump,
        SupportEntityValue::Stump(stump),
        IndexMap::new(),
        None,
    )
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
    let refused = |stump: Stump, mut support_data: SupportData, code: LimitationCode| {
        support_data.error = Some(code);
        StumpOverrideResult {
            placed: placed(stump),
            refusal: Some(code),
            support_data,
        }
    };

    // The cone body spans contact disk → socket and must never dip below the
    // root joint: an over-long cone on a downward axis pushes the shaft below
    // the root, into -Z.
    let joint_z = built.stump.joint.pos.z;
    let lowest_shaft_z = js_min(
        built.stump.contact_cone.pos.z,
        get_final_socket_position(&built.stump.contact_cone).z,
    );
    if lowest_shaft_z < joint_z - 1e-3 {
        return refused(
            built.stump,
            built.support_data,
            LimitationCode::StumpBelowRoot,
        );
    }

    if let Some(collision) = stump_model_collision(&built.stump, request.mesh) {
        return refused(built.stump, built.support_data, collision);
    }

    StumpOverrideResult {
        placed: placed(built.stump),
        refusal: None,
        support_data: built.support_data,
    }
}
