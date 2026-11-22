//! DragonFruit `src/supports/SupportTypes/Trunk/TrunkReplacement/maxConnectedDiameter.ts`,
//! the parts the auto-support plan uses: the member diameter the forest report
//! prints, the stepwise host diameter profile, and the forest-wide resize.
//! (`computeMaxConnectedDiameterFromTrunk`, the connected-graph walk, is the
//! store's and is ported with it.)


use crate::supports::support_kinds::{SupportEntityValue, get_support_type_descriptor};
use crate::supports::types::Leaf;
use crate::geom::math_utils::js_max;

fn leaf_diameter(leaf: &Leaf) -> f64 {
    let p = &leaf.contact_cone.profile;
    js_max(p.body_diameter_mm, p.contact_diameter_mm)
}

fn widest_segment(segments: &[crate::supports::types::Segment]) -> f64 {
    segments
        .iter()
        .fold(0.0, |m, s| if s.diameter > m { s.diameter } else { m })
}

/// The diameter a support contributes to its connected graph: a shafted type
/// its widest segment, a brace its profile, a leaf its contact cone.
pub fn member_diameter_of(entity: &SupportEntityValue) -> f64 {
    let segments = match entity {
        SupportEntityValue::Trunk(e) => Some(&e.segments),
        SupportEntityValue::Branch(e) => Some(&e.segments),
        SupportEntityValue::Twig(e) => Some(&e.segments),
        SupportEntityValue::Stick(e) => Some(&e.segments),
        SupportEntityValue::Stump(e) => Some(&e.segments),
        SupportEntityValue::Kickstand(e) => Some(&e.segments),
        SupportEntityValue::Brace(_) | SupportEntityValue::Leaf(_) => None,
    };
    let type_id = match entity {
        SupportEntityValue::Trunk(_) => {
            crate::supports::support_kinds::SupportTypeId::Trunk
        }
        SupportEntityValue::Branch(_) => {
            crate::supports::support_kinds::SupportTypeId::Branch
        }
        SupportEntityValue::Leaf(_) => crate::supports::support_kinds::SupportTypeId::Leaf,
        SupportEntityValue::Twig(_) => crate::supports::support_kinds::SupportTypeId::Twig,
        SupportEntityValue::Stick(_) => {
            crate::supports::support_kinds::SupportTypeId::Stick
        }
        SupportEntityValue::Brace(_) => {
            crate::supports::support_kinds::SupportTypeId::Brace
        }
        SupportEntityValue::Stump(_) => {
            crate::supports::support_kinds::SupportTypeId::Stump
        }
        SupportEntityValue::Kickstand(_) => {
            crate::supports::support_kinds::SupportTypeId::Kickstand
        }
    };
    if get_support_type_descriptor(type_id).has_segments {
        return segments.map_or(0.0, |s| widest_segment(s));
    }
    match entity {
        SupportEntityValue::Brace(b) => b.profile.diameter,
        SupportEntityValue::Leaf(l) => leaf_diameter(l),
        _ => 0.0,
    }
}
