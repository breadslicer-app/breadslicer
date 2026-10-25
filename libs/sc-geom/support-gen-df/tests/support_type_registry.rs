//! DragonFruit `src/supports/__tests__/supportTypeRegistry.test.ts` and
//! `derivedTypeSubsets.test.ts`, plus the load-time drift checks the registry
//! module runs in TypeScript (`sidebarPanelOrderDrift`, `shaftHostedMemberOrderDrift`,
//! `placementModeOwnerDrift`, the "exactly one" asserts).

use std::collections::HashSet;

use support_gen_df::supports::support_type_registry::*;
use support_gen_df::supports::types::SupportCollections;

#[test]
fn every_support_type_is_declared_exactly_once() {
    let ids: Vec<SupportTypeId> = SUPPORT_TYPES.iter().map(|d| d.id).collect();
    assert_eq!(
        ids.iter().collect::<HashSet<_>>().len(),
        ids.len(),
        "a type is declared more than once"
    );
    // Two descriptors sharing one collection would silently overwrite each other.
    let keys: Vec<SupportCollectionKey> = SUPPORT_TYPES.iter().map(|d| d.location.key()).collect();
    assert_eq!(
        keys.iter().collect::<HashSet<_>>().len(),
        keys.len(),
        "two types share a collection"
    );
    // Registry order is the enum's declaration order, which `get_support_type_descriptor` indexes by.
    assert_eq!(ids, SupportTypeId::ALL);
}

#[test]
fn history_actions_match_what_the_builders_produce() {
    for descriptor in &SUPPORT_TYPES {
        assert_eq!(
            descriptor.history_add(),
            add_action(descriptor.id),
            "{} add",
            descriptor.id
        );
        assert_eq!(
            descriptor.history_remove(),
            remove_action(descriptor.id),
            "{} remove",
            descriptor.id
        );
        assert_eq!(
            descriptor.history_add(),
            format!("support:add-{}", descriptor.id)
        );
    }
}

#[test]
fn selection_categories_are_unique_and_resolve_back_to_their_type() {
    let categories: Vec<SupportSelectionCategory> =
        SUPPORT_TYPES.iter().map(|d| d.selection_category).collect();
    assert_eq!(
        categories.iter().collect::<HashSet<_>>().len(),
        categories.len()
    );
    for descriptor in &SUPPORT_TYPES {
        assert_eq!(
            get_support_type_by_selection_category(Some(descriptor.selection_category))
                .map(|d| d.id),
            Some(descriptor.id)
        );
    }
}

#[test]
fn primitive_categories_are_not_support_types() {
    for category in [
        Some(SupportSelectionCategory::Root),
        Some(SupportSelectionCategory::Joint),
        Some(SupportSelectionCategory::Knot),
        Some(SupportSelectionCategory::Segment),
        Some(SupportSelectionCategory::ContactDisk),
        None,
    ] {
        assert!(get_support_type_by_selection_category(category).is_none());
    }
}

#[test]
fn every_type_lives_on_support_state() {
    assert_eq!(support_state_types().len(), SUPPORT_TYPES.len());
}

#[test]
fn empty_collections_cover_every_entity_collection_on_support_state() {
    let json = serde_json::to_value(create_empty_support_collections()).unwrap();
    let mut keys: Vec<&str> = json
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "braces",
            "branches",
            "kickstands",
            "knots",
            "leaves",
            "roots",
            "sticks",
            "stumps",
            "trunks",
            "twigs"
        ]
    );
}

#[test]
fn selection_resolves_roots_first_then_support_types_in_registry_order() {
    let categories: Vec<SupportSelectionCategory> = support_state_collections()
        .into_iter()
        .map(|(_, c)| c)
        .collect();
    assert_eq!(
        categories[0],
        SupportSelectionCategory::Root,
        "roots resolve before any type"
    );
    let expected: Vec<SupportSelectionCategory> = support_state_types()
        .into_iter()
        .map(|d| SupportSelectionCategory::of_type(d.id))
        .collect();
    assert_eq!(
        categories[1..],
        expected[..],
        "each declared type selects under its own id, in registry order"
    );
}

#[test]
fn every_collection_is_either_a_support_type_or_a_declared_primitive() {
    let covered: HashSet<&str> = SUPPORT_TYPES
        .iter()
        .map(|d| d.location.key().as_str())
        .chain(SUPPORT_PRIMITIVE_COLLECTIONS.iter().map(|c| c.key.as_str()))
        .collect();
    let json = serde_json::to_value(SupportCollections::default()).unwrap();
    let uncovered: Vec<&String> = json
        .as_object()
        .unwrap()
        .keys()
        .filter(|k| !covered.contains(k.as_str()))
        .collect();
    assert!(
        uncovered.is_empty(),
        "add a descriptor or a primitive entry for these: {uncovered:?}"
    );
}

#[test]
fn primitives_are_collections_that_are_not_support_types() {
    let mut keys: Vec<&str> = SUPPORT_PRIMITIVE_COLLECTIONS
        .iter()
        .map(|c| c.key.as_str())
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["knots", "roots"]);
}

