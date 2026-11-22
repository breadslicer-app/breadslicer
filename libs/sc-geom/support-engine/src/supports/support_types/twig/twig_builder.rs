//! A twig: a thin cylinder strung between two model contacts, with a contact
//! disk at each end and no cone.

use serde::Serialize;

use super::twig_joint_standoff::twig_disk_joint_standoff;
use super::twig_taper::twig_joint_diameter_for_local_diameter;
use crate::supports::model_mesh::ModelMesh;
use crate::supports::placement_logic::collision_utils::check_short_bridge_collision;
use crate::supports::placement_logic::cone_axis_policy::{
    MAX_CONE_AXIS_DEVIATION_FROM_SURFACE_NORMAL_DEG, clamp_cone_axis_deviation_from_surface_normal,
};
use crate::supports::settings::types::SupportSettings;
use crate::supports::tips::{ContactDiskProfile, DiskTag};
use crate::supports::types::{
    ContactDisk, Joint, LimitationCode, Segment, SupportEntity, Twig, Vec3,
};

/// Twigs are short and low, so their disks are a little smaller than the
/// tip contact a support would get.
const TWIG_CONTACT_SCALE: f64 = 0.9;

#[derive(Clone, Copy, Debug)]
pub struct TwigBuildInput<'a> {
    pub model_id: &'a str,
    pub a_pos: Vec3,
    pub a_normal: Vec3,
    pub b_pos: Vec3,
    pub b_normal: Vec3,
    pub mesh: Option<ModelMesh<'a>>,
    /// Contact diameter of the auto-support tier; manual placement leaves it
    /// unset and takes the settings' tip.
    pub tip_contact_diameter_mm: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TwigBuildResult {
    pub twig: Twig,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<LimitationCode>,
}

/// One end of the twig: its disk's axis, how far the joint stands off the
/// contact, and where that joint sits.
struct TwigEnd {
    axis: Vec3,
    standoff: f64,
    joint_pos: Vec3,
}

fn twig_end(
    pos: Vec3,
    normal: Vec3,
    toward_other_end: Vec3,
    disk: &ContactDiskProfile,
    joint_diameter: f64,
) -> TwigEnd {
    let axis = clamp_cone_axis_deviation_from_surface_normal(
        normal,
        toward_other_end,
        MAX_CONE_AXIS_DEVIATION_FROM_SURFACE_NORMAL_DEG,
    );
    let standoff = twig_disk_joint_standoff(normal, axis, disk, joint_diameter);
    TwigEnd {
        axis,
        standoff,
        joint_pos: Vec3::new(
            pos.x + normal.x * standoff,
            pos.y + normal.y * standoff,
            pos.z + normal.z * standoff,
        ),
    }
}

pub fn build_twig(
    input: &TwigBuildInput<'_>,
    settings: &SupportSettings,
    new_id: &mut dyn FnMut() -> String,
) -> TwigBuildResult {
    let tip = &settings.tip;
    let disk = ContactDiskProfile {
        kind: DiskTag::Disk,
        disk_thickness_mm: Some(tip.disk_thickness_mm.unwrap_or(0.1)),
        max_standoff_mm: Some(tip.max_standoff_mm.unwrap_or(1.5)),
        standoff_angle_threshold: Some(
            tip.standoff_angle_threshold
                .unwrap_or(std::f64::consts::FRAC_PI_4),
        ),
        contact_diameter_mm: None,
    };

    // A twig is a plain cylinder: both disks and the shaft share one
    // diameter, and the joints at its ends are a little wider.
    let diameter = input
        .tip_contact_diameter_mm
        .unwrap_or(tip.contact_diameter_mm)
        * TWIG_CONTACT_SCALE;
    let joint_diameter = twig_joint_diameter_for_local_diameter(diameter);

    let mut span = input.b_pos.sub(input.a_pos);
    if span.length_sq() < 0.000001 {
        span = Vec3::new(0.0, 0.0, 1.0);
    }
    let a_to_b = span.normalize();
    let end_a = twig_end(input.a_pos, input.a_normal, a_to_b, &disk, joint_diameter);
    let end_b = twig_end(
        input.b_pos,
        input.b_normal,
        a_to_b.multiply_scalar(-1.0),
        &disk,
        joint_diameter,
    );

    let joint_a = Joint {
        id: new_id(),
        pos: end_a.joint_pos,
        diameter: joint_diameter,
    };
    let joint_b = Joint {
        id: new_id(),
        pos: end_b.joint_pos,
        diameter: joint_diameter,
    };
    let shaft = Segment {
        bottom_joint: Some(joint_a),
        top_joint: Some(joint_b),
        ..Segment::straight(new_id(), diameter)
    };
    let mut contact_disk = |pos: Vec3, surface_normal: Vec3, end: &TwigEnd| ContactDisk {
        id: new_id(),
        pos,
        surface_normal,
        cone_axis: end.axis,
        disk_length_override: Some(end.standoff),
        placement_surface: None,
        profile: disk,
        contact_diameter_mm: diameter,
    };
    let contact_disk_a = contact_disk(input.a_pos, input.a_normal, &end_a);
    let contact_disk_b = contact_disk(input.b_pos, input.b_normal, &end_b);

    let twig = Twig {
        base: SupportEntity {
            id: new_id(),
            model_id: input.model_id.to_owned(),
            ..SupportEntity::default()
        },
        segments: vec![shaft],
        contact_disk_a,
        contact_disk_b,
    };

    // A twig spans a gap between two facing surfaces, so it is tested with
    // rays: a signed distance field takes its sign from the nearest triangle,
    // which flips across such a gap and reads the air there as material. The
    // shaft is as wide as the disks, so one cast covers both.
    let error = input
        .mesh
        .filter(|mesh| {
            check_short_bridge_collision(
                end_a.joint_pos,
                end_b.joint_pos,
                diameter / 2.0,
                mesh.rays,
            )
            .hit
        })
        .map(|_| LimitationCode::CollisionWithModel);

    TwigBuildResult { twig, error }
}
