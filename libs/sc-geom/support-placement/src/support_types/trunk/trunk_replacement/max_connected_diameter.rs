//! How wide each support is, for sizing the trunk that carries it.

use support_geom::geom::math_utils::js_max;
use support_model::support_kinds::{
    SupportEntityValue, SupportTypeId, get_support_type_descriptor,
};
use support_model::types::Segment;

/// The widest segment, or 0 for none.
fn widest(segments: &[Segment]) -> f64 {
    segments
        .iter()
        .fold(0.0, |w, s| if s.diameter > w { s.diameter } else { w })
}

/// How wide a support counts as: a type with a shaft by its widest segment, a
/// brace by its profile, a leaf by the wider part of its cone.
pub fn member_diameter_of(entity: &SupportEntityValue) -> f64 {
    let (type_id, segments) = match entity {
        SupportEntityValue::Trunk(e) => (SupportTypeId::Trunk, Some(&e.segments)),
        SupportEntityValue::Branch(e) => (SupportTypeId::Branch, Some(&e.segments)),
        SupportEntityValue::Twig(e) => (SupportTypeId::Twig, Some(&e.segments)),
        SupportEntityValue::Stick(e) => (SupportTypeId::Stick, Some(&e.segments)),
        SupportEntityValue::Stump(e) => (SupportTypeId::Stump, Some(&e.segments)),
        SupportEntityValue::Kickstand(e) => (SupportTypeId::Kickstand, Some(&e.segments)),
        SupportEntityValue::Brace(_) => (SupportTypeId::Brace, None),
        SupportEntityValue::Leaf(_) => (SupportTypeId::Leaf, None),
    };
    if get_support_type_descriptor(type_id).has_segments {
        return segments.map_or(0.0, |s| widest(s));
    }
    match entity {
        SupportEntityValue::Brace(brace) => brace.profile.diameter,
        SupportEntityValue::Leaf(leaf) => {
            let cone = &leaf.contact_cone.profile;
            js_max(cone.body_diameter_mm, cone.contact_diameter_mm)
        }
        _ => 0.0,
    }
}