#[test]
fn the_model_id_walk_covers_everything_except_knots() {
    let walked: HashSet<SupportCollectionKey> = model_id_collection_keys().into_iter().collect();
    let missing: Vec<SupportCollectionKey> = support_collection_keys()
        .into_iter()
        .filter(|k| !walked.contains(k))
        .collect();
    assert_eq!(missing, [SupportCollectionKey::Knots]);
}

#[test]
fn a_types_display_label_names_the_same_type_its_id_does() {
    for descriptor in &SUPPORT_TYPES {
        let stem = &descriptor.id.as_str()[..3];
        assert!(
            descriptor.label.to_lowercase().starts_with(stem),
            "{}: label \"{}\" does not look like a name for this type",
            descriptor.id,
            descriptor.label
        );
    }
}

// --- derivedTypeSubsets.test.ts ---

fn ids_flagged(read: impl Fn(&SupportTypeDescriptor) -> bool) -> Vec<SupportTypeId> {
    let mut ids: Vec<SupportTypeId> = SUPPORT_TYPES
        .iter()
        .filter(|d| read(d))
        .map(|d| d.id)
        .collect();
    ids.sort();
    ids
}

fn sorted(ids: &[SupportTypeId]) -> Vec<SupportTypeId> {
    let mut ids = ids.to_vec();
    ids.sort();
    ids
}

#[test]
fn the_kickstand_host_set_is_exactly_the_types_declaring_hosts_kickstand() {
    assert_eq!(
        sorted(&KICKSTAND_HOST_TYPES),
        ids_flagged(|d| d.hosts_kickstand)
    );
}

#[test]
fn a_kickstand_hosts_on_every_type_carrying_a_shaft_and_only_those() {
    // A host knot rides a shaft: real segments, or a brace's selectable span.
    // A leaf declares a knot host too, but its pseudo-shaft is a contact cone.
    assert_eq!(
        sorted(&KICKSTAND_HOST_TYPES),
        ids_flagged(|d| d.has_segments || is_span_knot_host(d.id))
    );
}

#[test]
fn the_auto_placed_set_is_exactly_the_types_declaring_is_auto_placed() {
    assert_eq!(
        sorted(&auto_placed_type_ids()),
        ids_flagged(|d| d.is_auto_placed)
    );
    assert!(
        !is_auto_placed_type(Some(SupportTypeId::Brace)),
        "brace is placed by hand"
    );
    assert!(!is_auto_placed_type(None), "'reject' is not a placed kind");
}

#[test]
fn the_joint_removal_set_is_exactly_the_shafted_types_resolving_an_endpoint_elsewhere() {
    assert_eq!(
        sorted(&JOINT_REMOVAL_TYPES),
        ids_flagged(|d| d.has_segments && !d.segments_carry_both_joints)
    );
}

#[test]
fn hand_written_subsets_match_their_flags() {
    assert_eq!(
        sorted(&JOINT_DRAG_PREVIEW_TYPES),
        sorted(&[
            SupportTypeId::Trunk,
            SupportTypeId::Branch,
            SupportTypeId::Brace,
            SupportTypeId::Kickstand
        ])
    );
    assert_eq!(
        sorted(&MODEL_SURFACE_GESTURE_TYPES),
        ids_flagged(|d| d.claims_model_surface_gestures)
    );
}

// --- load-time drift checks of supportTypeRegistry.ts ---

#[test]
fn load_time_drift_checks_pass() {
    assert!(
        sidebar_panel_order_drift().is_empty(),
        "{:?}",
        sidebar_panel_order_drift()
    );
    assert!(
        shaft_hosted_member_order_drift().is_empty(),
        "{:?}",
        shaft_hosted_member_order_drift()
    );
    assert!(
        placement_mode_owner_drift().is_empty(),
        "{:?}",
        placement_mode_owner_drift()
    );
    assert!(types_declaring_own_history_entry_without_update().is_empty());
    assert!(
        types_missing_contact_override().is_empty(),
        "{:?}",
        types_missing_contact_override()
    );
}

#[test]
fn exactly_one_asserts_hold() {
    assert_eq!(span_knot_host_type(), SupportTypeId::Brace);
    assert_eq!(cone_knot_host_type(), SupportTypeId::Leaf);
    assert_eq!(default_placement_tool_type_id(), SupportTypeId::Trunk);
    assert_eq!(bundled_support_type_id(), SupportTypeId::Kickstand);
    assert_eq!(near_plate_origin(), SupportOriginId::Stump);
    assert_eq!(
        inline_root_types(),
        [InlineRootPlacement {
            type_id: SupportTypeId::Stump,
            collection_key: SupportCollectionKey::Stumps,
            pos_field: "rootPos",
            radius_field: "rootBaseDiameter",
        }]
    );
}

#[test]
fn shaft_hosted_members_walk_leaves_before_branches() {
    let members = shaft_hosted_member_types();
    assert_eq!(
        members.iter().map(|m| m.type_id).collect::<Vec<_>>(),
        [SupportTypeId::Leaf, SupportTypeId::Branch]
    );
    assert!(members.iter().all(|m| m.knot_field == "parentKnotId"));
}

