//! DragonFruit `src/supports/SupportTypes/Stump/stumpBuilder.ts`.
//!
//! Settings are passed in (`getSettings()`), ids come from `new_id`
//! (`uuidv4()`), and the mesh is the model's collision field. DragonFruit
//! also reads the raft settings here, into values the build never uses (the
//! stump's vertical offset is fixed at 0), so they are not asked.

use serde::Serialize;

use crate::supports::placement_logic::collision_avoidance::CollisionField;
use crate::supports::placement_logic::cone_axis_policy::{
    ConeAxisPolicyInput, resolve_cone_axis_policy,
};
use crate::supports::rendering::support_builder::SupportData;
use crate::supports::settings::support_settings_codec::encode_support_settings_hex;
use crate::supports::settings::types::SupportSettings;
use crate::supports::support_primitives::contact_cone::types::{
    ContactCone, SupportTipProfile, TipPrimitive,
};
use crate::supports::support_primitives::contact_disk::contact_disk_interaction::recompute_contact_cone_for_moved_disk;
use crate::supports::support_type_registry::inline_root_id;
use crate::supports::types::{Joint, Roots, Stump, SupportEntity, Transform, Vec3};
use crate::three::Quaternion;
use crate::three::math_utils::{js_max, js_min};

const STUMP_ROOT_BASE_DIAMETER_MM: f64 = 2.0;
const STUMP_ROOT_TOP_DIAMETER_MM: f64 = 1.5;
const STUMP_ROOT_HEIGHT_MM: f64 = 1.0;
const STUMP_JOINT_DIAMETER_MM: f64 = 1.5;
const ANCHOR_DISK_HEIGHT_MM: f64 = 0.1;

