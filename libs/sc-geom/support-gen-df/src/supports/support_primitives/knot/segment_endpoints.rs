//! DragonFruit `src/supports/SupportPrimitives/Knot/segmentEndpoints.ts`.
//!
//! Where a segment starts and ends in world space, for any shafted type.
//!
//! The declared `lower` endpoint anchors segment 0 -- a root's top, or a parent
//! knot -- and `upper` says what the shaft ends at.

use crate::supports::support_primitives::contact_cone::contact_cone_utils::get_final_socket_position;
use crate::supports::support_primitives::contact_cone::types::ContactCone;
use crate::supports::support_type_registry::{
    SupportEndpoint, SupportEndpointKind, SupportTypeDescriptor, SupportTypeId,
    get_support_type_descriptor,
};
use crate::supports::types::{
    Branch, ContactDisk, Kickstand, Knot, Roots, Segment, Stick, Stump, Trunk, Twig, Vec3,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentEndpoints {
    pub start: Vec3,
    pub end: Vec3,
}

/// The pieces a caller has to hand that the entity itself cannot supply.
#[derive(Debug, Clone, Copy, Default)]
pub struct EndpointHosts<'a> {
    /// The root this type owns, for types declaring `owns_root`.
    pub root: Option<&'a Roots>,
    /// The knot this type hangs from, for types with a `hostedBy` knot edge.
    pub host_knot: Option<&'a Knot>,
}