#[test]
fn derived_lists_follow_registry_order() {
    assert_eq!(
        auto_braceable_shaft_types(),
        [SupportTypeId::Trunk, SupportTypeId::Branch]
    );
    assert_eq!(lateral_stabiliser_types(), [SupportTypeId::Kickstand]);
    assert_eq!(
        contact_bridge_types(),
        [SupportTypeId::Twig, SupportTypeId::Stick]
    );
    assert_eq!(grid_host_collection_keys(), [SupportCollectionKey::Trunks]);
    assert_eq!(brace_snap_cone_type(), Some(SupportTypeId::Leaf));
    assert_eq!(types_with_auto_placement(), [SupportTypeId::Stump]);
    assert_eq!(
        leaf_sprout_host_types(),
        [
            SupportTypeId::Trunk,
            SupportTypeId::Branch,
            SupportTypeId::Twig,
            SupportTypeId::Stick,
            SupportTypeId::Stump,
            SupportTypeId::Kickstand,
        ]
    );
    assert_eq!(
        flexing_knot_host_types(),
        [(SupportTypeId::Branch, vec!["parentKnotId"])]
    );
}

#[test]
fn placement_rules_pick_one_type_per_band() {
    let no_setting = |_: SupportPlacementSettingPath| None;
    // tipHeight: stump owns [0, 5), trunk owns [5, inf) -- both rules say `upper`.
    assert_eq!(
        select_type_for_placement(SupportPlacementMetric::TipHeight, 4.999, &no_setting),
        Some(SupportTypeId::Stump)
    );
    assert_eq!(
        select_type_for_placement(SupportPlacementMetric::TipHeight, 5.0, &no_setting),
        Some(SupportTypeId::Trunk)
    );
    // contactSpan: twig owns (.., 5], stick owns (5, ..) -- default `lower` boundary.
    assert_eq!(
        select_type_for_placement(SupportPlacementMetric::ContactSpan, 5.0, &no_setting),
        Some(SupportTypeId::Twig)
    );
    assert_eq!(
        select_type_for_placement(SupportPlacementMetric::ContactSpan, 5.01, &no_setting),
        Some(SupportTypeId::Stick)
    );
    let cutoff_3 = |_: SupportPlacementSettingPath| Some(3.0);
    assert_eq!(
        select_type_for_placement(SupportPlacementMetric::ContactSpan, 4.0, &cutoff_3),
        Some(SupportTypeId::Stick)
    );
    assert_eq!(
        select_type_for_placement(SupportPlacementMetric::TipHeight, f64::NAN, &no_setting),
        None
    );
}

#[test]
fn preview_priority_ranks_the_active_mode_first() {
    let none_active = |_: SupportTypeId| false;
    assert_eq!(
        preview_types_by_priority(SupportPreviewPurpose::ContactGuideWidth, &none_active),
        [
            SupportTypeId::Trunk,
            SupportTypeId::Branch,
            SupportTypeId::Leaf,
            SupportTypeId::Kickstand
        ]
    );
    let branch_active = |t: SupportTypeId| t == SupportTypeId::Branch;
    assert_eq!(
        preview_types_by_priority(SupportPreviewPurpose::ContactGuideWidth, &branch_active),
        [
            SupportTypeId::Branch,
            SupportTypeId::Trunk,
            SupportTypeId::Branch,
            SupportTypeId::Leaf,
            SupportTypeId::Kickstand
        ]
    );
    assert_eq!(
        preview_types_by_priority(SupportPreviewPurpose::LimitationFeedback, &none_active),
        [SupportTypeId::Leaf, SupportTypeId::Trunk]
    );
}

#[test]
fn prefixed_ids_round_trip() {
    assert_eq!(
        parse_knot_host_id("leafCone:abc"),
        Some((SupportTypeId::Leaf, "abc"))
    );
    assert_eq!(
        parse_knot_host_id("braceSegment:b1"),
        Some((SupportTypeId::Brace, "b1"))
    );
    assert_eq!(parse_knot_host_id("seg-1"), None);
    assert_eq!(knot_host_id(SupportTypeId::Leaf, "x"), "leafCone:x");
    assert_eq!(
        segment_selection_id(SupportTypeId::Brace, "b1"),
        "braceSegment:b1"
    );
    assert_eq!(parse_inline_root_id(&inline_root_id("s1")), Some("s1"));
    assert_eq!(parse_inline_root_id("s1"), None);
    assert_eq!(
        export_group_name(SupportTypeId::Kickstand, "k1"),
        "Kickstand_k1"
    );
}

#[test]
fn origins_gate_tree_conversion() {
    assert!(!is_origin_convertible_to_tree(None));
    assert!(!is_origin_convertible_to_tree(Some(SupportOriginId::Stump)));
    assert!(!is_origin_convertible_to_tree(Some(
        SupportOriginId::Island
    )));
    assert!(is_origin_convertible_to_tree(Some(
        SupportOriginId::Overhang
    )));
    assert!(is_origin_convertible_to_tree(Some(
        SupportOriginId::Reinforcement
    )));
}
