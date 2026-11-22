//! The draft a run places into: every support goes into a copy of the
//! state, committed as one when the run ends. A run only ever adds.

use indexmap::IndexMap;

use bread_support_model::support_kinds::{
    PlacementPrimitive, SupportCollectionKey, SupportEdgeTarget, SupportEntityValue, SupportTypeId,
    get_support_type_descriptor,
};
use bread_support_model::types::SupportState;

/// Puts `entity` into the draft as a `type_id`, replacing one of its id.
pub fn draft_add_entity(
    draft: &mut SupportState,
    type_id: SupportTypeId,
    entity: SupportEntityValue,
) {
    let c = &mut draft.collections;
    macro_rules! put {
        ($into:expr, $e:expr) => {{
            let mut e = $e;
            e.base.type_id = Some(type_id);
            $into.insert(e.base.id.clone(), e);
        }};
    }
    match entity {
        SupportEntityValue::Trunk(e) => put!(c.trunks, e),
        SupportEntityValue::Branch(e) => put!(c.branches, e),
        SupportEntityValue::Leaf(e) => put!(c.leaves, e),
        SupportEntityValue::Twig(e) => put!(c.twigs, e),
        SupportEntityValue::Stick(e) => put!(c.sticks, e),
        SupportEntityValue::Brace(e) => put!(c.braces, e),
        SupportEntityValue::Stump(e) => put!(c.stumps, e),
        SupportEntityValue::Kickstand(e) => put!(c.kickstands, e),
    }
}

/// Puts a root or a knot into the draft (these carry no type).
pub fn draft_add_primitive(draft: &mut SupportState, primitive: PlacementPrimitive) {
    match primitive {
        PlacementPrimitive::Roots(r) => {
            draft.collections.roots.insert(r.base.id.clone(), r);
        }
        PlacementPrimitive::Knot(k) => {
            draft.collections.knots.insert(k.id.clone(), k);
        }
    }
}

/// Puts a placed support into the draft with the roots and knots its kind
/// links to, taken from `supplied` by link field.
pub fn draft_commit_support(
    draft: &mut SupportState,
    type_id: SupportTypeId,
    entity: SupportEntityValue,
    supplied: &IndexMap<String, PlacementPrimitive>,
) {
    draft_add_entity(draft, type_id, entity);
    for edge in get_support_type_descriptor(type_id).edges {
        let SupportEdgeTarget::Collection(key) = edge.to else {
            continue;
        };
        if matches!(
            key,
            SupportCollectionKey::Roots | SupportCollectionKey::Knots
        ) && let Some(primitive) = supplied.get(edge.field)
        {
            draft_add_primitive(draft, primitive.clone());
        }
    }
}
