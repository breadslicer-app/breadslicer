//! DragonFruit `src/supports/rendering/SupportBuilder.tsx`, the data half.
//!
//! The React renderer is the GUI's; `SupportData` is what builders hand it.

use serde::{Deserialize, Serialize};

use crate::supports::support_primitives::contact_cone::types::ContactCone;
use crate::supports::support_primitives::knot::segment_endpoints::{ContactRef, ShaftEntity};
use crate::supports::support_type_registry::{ContactKind, SupportTypeId, contact_endpoints_for};
use crate::supports::types::{
    ContactDisk, Knot, LimitationCode, Roots, Segment, Vec3, WarningCode,
};

/// Generic support data that SupportBuilder can render.
/// This is agnostic to support type (trunk, branch, twig, etc.)
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportData {
    pub id: String,
    // Optional components - render what's present
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roots: Option<Roots>,
    pub segments: Vec<Segment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_cone: Option<ContactCone>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_cones: Option<Vec<ContactCone>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_disks: Option<Vec<ContactDisk>>,
    /// Optional knot (used by Leaf preview)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knot: Option<Knot>,
    /// For branches: starting position (from knot)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_pos: Option<Vec3>,
    // Validation state
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<LimitationCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<WarningCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angle: Option<f64>,
}

/// The renderable form of a bridging entity, keyed on its declared contacts.
///
/// Which fields hold an entity's contacts, and whether they are cones or disks,
/// is declared per type -- so a caller holding a `typeId` builds this without
/// naming either the type or its contact fields.
pub fn support_data_for_entity(entity: ShaftEntity<'_>) -> SupportData {
    let type_id: SupportTypeId = entity.type_id();
    let mut data = SupportData {
        id: entity.id().to_owned(),
        segments: entity.segments().to_vec(),
        ..SupportData::default()
    };

    for endpoint in contact_endpoints_for(type_id) {
        let Some(contact) = entity.contact(endpoint.field) else {
            continue;
        };
        // A declared cone field holds a cone and a disk field a disk.
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
