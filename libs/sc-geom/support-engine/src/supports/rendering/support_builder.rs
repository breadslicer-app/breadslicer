//! `SupportData`: what a builder hands the drawing, and its errors.

use serde::{Deserialize, Serialize};

use crate::supports::shafts::{ContactRef, ShaftEntity};
use crate::supports::support_kinds::{ContactKind, contact_endpoints_for};
use crate::supports::tips::ContactCone;
use crate::supports::types::{
    ContactDisk, Knot, LimitationCode, Roots, Segment, Vec3, WarningCode,
};

/// What a builder hands back about a support: its parts (whichever it has),
/// and whether it could be built, with a warning or the angle it took.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportData {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roots: Option<Roots>,
    pub segments: Vec<Segment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_cone: Option<ContactCone>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_cones: Option<Vec<ContactCone>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_disks: Option<Vec<ContactDisk>>,
    /// The knot a leaf hangs from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knot: Option<Knot>,
    /// Where a branch starts (its knot).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_pos: Option<Vec3>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<LimitationCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<WarningCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angle: Option<f64>,
}

/// A support between two contacts (a twig or a stick) as [`SupportData`]:
/// its segments and its contacts, cones and disks as its kind declares them.
pub fn support_data_for_entity(entity: ShaftEntity<'_>) -> SupportData {
    let mut data = SupportData {
        id: entity.id().to_owned(),
        segments: entity.segments().to_vec(),
        ..SupportData::default()
    };
    for endpoint in contact_endpoints_for(entity.type_id()) {
        let Some(contact) = entity.contact(endpoint.field) else {
            continue;
        };
        debug_assert_eq!(
            endpoint.kind == ContactKind::Cone,
            matches!(contact, ContactRef::Cone(_))
        );
        match contact {
            ContactRef::Cone(cone) => data
                .contact_cones
                .get_or_insert_with(Vec::new)
                .push(cone.clone()),
            ContactRef::Disk(disk) => data
                .contact_disks
                .get_or_insert_with(Vec::new)
                .push(disk.clone()),
        }
    }
    data
}
