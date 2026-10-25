//! DragonFruit `src/supports/SupportTypes/Twig/twigBuilder.ts`.
//!
//! Settings are passed in (`getSettings()`), ids come from `new_id`
//! (`uuidv4()`), and the mesh is the model's [`ModelMesh`]; a twig asks it
//! rays only.

use serde::Serialize;

use super::twig_joint_standoff::twig_disk_joint_standoff;
use super::twig_taper::twig_joint_diameter_for_local_diameter;
use crate::supports::model_mesh::ModelMesh;
use crate::supports::placement_logic::collision_utils::check_short_bridge_collision;
use crate::supports::placement_logic::cone_axis_policy::{
    MAX_CONE_AXIS_DEVIATION_FROM_SURFACE_NORMAL_DEG, clamp_cone_axis_deviation_from_surface_normal,
};
use crate::supports::settings::types::SupportSettings;
use crate::supports::support_primitives::contact_cone::types::{ContactDiskProfile, DiskTag};
use crate::supports::types::{
    ContactDisk, Joint, LimitationCode, Segment, SupportEntity, Twig, Vec3,
};
use crate::three::math_utils::js_max;

/// A joint at a disk-end is 10% larger than that disk's contact diameter.
fn twig_joint_diameter_for_disk(disk_contact_diameter: f64) -> f64 {
    twig_joint_diameter_for_local_diameter(disk_contact_diameter)
}

#[derive(Clone, Copy)]
pub struct TwigBuildInput<'a> {
    pub model_id: &'a str,
    pub a_pos: Vec3,
    pub a_normal: Vec3,
    pub b_pos: Vec3,
    pub b_normal: Vec3,
    pub mesh: Option<ModelMesh<'a>>,
    /// Auto-support tier tip contact — absent for manual placement.
    pub tip_contact_diameter_mm: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TwigBuildResult {
    pub twig: Twig,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<LimitationCode>,
}

pub fn build_twig(
    input: &TwigBuildInput<'_>,
    settings: &SupportSettings,
    new_id: &mut dyn FnMut() -> String,
) -> TwigBuildResult {
    let TwigBuildInput {
        model_id,
        a_pos,
        a_normal,
        b_pos,
        b_normal,
        mesh,
        ..
    } = *input;

    let disk_profile = ContactDiskProfile {
        kind: DiskTag::Disk,
        disk_thickness_mm: Some(settings.tip.disk_thickness_mm.unwrap_or(0.1)),
        max_standoff_mm: Some(settings.tip.max_standoff_mm.unwrap_or(1.5)),
        standoff_angle_threshold: Some(
            settings
                .tip
                .standoff_angle_threshold
                .unwrap_or(std::f64::consts::PI / 4.0),
        ),
        contact_diameter_mm: None,
    };

    // Twig sizing rule: both disk ends share one diameter — twigs are
    // CYLINDRICAL, slightly smaller than the global tip contact (twigs are
    // short, low-span supports). The taper machinery still works; with equal
    // ends it degenerates to a cylinder.
    let disk_a_contact_diameter = input
        .tip_contact_diameter_mm
        .unwrap_or(settings.tip.contact_diameter_mm)
        * 0.9;
    let disk_b_contact_diameter = disk_a_contact_diameter;

    let joint_diameter_a = twig_joint_diameter_for_disk(disk_a_contact_diameter);
    let joint_diameter_b = twig_joint_diameter_for_disk(disk_b_contact_diameter);

    // Legacy uniform value for slicer/proxy and any consumer that reads
    // segment.diameter. The visible taper is carried per-end by the joints.
    let shaft_diameter = disk_a_contact_diameter;

    let mut axis_a = b_pos.sub(a_pos);
    if axis_a.length_sq() < 0.000001 {
        axis_a = Vec3::new(0.0, 0.0, 1.0);
    }
    axis_a = axis_a.normalize();
    let axis_b = axis_a.multiply_scalar(-1.0);

    let axis_a = clamp_cone_axis_deviation_from_surface_normal(
        a_normal,
        axis_a,
        MAX_CONE_AXIS_DEVIATION_FROM_SURFACE_NORMAL_DEG,
    );
    let axis_b = clamp_cone_axis_deviation_from_surface_normal(
        b_normal,
        axis_b,
        MAX_CONE_AXIS_DEVIATION_FROM_SURFACE_NORMAL_DEG,
    );

    // Joint stand-off scales with joint diameter so a large disk-end joint
    // never punches through the model.
    let disk_thickness_a =
        twig_disk_joint_standoff(a_normal, axis_a, &disk_profile, joint_diameter_a);
    let disk_thickness_b =
        twig_disk_joint_standoff(b_normal, axis_b, &disk_profile, joint_diameter_b);

    // Shaft connects to the center of the disk tip sphere at each end.
    let joint_pos_a = Vec3::new(
        a_pos.x + a_normal.x * disk_thickness_a,
        a_pos.y + a_normal.y * disk_thickness_a,
        a_pos.z + a_normal.z * disk_thickness_a,
    );
    let joint_pos_b = Vec3::new(
        b_pos.x + b_normal.x * disk_thickness_b,
        b_pos.y + b_normal.y * disk_thickness_b,
        b_pos.z + b_normal.z * disk_thickness_b,
    );

    let socket_joint_a = Joint {
        id: new_id(),
        pos: joint_pos_a,
        diameter: joint_diameter_a,
    };
    let socket_joint_b = Joint {
        id: new_id(),
        pos: joint_pos_b,
        diameter: joint_diameter_b,
    };

    let segment = Segment {
        bottom_joint: Some(socket_joint_a.clone()),
        top_joint: Some(socket_joint_b.clone()),
        ..Segment::straight(new_id(), shaft_diameter)
    };

    let contact_disk_a = ContactDisk {
        id: new_id(),
        pos: a_pos,
        surface_normal: a_normal,
        cone_axis: axis_a,
        disk_length_override: Some(disk_thickness_a),
        placement_surface: None,
        profile: disk_profile,
        contact_diameter_mm: disk_a_contact_diameter,
    };
    let contact_disk_b = ContactDisk {
        id: new_id(),
        pos: b_pos,
        surface_normal: b_normal,
        cone_axis: axis_b,
        disk_length_override: Some(disk_thickness_b),
        placement_surface: None,
        profile: disk_profile,
        contact_diameter_mm: disk_b_contact_diameter,
    };

    let twig = Twig {
        base: SupportEntity {
            id: new_id(),
            model_id: model_id.to_owned(),
            ..SupportEntity::default()
        },
        segments: vec![segment],
        contact_disk_a,
        contact_disk_b,
    };

    let mut error = None;
    if let Some(mesh) = mesh {
        // Twigs test with RAYS, not the signed SDF. A twig spans a gap between
        // two model surfaces, and the SDF resolves its sign from the nearest
        // triangle: across a gap between two facing faces the nearest one
        // changes, so the empty space reads as material and every SDF gate
        // refuses twigs by construction. Ray casts need no sign. The shaft
        // diameter already equals the disk contact diameter, so one cast at
        // that radius covers shaft and disks alike.
        let twig_radius = js_max(shaft_diameter, disk_a_contact_diameter) / 2.0;
        if check_short_bridge_collision(joint_pos_a, joint_pos_b, twig_radius, mesh.rays).hit {
            error = Some(LimitationCode::CollisionWithModel);
        }
    }

    TwigBuildResult { twig, error }
}
