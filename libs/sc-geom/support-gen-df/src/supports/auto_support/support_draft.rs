//! DragonFruit `src/supports/autoSupport/supportDraft.ts`.
//!
//! Draft mutations for the auto-support PLAN phase: the whole placement is
//! computed against a local draft (no store commits), so the run is one atomic
//! commit. These mirror the entity-add arm of the store's `addSupportEntity`,
//! minus the settings-code-hex cache and the notifications. The plan phase only
//! ever ADDS entities.

use indexmap::IndexMap;

use crate::supports::support_type_registry::{
    PlacementPrimitive, SupportCollectionKey, SupportEdgeTarget, SupportEntityValue, SupportTypeId,
    get_support_type_descriptor,
};
use crate::supports::types::SupportState;

/// Add one support entity to a draft, stamped with its type. An entity already
/// under that id is replaced in place, as an object spread keeps its key.
pub fn draft_add_entity(
    draft: &mut SupportState,
    type_id: SupportTypeId,
    entity: SupportEntityValue,
) {
    let c = &mut draft.collections;
    macro_rules! put {
        ($collection:expr, $e:expr) => {{
            let mut e = $e;
            e.base.type_id = Some(type_id);
            $collection.insert(e.base.id.clone(), e);
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

/// Primitives are not support types and carry no `typeId`.
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

/// Commit one placed support: the entity plus every primitive its declared
/// `edges` point at, keyed by edge field.
pub fn draft_commit_support(
    draft: &mut SupportState,
    type_id: SupportTypeId,
    entity: SupportEntityValue,
    supplied: &IndexMap<String, PlacementPrimitive>,
) {
    draft_add_entity(draft, type_id, entity);
    for edge in get_support_type_descriptor(type_id).edges {
        // Only roots and knots are collections a placement can carry in.
        let SupportEdgeTarget::Collection(key) = edge.to else {
            continue;
        };
        if !matches!(
            key,
            SupportCollectionKey::Roots | SupportCollectionKey::Knots
        ) {
            continue;
        }
        let Some(primitive) = supplied.get(edge.field) else {
            continue;
        };
        draft_add_primitive(draft, primitive.clone());
    }
}
