//! DragonFruit `src/supports/SupportTypes/Leaf/leafBuilder.ts`.
//!
//! Settings are passed in (DragonFruit's `getSettings()`), ids come from
//! `new_id` (`uuidv4()`), and the mesh is its collision field.

use crate::supports::placement_logic::collision_avoidance::CollisionField;
use crate::supports::rendering::support_builder::SupportData;
use crate::supports::settings::types::SupportSettings;
use crate::supports::support_primitives::contact_cone::types::{
    ContactCone, SupportTipProfile, TipPrimitive,
};
use crate::supports::support_primitives::contact_disk::contact_disk_interaction::recompute_contact_cone_for_moved_disk;
use crate::supports::support_kinds::SupportTypeId;
use crate::supports::types::{Knot, Leaf, SupportEntity, Vec3};

#[derive(Clone, Copy, Debug)]
pub struct LeafBuildInput<'a> {
    pub tip_pos: Vec3,
    pub surface_normal: Vec3,
    pub model_id: &'a str,
    pub parent_knot: &'a Knot,
    pub host_diameter_mm: f64,
    /// Auto-support tier tip contact — absent for manual placement.
    pub tip_contact_diameter_mm: Option<f64>,
    pub mesh: Option<&'a dyn CollisionField>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeafBuildResult {
    pub leaf: Leaf,
    pub support_data: SupportData,
}

struct LeafConeShape {
    axis: Vec3,
    length_mm: f64,
}

fn compute_leaf_cone_axis_and_length(
    tip_pos: Vec3,
    surface_normal: Vec3,
    knot_pos: Vec3,
    base_profile: SupportTipProfile,
    mesh: Option<&dyn CollisionField>,
) -> LeafConeShape {
    let cone = recompute_contact_cone_for_moved_disk(
        &ContactCone {
            id: "preview-leaf-cone".to_owned(),
            pos: tip_pos,
            normal: surface_normal,
            surface_normal: Some(surface_normal),
            profile: base_profile,
            ..ContactCone::default()
        },
        tip_pos,
        surface_normal,
        Some(knot_pos),
        mesh,
    );

    LeafConeShape {
        axis: cone.normal,
        length_mm: cone.profile.length_mm,
    }
}

pub fn build_leaf_data(
    input: &LeafBuildInput<'_>,
    settings: &SupportSettings,
    new_id: &mut dyn FnMut() -> String,
) -> LeafBuildResult {
    let LeafBuildInput {
        tip_pos,
        surface_normal,
        model_id,
        parent_knot,
        host_diameter_mm,
        mesh,
        ..
    } = *input;
    // SoulCrafted (#360): a zone size's leaf is narrower than its host where
    // it meets it, or its tilted base would stick out of the host.
    let host_diameter_mm = if settings.auto_support.zone_rules.is_some() {
        host_diameter_mm * settings.auto_support.member_host_shaft_ratio
    } else {
        host_diameter_mm
    };
    // Auto-support tier override — absent for manual placement (global band).
    let tip_contact_diameter_mm = input
        .tip_contact_diameter_mm
        .unwrap_or(settings.tip.contact_diameter_mm);

    let base_profile = SupportTipProfile {
        kind: Some(TipPrimitive::Disk),
        contact_diameter_mm: tip_contact_diameter_mm,
        body_diameter_mm: host_diameter_mm,
        length_mm: settings.tip.length_mm,
        penetration_mm: settings.tip.penetration_mm,
        disk_thickness_mm: Some(0.1),
        max_standoff_mm: Some(1.5),
        standoff_angle_threshold: Some(std::f64::consts::PI / 4.0),
        break_point_mm: settings.tip.breakpoint_mm.filter(|b| *b > 0.0),
    };

    let LeafConeShape { axis, length_mm } = compute_leaf_cone_axis_and_length(
        tip_pos,
        surface_normal,
        parent_knot.pos,
        base_profile,
        mesh,
    );

    let profile = SupportTipProfile {
        length_mm,
        body_diameter_mm: host_diameter_mm,
        ..base_profile
    };

    let contact_cone = ContactCone {
        id: new_id(),
        ..recompute_contact_cone_for_moved_disk(
            &ContactCone {
                id: "leaf-cone".to_owned(),
                pos: tip_pos,
                normal: axis,
                surface_normal: Some(surface_normal),
                profile,
                ..ContactCone::default()
            },
            tip_pos,
            surface_normal,
            Some(parent_knot.pos),
            mesh,
        )
    };

    let leaf_id = new_id();
    let leaf = Leaf {
        base: SupportEntity {
            id: leaf_id.clone(),
            model_id: model_id.to_owned(),
            type_id: Some(SupportTypeId::Leaf),
            ..SupportEntity::default()
        },
        parent_knot_id: parent_knot.id.clone(),
        contact_cone: contact_cone.clone(),
    };

    let support_data = SupportData {
        id: leaf_id,
        segments: Vec::new(),
        knot: Some(parent_knot.clone()),
        contact_cone: Some(contact_cone),
        ..SupportData::default()
    };

    LeafBuildResult { leaf, support_data }
}
