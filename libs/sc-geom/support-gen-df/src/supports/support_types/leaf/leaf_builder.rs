//! DragonFruit `src/supports/SupportTypes/Leaf/leafBuilder.ts`.
//!
//! Settings are passed in (DragonFruit's `getSettings()`), ids come from
//! `new_id` (`uuidv4()`), and the mesh is its collision field.

use crate::supports::auto_support::parameter_sizing::{
    SizeOverrides, apply_sizing_overrides_to_settings,
};
use crate::supports::placement_logic::collision_avoidance::CollisionField;
use crate::supports::rendering::support_builder::SupportData;
use crate::supports::settings::support_settings_codec::encode_support_settings_hex;
use crate::supports::settings::types::SupportSettings;
use crate::supports::support_primitives::contact_cone::types::{
    ContactCone, SupportTipProfile, TipPrimitive,
};
use crate::supports::support_primitives::contact_disk::contact_disk_interaction::recompute_contact_cone_for_moved_disk;
use crate::supports::support_type_registry::SupportTypeId;
use crate::supports::types::{Knot, Leaf, SupportEntity, Vec3};

#[derive(Clone, Copy)]
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
    // Auto-support tier override — absent for manual placement (global band).
    let tip_contact_diameter_mm = input
        .tip_contact_diameter_mm
        .unwrap_or(settings.tip.contact_diameter_mm);
    let settings_code_hex = encode_support_settings_hex(&apply_sizing_overrides_to_settings(
        settings,
        Some(&SizeOverrides {
            tip_contact_diameter_mm: input.tip_contact_diameter_mm,
            tip_body_diameter_mm: Some(input.host_diameter_mm),
            ..SizeOverrides::default()
        }),
    ));

    let base_profile = SupportTipProfile {
        kind: Some(TipPrimitive::Disk),
        contact_diameter_mm: tip_contact_diameter_mm,
        body_diameter_mm: host_diameter_mm,
        length_mm: settings.tip.length_mm,
        penetration_mm: settings.tip.penetration_mm,
        disk_thickness_mm: Some(0.1),
        max_standoff_mm: Some(1.5),
        standoff_angle_threshold: Some(std::f64::consts::PI / 4.0),
        sphere_radius_ratio: None,
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
            settings_code_hex: Some(settings_code_hex),
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
