//! A leaf: a bare contact cone that hangs off a knot of another support, with
//! no shaft of its own. The cone runs from the model contact to the knot.

use bread_support_model::collision_avoidance::CollisionField;
use bread_support_model::rendering::support_builder::SupportData;
use bread_support_model::settings::types::SupportSettings;
use bread_support_model::support_kinds::SupportTypeId;
use bread_support_model::tips::{
    ContactCone, SupportTipProfile, TipPrimitive, recompute_contact_cone_for_moved_disk,
};
use bread_support_model::types::{Knot, Leaf, SupportEntity, Vec3};

#[derive(Clone, Copy, Debug)]
pub struct LeafBuildInput<'a> {
    pub tip_pos: Vec3,
    pub surface_normal: Vec3,
    pub model_id: &'a str,
    pub parent_knot: &'a Knot,
    pub host_diameter_mm: f64,
    /// Contact diameter of the auto-support tier; manual placement leaves it
    /// unset and takes the settings' tip.
    pub tip_contact_diameter_mm: Option<f64>,
    pub mesh: Option<&'a dyn CollisionField>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeafBuildResult {
    pub leaf: Leaf,
    pub support_data: SupportData,
}

/// A cone on the contact, aimed along `axis`, solved so its socket meets the
/// knot.
fn cone_onto_knot(
    input: &LeafBuildInput<'_>,
    axis: Vec3,
    profile: SupportTipProfile,
) -> ContactCone {
    let draft = ContactCone {
        id: "leaf-draft".to_owned(),
        pos: input.tip_pos,
        normal: axis,
        surface_normal: Some(input.surface_normal),
        profile,
        ..ContactCone::default()
    };
    recompute_contact_cone_for_moved_disk(
        &draft,
        input.tip_pos,
        input.surface_normal,
        Some(input.parent_knot.pos),
        input.mesh,
    )
}

pub fn build_leaf_data(
    input: &LeafBuildInput<'_>,
    settings: &SupportSettings,
    new_id: &mut dyn FnMut() -> String,
) -> LeafBuildResult {
    let knot = input.parent_knot;
    // The cone's base is narrower than the host where they meet (#360): a
    // tilted base as wide as the host would show past its sides.
    let base_diameter = input.host_diameter_mm * settings.auto_support.member_host_shaft_ratio;

    let first_profile = SupportTipProfile {
        kind: Some(TipPrimitive::Disk),
        contact_diameter_mm: input
            .tip_contact_diameter_mm
            .unwrap_or(settings.tip.contact_diameter_mm),
        body_diameter_mm: base_diameter,
        length_mm: settings.tip.length_mm,
        penetration_mm: settings.tip.penetration_mm,
        disk_thickness_mm: Some(0.1),
        max_standoff_mm: Some(1.5),
        standoff_angle_threshold: Some(std::f64::consts::FRAC_PI_4),
        break_point_mm: settings.tip.breakpoint_mm.filter(|b| *b > 0.0),
    };

    // Solve twice: once from the surface normal to find the axis and length
    // that reach the knot, then again from that axis with that length.
    let probe = cone_onto_knot(input, input.surface_normal, first_profile);
    let settled = cone_onto_knot(
        input,
        probe.normal,
        SupportTipProfile {
            length_mm: probe.profile.length_mm,
            ..first_profile
        },
    );
    let contact_cone = ContactCone {
        id: new_id(),
        ..settled
    };

    let id = new_id();
    LeafBuildResult {
        leaf: Leaf {
            base: SupportEntity {
                id: id.clone(),
                model_id: input.model_id.to_owned(),
                type_id: Some(SupportTypeId::Leaf),
                ..SupportEntity::default()
            },
            parent_knot_id: knot.id.clone(),
            contact_cone: contact_cone.clone(),
        },
        support_data: SupportData {
            id,
            segments: Vec::new(),
            knot: Some(knot.clone()),
            contact_cone: Some(contact_cone),
            ..SupportData::default()
        },
    }
}