/// A contact primitive read off an entity by its declared field.
#[derive(Debug, Clone, Copy)]
pub enum ContactRef<'a> {
    Cone(&'a ContactCone),
    Disk(&'a ContactDisk),
}

/// Any shafted entity. Contact fields are read by the names the registry declares.
#[derive(Debug, Clone, Copy)]
pub enum ShaftEntity<'a> {
    Trunk(&'a Trunk),
    Branch(&'a Branch),
    Twig(&'a Twig),
    Stick(&'a Stick),
    Stump(&'a Stump),
    Kickstand(&'a Kickstand),
}

impl<'a> ShaftEntity<'a> {
    pub fn id(&self) -> &'a str {
        match self {
            ShaftEntity::Trunk(e) => &e.base.id,
            ShaftEntity::Branch(e) => &e.base.id,
            ShaftEntity::Twig(e) => &e.base.id,
            ShaftEntity::Stick(e) => &e.base.id,
            ShaftEntity::Stump(e) => &e.base.id,
            ShaftEntity::Kickstand(e) => &e.base.id,
        }
    }

    /// The entity's type. DragonFruit reads `typeId` off the entity (or the
    /// store); a typed reference always knows it.
    pub fn type_id(&self) -> SupportTypeId {
        match self {
            ShaftEntity::Trunk(_) => SupportTypeId::Trunk,
            ShaftEntity::Branch(_) => SupportTypeId::Branch,
            ShaftEntity::Twig(_) => SupportTypeId::Twig,
            ShaftEntity::Stick(_) => SupportTypeId::Stick,
            ShaftEntity::Stump(_) => SupportTypeId::Stump,
            ShaftEntity::Kickstand(_) => SupportTypeId::Kickstand,
        }
    }

    pub fn segments(&self) -> &'a [Segment] {
        match self {
            ShaftEntity::Trunk(e) => &e.segments,
            ShaftEntity::Branch(e) => &e.segments,
            ShaftEntity::Twig(e) => &e.segments,
            ShaftEntity::Stick(e) => &e.segments,
            ShaftEntity::Stump(e) => &e.segments,
            ShaftEntity::Kickstand(e) => &e.segments,
        }
    }

    /// `entity[field]` for a declared contact field, or `None` when unset.
    pub fn contact(&self, field: &str) -> Option<ContactRef<'a>> {
        match (self, field) {
            (ShaftEntity::Trunk(e), "contactCone") => e.contact_cone.as_ref().map(ContactRef::Cone),
            (ShaftEntity::Branch(e), "contactCone") => {
                e.contact_cone.as_ref().map(ContactRef::Cone)
            }
            (ShaftEntity::Stump(e), "contactCone") => Some(ContactRef::Cone(&e.contact_cone)),
            (ShaftEntity::Stick(e), "contactConeA") => Some(ContactRef::Cone(&e.contact_cone_a)),
            (ShaftEntity::Stick(e), "contactConeB") => Some(ContactRef::Cone(&e.contact_cone_b)),
            (ShaftEntity::Twig(e), "contactDiskA") => Some(ContactRef::Disk(&e.contact_disk_a)),
            (ShaftEntity::Twig(e), "contactDiskB") => Some(ContactRef::Disk(&e.contact_disk_b)),
            _ => None,
        }
    }
}

/// Top of a root's disk-plus-cone stack, where an owned shaft leaves it.
pub(crate) fn root_top(root: &Roots) -> Vec3 {
    let disk_height = if root.disk_height.is_finite() {
        root.disk_height
    } else {
        0.0
    };
    let cone_height = if root.cone_height.is_finite() {
        root.cone_height
    } else {
        root.height.filter(|h| h.is_finite()).unwrap_or(0.0)
    };
    root.transform
        .pos
        .add(Vec3::new(0.0, 0.0, disk_height + cone_height))
}

/// Whether this type's lower end hangs from a knot rather than a root.
fn starts_at_knot(descriptor: &SupportTypeDescriptor) -> bool {
    descriptor.lower.kind == SupportEndpointKind::Knot
}

/// Where segment 0 begins. Later segments always continue from the previous
/// segment's top joint, so only the first needs the host.
pub fn anchor_point(descriptor: &SupportTypeDescriptor, hosts: EndpointHosts<'_>) -> Option<Vec3> {
    match descriptor.lower.kind {
        SupportEndpointKind::PlateRoot => hosts.root.map(root_top),
        SupportEndpointKind::Knot => hosts.host_knot.map(|knot| knot.pos),
        // A contact or inline root anchors the shaft on the entity itself,
        // so the segment joints already carry it.
        _ => None,
    }
}

/// The socket position of a declared contact endpoint.
///
/// A disk has no socket of its own: DragonFruit hands it to
/// `getFinalSocketPosition` regardless and throws reading its missing `normal`.
/// It is reported as no contact here, so the caller's fallback applies.
fn contact_at(endpoint: &SupportEndpoint, entity: &ShaftEntity<'_>) -> Option<Vec3> {
    match entity.contact(endpoint.field?)? {
        ContactRef::Cone(cone) => Some(get_final_socket_position(cone)),
        ContactRef::Disk(_) => None,
    }
}

/// Where a shaft is anchored, as a plain Vec3. The joint-drag angle clamp
/// measures from here.
///
/// Takes the type rather than the entity, because the shaft being dragged is not
/// always the thing the caller holds.
pub fn resolve_shaft_anchor(type_id: SupportTypeId, hosts: EndpointHosts<'_>) -> Option<Vec3> {
    anchor_point(get_support_type_descriptor(type_id), hosts)
}

/// Endpoints of `segment` on a shafted support, or `None` when the host it
/// needs is missing.
///
/// A type whose segments carry both joints resolves from the segment alone; the
/// others fall back through the previous joint, the declared anchor, then the
/// contact socket.
pub fn resolve_segment_endpoints(
    entity: ShaftEntity<'_>,
    segment: &Segment,
    segment_index: usize,
    hosts: EndpointHosts<'_>,
) -> Option<SegmentEndpoints> {
    let descriptor = get_support_type_descriptor(entity.type_id());
    if !descriptor.has_segments {
        return None;
    }

    let anchor = anchor_point(descriptor, hosts);
    // A type that declares a host but was handed none cannot be placed.
    if !descriptor.segments_carry_both_joints && anchor.is_none() {
        return None;
    }

    // A knot-hosted shaft starts at its host, not at its own bottom joint: the
    // joint is a render artefact there, and trusting it detaches the shaft from
    // the knot it hangs on. Root-owned and self-contained shafts read the joint
    // first, which is what their originals did.
    let starts_at_host = starts_at_knot(descriptor);

    let start = if !starts_at_host && let Some(bottom) = &segment.bottom_joint {
        bottom.pos
    } else if segment_index == 0 {
        anchor?
    } else {
        let previous = entity.segments().get(segment_index - 1);
        if let Some(top) = previous.and_then(|p| p.top_joint.as_ref()) {
            top.pos
        } else if let Some(anchor) = anchor {
            anchor
        } else if let Some(bottom) = &segment.bottom_joint {
            bottom.pos
        } else {
            return None;
        }
    };

    let end = if let Some(top) = &segment.top_joint {
        top.pos
    } else if descriptor.upper.kind == SupportEndpointKind::Knot {
        // A kickstand ends at the knot it braces, not at a contact.
        hosts.host_knot?.pos
    } else {
        contact_at(&descriptor.upper, &entity).unwrap_or_else(|| {
            start.add(Vec3::new(
                0.0,
                0.0,
                descriptor.shaft_fallback.stub_length_mm,
            ))
        })
    };

    Some(SegmentEndpoints { start, end })
}
