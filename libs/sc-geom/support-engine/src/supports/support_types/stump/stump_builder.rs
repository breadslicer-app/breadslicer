//! A stump: a short frustum standing on the plate, with a joint on top and a
//! cone stretched from that joint up to a contact close above the plate.

use serde::Serialize;

use crate::geom::Quaternion;
use crate::geom::math_utils::{js_max, js_min};
use crate::supports::placement_logic::collision_avoidance::CollisionField;
use crate::supports::placement_logic::cone_axis_policy::{
    ConeAxisPolicyInput, resolve_cone_axis_policy,
};
use crate::supports::rendering::support_builder::SupportData;
use crate::supports::settings::types::SupportSettings;
use crate::supports::support_kinds::inline_root_id;
use crate::supports::tips::{
    ContactCone, SupportTipProfile, TipPrimitive, recompute_contact_cone_for_moved_disk,
};
use crate::supports::types::{Joint, Roots, Stump, SupportEntity, Transform, Vec3};

const ROOT_BASE_DIAMETER_MM: f64 = 2.0;
const ROOT_TOP_DIAMETER_MM: f64 = 1.5;
const ROOT_HEIGHT_MM: f64 = 1.0;
const JOINT_DIAMETER_MM: f64 = 1.5;
/// Thickness of the disk the root stands on.
const ROOT_DISK_HEIGHT_MM: f64 = 0.1;
/// Height of the joint on top of the root: the disk plus the frustum.
const JOINT_Z_MM: f64 = ROOT_DISK_HEIGHT_MM + ROOT_HEIGHT_MM;

#[derive(Clone, Copy, Debug)]
pub struct StumpBuildInput<'a> {
    pub tip_pos: Vec3,
    pub tip_normal: Vec3,
    pub model_id: &'a str,
    pub mesh: Option<&'a dyn CollisionField>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StumpBuildResult {
    pub stump: Stump,
    pub support_data: SupportData,
}

/// How long the cone must be, along `axis` from the contact, to come down to
/// the joint's height: never shorter than the stock length, and never so
/// long on a falling axis that its end drops below the joint.
fn cone_length_to_joint(tip_pos: Vec3, axis: Vec3, stock_length: f64) -> f64 {
    let fall_per_mm = axis.z;
    let reach = if fall_per_mm.abs() > 1e-6 {
        (JOINT_Z_MM - tip_pos.z) / fall_per_mm
    } else {
        stock_length
    };
    let length = js_max(reach, stock_length);
    if fall_per_mm < -1e-6 {
        let longest = (tip_pos.z - JOINT_Z_MM) / -fall_per_mm;
        if longest > 0.0 {
            return js_min(length, longest);
        }
    }
    length
}

pub fn build_stump_data(
    input: &StumpBuildInput<'_>,
    settings: &SupportSettings,
    new_id: &mut dyn FnMut() -> String,
) -> StumpBuildResult {
    let StumpBuildInput {
        tip_pos,
        tip_normal,
        model_id,
        mesh,
    } = *input;
    let tip = &settings.tip;

    let axis = resolve_cone_axis_policy(ConeAxisPolicyInput {
        surface_normal: tip_normal,
        cone_angle_mode: tip.cone_angle_mode.unwrap_or_default(),
        adaptive_cone_angle_offset_deg: Some(tip.adaptive_cone_angle_offset_deg.unwrap_or(30.0)),
    })
    .cone_axis;
    let length = cone_length_to_joint(tip_pos, axis, tip.length_mm);

    // The cone's body is as wide as the joint it ends on.
    let profile = SupportTipProfile {
        kind: Some(TipPrimitive::Disk),
        contact_diameter_mm: tip.contact_diameter_mm,
        body_diameter_mm: JOINT_DIAMETER_MM - 0.1,
        length_mm: length,
        penetration_mm: tip.penetration_mm,
        disk_thickness_mm: Some(0.1),
        max_standoff_mm: Some(1.5),
        standoff_angle_threshold: Some(std::f64::consts::FRAC_PI_4),
        break_point_mm: tip.breakpoint_mm.filter(|b| *b > 0.0),
    };
    let aim = Vec3::new(
        tip_pos.x + axis.x * length,
        tip_pos.y + axis.y * length,
        tip_pos.z + axis.z * length,
    );
    let cone = recompute_contact_cone_for_moved_disk(
        &ContactCone {
            id: "stump-draft".to_owned(),
            pos: tip_pos,
            normal: axis,
            surface_normal: Some(tip_normal),
            disk_length_override: None,
            placement_surface: None,
            profile,
            socket_joint_id: None,
        },
        tip_pos,
        tip_normal,
        Some(aim),
        mesh,
    );

    // The socket at the cone's end: up the surface normal by the disk, then
    // along the axis by the cone.
    let surface = cone.surface_normal.unwrap_or(tip_normal);
    let disk = cone.disk_length_override.unwrap_or(0.0);
    let cone_length = cone.profile.length_mm;
    let socket_pos = Vec3::new(
        cone.pos.x + surface.x * disk + cone.normal.x * cone_length,
        cone.pos.y + surface.y * disk + cone.normal.y * cone_length,
        cone.pos.z + surface.z * disk + cone.normal.z * cone_length,
    );

    // The root stands plumb under the socket, with its joint on top.
    let root_pos = Vec3::new(socket_pos.x, socket_pos.y, 0.0);
    let joint = Joint {
        id: new_id(),
        pos: Vec3::new(socket_pos.x, socket_pos.y, JOINT_Z_MM),
        diameter: JOINT_DIAMETER_MM,
    };
    let socket_joint_id = new_id();
    let contact_cone = ContactCone {
        id: new_id(),
        socket_joint_id: Some(socket_joint_id),
        ..cone
    };

    let id = new_id();
    let entity = |id: String| SupportEntity {
        id,
        model_id: model_id.to_owned(),
        ..SupportEntity::default()
    };
    // The preview draws the root through a stand-in roots entity, the same
    // shape a placed stump draws.
    let preview_root = Roots {
        base: entity(inline_root_id(&id)),
        transform: Transform {
            pos: root_pos,
            rot: Quaternion::IDENTITY,
            scale: None,
        },
        diameter: ROOT_BASE_DIAMETER_MM,
        disk_height: ROOT_DISK_HEIGHT_MM,
        cone_height: ROOT_HEIGHT_MM,
        height: None,
        join_cone_mm: None,
    };
    let support_data = SupportData {
        id: id.clone(),
        roots: Some(preview_root),
        segments: Vec::new(),
        contact_cone: Some(contact_cone.clone()),
        ..SupportData::default()
    };
    let stump = Stump {
        base: entity(id),
        root_pos,
        root_base_diameter: ROOT_BASE_DIAMETER_MM,
        root_top_diameter: ROOT_TOP_DIAMETER_MM,
        root_height: ROOT_HEIGHT_MM,
        joint,
        segments: Vec::new(),
        contact_cone,
    };

    StumpBuildResult {
        stump,
        support_data,
    }
}