#[derive(Clone, Copy)]
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

    let settings_code_hex = encode_support_settings_hex(settings);
    let cone_angle_mode = settings.tip.cone_angle_mode.unwrap_or_default();
    let adaptive_cone_angle_offset_deg =
        settings.tip.adaptive_cone_angle_offset_deg.unwrap_or(30.0);

    let effective_cone_axis = resolve_cone_axis_policy(ConeAxisPolicyInput {
        surface_normal: tip_normal,
        cone_angle_mode,
        adaptive_cone_angle_offset_deg: Some(adaptive_cone_angle_offset_deg),
    })
    .cone_axis;
    let tip_profile = SupportTipProfile {
        kind: Some(TipPrimitive::Disk),
        contact_diameter_mm: settings.tip.contact_diameter_mm,
        body_diameter_mm: settings.tip.body_diameter_mm,
        length_mm: settings.tip.length_mm,
        penetration_mm: settings.tip.penetration_mm,
        disk_thickness_mm: Some(0.1),
        max_standoff_mm: Some(1.5),
        standoff_angle_threshold: Some(std::f64::consts::PI / 4.0),
        sphere_radius_ratio: None,
        break_point_mm: settings.tip.breakpoint_mm.filter(|b| *b > 0.0),
    };

    // Sphere center Z = verticalOffset + effectiveDiskHeight + coneHeight
    // (sphere center is at top of cone, not offset by radius); the offset is 0.
    let vertical_offset = 0.0;
    let target_socket_z = vertical_offset + ANCHOR_DISK_HEIGHT_MM + STUMP_ROOT_HEIGHT_MM;
    let dz_per_unit = effective_cone_axis.z;
    let cone_length = if dz_per_unit.abs() > 1e-6 {
        (target_socket_z - tip_pos.z) / dz_per_unit
    } else {
        tip_profile.length_mm // fallback if cone axis is nearly horizontal
    };

    // Use the stretched length (but never shorter than the default) — and never
    // so long that the socket overshoots BELOW the root joint.
    let mut effective_cone_length = js_max(cone_length, tip_profile.length_mm);
    if dz_per_unit < -1e-6 {
        let max_cone_length = (tip_pos.z - target_socket_z) / -dz_per_unit;
        if max_cone_length > 0.0 {
            effective_cone_length = js_min(effective_cone_length, max_cone_length);
        }
    }

    let guessed_socket_pos = Vec3::new(
        tip_pos.x + effective_cone_axis.x * effective_cone_length,
        tip_pos.y + effective_cone_axis.y * effective_cone_length,
        tip_pos.z + effective_cone_axis.z * effective_cone_length,
    );
    // Stretched length + body diameter matching the joint.
    let stump_tip_profile = SupportTipProfile {
        length_mm: effective_cone_length,
        body_diameter_mm: STUMP_JOINT_DIAMETER_MM - 0.1,
        ..tip_profile
    };
    let authored_cone = recompute_contact_cone_for_moved_disk(
        &ContactCone {
            id: "preview-stump-cone".to_owned(),
            pos: tip_pos,
            normal: effective_cone_axis,
            surface_normal: Some(tip_normal),
            disk_length_override: None,
            placement_surface: None,
            profile: stump_tip_profile,
            socket_joint_id: None,
        },
        tip_pos,
        tip_normal,
        Some(guessed_socket_pos),
        mesh,
    );
    let surface = authored_cone.surface_normal.unwrap_or(tip_normal);
    let disk = authored_cone.disk_length_override.unwrap_or(0.0);
    let length = authored_cone.profile.length_mm;
    let socket_pos = Vec3::new(
        authored_cone.pos.x + surface.x * disk + authored_cone.normal.x * length,
        authored_cone.pos.y + surface.y * disk + authored_cone.normal.y * length,
        authored_cone.pos.z + surface.z * disk + authored_cone.normal.z * length,
    );

    // Root and joint are positioned at the socket XY, vertical stack
    let root_pos = Vec3::new(socket_pos.x, socket_pos.y, 0.0);
    let joint_pos = Vec3::new(socket_pos.x, socket_pos.y, target_socket_z);
    let joint = Joint {
        id: new_id(),
        pos: joint_pos,
        diameter: STUMP_JOINT_DIAMETER_MM,
    };
    let socket_joint = Joint {
        id: new_id(),
        pos: socket_pos,
        diameter: STUMP_JOINT_DIAMETER_MM,
    };

    let contact_cone = ContactCone {
        id: new_id(),
        socket_joint_id: Some(socket_joint.id.clone()),
        ..authored_cone
    };

    let stump_id = new_id();
    let stump = Stump {
        base: SupportEntity {
            id: stump_id.clone(),
            model_id: model_id.to_owned(),
            settings_code_hex: Some(settings_code_hex),
            ..SupportEntity::default()
        },
        root_pos,
        root_base_diameter: STUMP_ROOT_BASE_DIAMETER_MM,
        root_top_diameter: STUMP_ROOT_TOP_DIAMETER_MM,
        root_height: STUMP_ROOT_HEIGHT_MM,
        joint,
        segments: Vec::new(),
        contact_cone: contact_cone.clone(),
    };

    let support_data = SupportData {
        id: stump_id.clone(),
        // Synthetic Roots entity so the generic preview renders the root cone
        // exactly like the stump renderer does for placed stumps.
        roots: Some(Roots {
            base: SupportEntity {
                id: inline_root_id(&stump_id),
                model_id: model_id.to_owned(),
                ..SupportEntity::default()
            },
            transform: Transform {
                pos: root_pos,
                rot: Quaternion::IDENTITY,
                scale: None,
            },
            diameter: STUMP_ROOT_BASE_DIAMETER_MM,
            disk_height: ANCHOR_DISK_HEIGHT_MM,
            cone_height: STUMP_ROOT_HEIGHT_MM,
            height: None,
            join_cone_mm: None,
        }),
        segments: Vec::new(),
        contact_cone: Some(contact_cone),
        ..SupportData::default()
    };

    StumpBuildResult {
        stump,
        support_data,
    }
}
