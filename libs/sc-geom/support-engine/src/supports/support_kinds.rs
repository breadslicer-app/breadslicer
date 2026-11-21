//! The kinds of support: what each one stands on and ends in, how it links to
//! others, and the facts the stages read about it.

use std::fmt;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::auto_support::constants::ANCHOR_HEIGHT_THRESHOLD_MM;
use super::types::{Brace, Branch, Kickstand, Knot, Leaf, Roots, Stick, Stump, Trunk, Twig};

/// A kind of support. Its order is the order every walk over kinds takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SupportTypeId {
    /// From a root on the plate up to the model.
    Trunk,
    /// From a knot on another shaft up to the model.
    Branch,
    /// A tip straight off a knot on another shaft.
    Leaf,
    /// A short shaft from the model to the model, ending in disks.
    Twig,
    /// A longer shaft from the model to the model, ending in cones.
    Stick,
    /// A shaft between two knots.
    Brace,
    /// A short support near the plate, with its root on it.
    Stump,
    /// A slanted shaft from a root on the plate to a knot on another shaft.
    Kickstand,
}

impl SupportTypeId {
    pub const ALL: [SupportTypeId; 8] = [
        SupportTypeId::Trunk,
        SupportTypeId::Branch,
        SupportTypeId::Leaf,
        SupportTypeId::Twig,
        SupportTypeId::Stick,
        SupportTypeId::Brace,
        SupportTypeId::Stump,
        SupportTypeId::Kickstand,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            SupportTypeId::Trunk => "trunk",
            SupportTypeId::Branch => "branch",
            SupportTypeId::Leaf => "leaf",
            SupportTypeId::Twig => "twig",
            SupportTypeId::Stick => "stick",
            SupportTypeId::Brace => "brace",
            SupportTypeId::Stump => "stump",
            SupportTypeId::Kickstand => "kickstand",
        }
    }

    pub fn parse(name: &str) -> Option<SupportTypeId> {
        SupportTypeId::ALL.into_iter().find(|t| t.as_str() == name)
    }
}

impl fmt::Display for SupportTypeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What can be selected in a support set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SupportSelectionCategory {
    Trunk,
    Branch,
    Leaf,
    Twig,
    Stick,
    Brace,
    Stump,
    Kickstand,
    Root,
    Joint,
    Knot,
    Segment,
    ContactDisk,
}

/// The collections of a support set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SupportCollectionKey {
    Roots,
    Trunks,
    Branches,
    Leaves,
    Twigs,
    Sticks,
    Braces,
    Stumps,
    Kickstands,
    Knots,
}

/// What a support stands on or ends in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SupportEndpointKind {
    /// A root of the set's roots, on the plate.
    PlateRoot,
    /// A root the support carries itself.
    InlineRoot,
    /// A knot on another support's shaft.
    Knot,
    /// A contact cone on the model.
    Cone,
    /// A contact disk on the model.
    Disk,
    None,
}

/// One end of a support, and the field that holds what is there (for a cone
/// or a disk).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupportEndpoint {
    pub kind: SupportEndpointKind,
    pub field: Option<&'static str>,
}

const fn on(kind: SupportEndpointKind) -> SupportEndpoint {
    SupportEndpoint { kind, field: None }
}

const fn contact(kind: SupportEndpointKind, field: &'static str) -> SupportEndpoint {
    SupportEndpoint {
        kind,
        field: Some(field),
    }
}

/// What picks a kind for a placement: the span between two contacts, or the
/// tip's height over the plate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SupportPlacementMetric {
    ContactSpan,
    TipHeight,
}

/// A setting a placement bound may come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SupportPlacementSettingPath {
    /// The span over which a bridge from the model to the model is a stick
    /// rather than a twig.
    MeshToMeshStickVsTwigCutoffMm,
}

/// A placement bound: a length, or a setting with the length it falls back to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SupportPlacementThreshold {
    Mm(f64),
    Setting {
        setting: SupportPlacementSettingPath,
        fallback: f64,
    },
}

impl SupportPlacementThreshold {
    fn mm(self, read: &dyn Fn(SupportPlacementSettingPath) -> Option<f64>) -> f64 {
        match self {
            SupportPlacementThreshold::Mm(mm) => mm,
            SupportPlacementThreshold::Setting { setting, fallback } => {
                read(setting).unwrap_or(fallback)
            }
        }
    }
}

/// The measurements a kind serves when kinds are picked by a measurement:
/// from `min_mm` to `max_mm`. With `includes_max` a value right on the max is
/// in and one right on the min is out; without, the reverse.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SupportPlacementRule {
    pub metric: SupportPlacementMetric,
    pub min_mm: Option<SupportPlacementThreshold>,
    pub max_mm: Option<SupportPlacementThreshold>,
    pub includes_max: bool,
}

/// How densely the automatic pass lays a kind down, and how thick.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SupportAutoPlacement {
    pub min_spacing_mm: Option<f64>,
    pub min_xy_mm: Option<f64>,
    pub min_area_mm2: Option<f64>,
    pub shaft_multiplier: Option<f64>,
}

/// Where a link points: a collection, or a segment of another support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SupportEdgeTarget {
    Collection(SupportCollectionKey),
    Segment,
}

/// A link a kind carries: the field holding the id, and what it points to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupportEdge {
    pub field: &'static str,
    pub to: SupportEdgeTarget,
}

const ROOT: SupportEdge = SupportEdge {
    field: "rootId",
    to: SupportEdgeTarget::Collection(SupportCollectionKey::Roots),
};

const fn knot(field: &'static str) -> SupportEdge {
    SupportEdge {
        field,
        to: SupportEdgeTarget::Collection(SupportCollectionKey::Knots),
    }
}

/// Which shaft a kind's contact cone takes its body width from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConeBodyFollows {
    /// The first segment (the shaft hangs down from the cone).
    OwnFirstSegment,
    /// The last segment (the shaft climbs into the cone).
    OwnLastSegment,
    /// The segment of the shaft the support hangs from.
    HostKnotSegment,
}

/// A shaft's width when it has no segments: a length, or a field of the
/// support.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FallbackDiameter {
    Mm(f64),
    Path(&'static str),
}

/// What a shaft whose ends cannot be found falls back to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShaftFallback {
    /// How long a shaft with nothing at its top end is drawn.
    pub stub_length_mm: f64,
    /// Whether a start that cannot be found is the point the shaft splits at.
    pub start_falls_back_to_split_point: bool,
    pub fallback_diameter_mm: Option<FallbackDiameter>,
}

impl ShaftFallback {
    const fn short(start_falls_back_to_split_point: bool) -> Self {
        ShaftFallback {
            stub_length_mm: 5.0,
            start_falls_back_to_split_point,
            fallback_diameter_mm: None,
        }
    }
}

/// The facts about a kind of support.
#[derive(Debug, Clone, PartialEq)]
pub struct SupportTypeDescriptor {
    pub id: SupportTypeId,
    pub singular: &'static str,
    /// A bridge of this kind may land sideways of its contact.
    pub may_reach_sideways: bool,
    /// Others may hang off its shaft when fanning out.
    pub can_be_grid_host: bool,
    /// The automatic pass places it and reports it.
    pub is_auto_placed: bool,
    /// It has a shaft of segments.
    pub has_segments: bool,
    /// A selected segment of it is named with this prefix and its id.
    pub segment_selection_prefix: Option<&'static str>,
    /// It records which rule placed it.
    pub has_origin: bool,
    /// Its shaft may be braced to others.
    pub is_auto_braceable: bool,
    pub placement_rule: Option<SupportPlacementRule>,
    pub auto_placement: Option<SupportAutoPlacement>,
    pub cone_body_follows: Option<ConeBodyFollows>,
    pub lower: SupportEndpoint,
    pub upper: SupportEndpoint,
    /// Each of its segments carries its own joints at both ends.
    pub segments_carry_both_joints: bool,
    /// It has a root of its own in the set's roots.
    pub owns_root: bool,
    pub edges: &'static [SupportEdge],
    pub shaft_fallback: ShaftFallback,
}

/// The span over which a bridge is a stick rather than a twig.
const STICK_SPAN: SupportPlacementThreshold = SupportPlacementThreshold::Setting {
    setting: SupportPlacementSettingPath::MeshToMeshStickVsTwigCutoffMm,
    fallback: 5.0,
};

/// A kind that is none of the special cases: drawn without a fallback
/// width, with a short stub, placed automatically.
const PLAIN: SupportTypeDescriptor = SupportTypeDescriptor {
    id: SupportTypeId::Trunk,
    singular: "",
    may_reach_sideways: false,
    can_be_grid_host: false,
    is_auto_placed: true,
    has_segments: true,
    segment_selection_prefix: None,
    has_origin: false,
    is_auto_braceable: false,
    placement_rule: None,
    auto_placement: None,
    cone_body_follows: None,
    lower: on(SupportEndpointKind::None),
    upper: on(SupportEndpointKind::None),
    segments_carry_both_joints: true,
    owns_root: false,
    edges: &[],
    shaft_fallback: ShaftFallback::short(false),
};

const TRUNK: SupportTypeDescriptor = SupportTypeDescriptor {
    id: SupportTypeId::Trunk,
    singular: "trunk",
    can_be_grid_host: true,
    has_origin: true,
    is_auto_braceable: true,
    placement_rule: Some(SupportPlacementRule {
        metric: SupportPlacementMetric::TipHeight,
        min_mm: Some(SupportPlacementThreshold::Mm(ANCHOR_HEIGHT_THRESHOLD_MM)),
        max_mm: None,
        includes_max: false,
    }),
    cone_body_follows: Some(ConeBodyFollows::OwnLastSegment),
    lower: on(SupportEndpointKind::PlateRoot),
    upper: contact(SupportEndpointKind::Cone, "contactCone"),
    segments_carry_both_joints: false,
    owns_root: true,
    edges: &[ROOT],
    shaft_fallback: ShaftFallback {
        stub_length_mm: 10.0,
        start_falls_back_to_split_point: false,
        fallback_diameter_mm: Some(FallbackDiameter::Mm(1.5)),
    },
    ..PLAIN
};

const BRANCH: SupportTypeDescriptor = SupportTypeDescriptor {
    id: SupportTypeId::Branch,
    singular: "branch",
    has_origin: true,
    is_auto_braceable: true,
    cone_body_follows: Some(ConeBodyFollows::OwnFirstSegment),
    lower: on(SupportEndpointKind::Knot),
    upper: contact(SupportEndpointKind::Cone, "contactCone"),
    segments_carry_both_joints: false,
    edges: &[knot("parentKnotId")],
    ..PLAIN
};

const LEAF: SupportTypeDescriptor = SupportTypeDescriptor {
    id: SupportTypeId::Leaf,
    singular: "leaf",
    has_segments: false,
    has_origin: true,
    cone_body_follows: Some(ConeBodyFollows::HostKnotSegment),
    lower: on(SupportEndpointKind::Knot),
    upper: contact(SupportEndpointKind::Cone, "contactCone"),
    edges: &[knot("parentKnotId")],
    ..PLAIN
};

const TWIG: SupportTypeDescriptor = SupportTypeDescriptor {
    id: SupportTypeId::Twig,
    singular: "twig",
    may_reach_sideways: true,
    placement_rule: Some(SupportPlacementRule {
        metric: SupportPlacementMetric::ContactSpan,
        min_mm: None,
        max_mm: Some(STICK_SPAN),
        includes_max: true,
    }),
    lower: contact(SupportEndpointKind::Disk, "contactDiskA"),
    upper: contact(SupportEndpointKind::Disk, "contactDiskB"),
    shaft_fallback: ShaftFallback::short(true),
    ..PLAIN
};

const STICK: SupportTypeDescriptor = SupportTypeDescriptor {
    id: SupportTypeId::Stick,
    singular: "stick",
    // A tall stick inside a cavity is braced as a trunk outside is (#632).
    is_auto_braceable: true,
    placement_rule: Some(SupportPlacementRule {
        metric: SupportPlacementMetric::ContactSpan,
        min_mm: Some(STICK_SPAN),
        max_mm: None,
        includes_max: true,
    }),
    lower: contact(SupportEndpointKind::Cone, "contactConeA"),
    upper: contact(SupportEndpointKind::Cone, "contactConeB"),
    shaft_fallback: ShaftFallback::short(true),
    ..PLAIN
};

const BRACE: SupportTypeDescriptor = SupportTypeDescriptor {
    id: SupportTypeId::Brace,
    singular: "brace",
    is_auto_placed: false,
    has_segments: false,
    segment_selection_prefix: Some("braceSegment:"),
    lower: on(SupportEndpointKind::Knot),
    upper: on(SupportEndpointKind::Knot),
    edges: &[knot("startKnotId"), knot("endKnotId")],
    ..PLAIN
};

const STUMP: SupportTypeDescriptor = SupportTypeDescriptor {
    id: SupportTypeId::Stump,
    singular: "stump",
    has_origin: true,
    placement_rule: Some(SupportPlacementRule {
        metric: SupportPlacementMetric::TipHeight,
        min_mm: None,
        max_mm: Some(SupportPlacementThreshold::Mm(ANCHOR_HEIGHT_THRESHOLD_MM)),
        includes_max: false,
    }),
    auto_placement: Some(SupportAutoPlacement {
        min_spacing_mm: Some(1.8),
        min_xy_mm: Some(4.0),
        min_area_mm2: Some(12.0),
        shaft_multiplier: Some(1.25),
    }),
    lower: contact(SupportEndpointKind::InlineRoot, "rootPos"),
    upper: contact(SupportEndpointKind::Cone, "contactCone"),
    shaft_fallback: ShaftFallback::short(true),
    ..PLAIN
};

const KICKSTAND: SupportTypeDescriptor = SupportTypeDescriptor {
    id: SupportTypeId::Kickstand,
    singular: "kickstand",
    is_auto_placed: false,
    is_auto_braceable: true,
    lower: on(SupportEndpointKind::PlateRoot),
    upper: on(SupportEndpointKind::Knot),
    segments_carry_both_joints: false,
    owns_root: true,
    edges: &[
        ROOT,
        knot("hostKnotId"),
        SupportEdge {
            field: "hostSegmentId",
            to: SupportEdgeTarget::Segment,
        },
    ],
    shaft_fallback: ShaftFallback {
        stub_length_mm: 5.0,
        start_falls_back_to_split_point: false,
        fallback_diameter_mm: Some(FallbackDiameter::Path("profile.bodyDiameterMm")),
    },
    ..PLAIN
};

/// Every kind's facts, in kind order.
static KINDS: [SupportTypeDescriptor; 8] =
    [TRUNK, BRANCH, LEAF, TWIG, STICK, BRACE, STUMP, KICKSTAND];

pub fn get_support_type_descriptor(id: SupportTypeId) -> &'static SupportTypeDescriptor {
    &KINDS[id as usize]
}

/// The kinds placed beside a group of shafts to hold it sideways.
pub fn lateral_stabiliser_types() -> Vec<SupportTypeId> {
    vec![SupportTypeId::Kickstand]
}

pub fn is_lateral_stabiliser_type(id: SupportTypeId) -> bool {
    id == SupportTypeId::Kickstand
}

/// The kinds that bridge the model to the model, in kind order.
pub fn contact_bridge_types() -> Vec<SupportTypeId> {
    vec![SupportTypeId::Twig, SupportTypeId::Stick]
}

/// The kinds whose contact the automatic pass may move.
pub const CONTACT_OVERRIDE_REGISTRATIONS: &[SupportTypeId] = &[SupportTypeId::Stump];

/// Which end of a support a contact is at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EndpointEnd {
    Lower,
    Upper,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContactKind {
    Cone,
    Disk,
}

/// One contact a kind carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContactEndpoint {
    pub end: EndpointEnd,
    pub kind: ContactKind,
    pub field: &'static str,
}

/// The contacts a kind carries, lower end first.
pub fn contact_endpoints_for(id: SupportTypeId) -> Vec<ContactEndpoint> {
    let kind = get_support_type_descriptor(id);
    [
        (EndpointEnd::Lower, kind.lower),
        (EndpointEnd::Upper, kind.upper),
    ]
    .into_iter()
    .filter_map(|(end, at)| {
        let contact = match at.kind {
            SupportEndpointKind::Cone => ContactKind::Cone,
            SupportEndpointKind::Disk => ContactKind::Disk,
            _ => return None,
        };
        Some(ContactEndpoint {
            end,
            kind: contact,
            field: at.field?,
        })
    })
    .collect()
}

/// Whether a bridge of `id` may land `dist_mm` from its contact: any kind
/// within `cutoff_mm` or when it did not reach sideways, only a kind that
/// may reach sideways beyond.
pub fn bridge_may_land_sideways(
    id: SupportTypeId,
    dist_mm: f64,
    cutoff_mm: f64,
    reached_sideways: bool,
) -> bool {
    !reached_sideways || dist_mm <= cutoff_mm || get_support_type_descriptor(id).may_reach_sideways
}

/// The first kind, in kind order, whose placement rule on `metric` takes
/// `value_mm` (bounds read through `read_setting`); none for a value that is
/// not finite.
pub fn select_type_for_placement(
    metric: SupportPlacementMetric,
    value_mm: f64,
    read_setting: &dyn Fn(SupportPlacementSettingPath) -> Option<f64>,
) -> Option<SupportTypeId> {
    if !value_mm.is_finite() {
        return None;
    }
    let takes = |rule: &SupportPlacementRule| {
        let above_min = rule.min_mm.is_none_or(|b| {
            let min = b.mm(read_setting);
            if rule.includes_max {
                value_mm > min
            } else {
                value_mm >= min
            }
        });
        let below_max = rule.max_mm.is_none_or(|b| {
            let max = b.mm(read_setting);
            if rule.includes_max {
                value_mm <= max
            } else {
                value_mm < max
            }
        });
        rule.metric == metric && above_min && below_max
    };
    KINDS
        .iter()
        .find(|k| k.placement_rule.as_ref().is_some_and(takes))
        .map(|k| k.id)
}

/// Whether shafts of `id` may be braced to each other: braceable kinds with a
/// shaft, other than those that hold a group sideways.
pub fn is_auto_braceable_shaft_type(id: SupportTypeId) -> bool {
    let kind = get_support_type_descriptor(id);
    kind.is_auto_braceable && kind.has_segments && !is_lateral_stabiliser_type(id)
}

/// The kinds others may fan out from, in kind order.
pub fn grid_host_types() -> Vec<&'static SupportTypeDescriptor> {
    KINDS.iter().filter(|k| k.can_be_grid_host).collect()
}

/// Whether a placed kind is one the automatic pass reports (`None`: rejected).
pub fn is_auto_placed_type(kind: Option<SupportTypeId>) -> bool {
    kind.is_some_and(|k| get_support_type_descriptor(k).is_auto_placed)
}

/// The kinds that hang off another shaft by one knot, in the order they are
/// walked: leaves before branches.
pub const SHAFT_HOSTED_MEMBER_TYPE_ORDER: [SupportTypeId; 2] =
    [SupportTypeId::Leaf, SupportTypeId::Branch];

/// The kinds a kickstand may stand against: every kind with a shaft, and a
/// brace.
pub const KICKSTAND_HOST_TYPES: [SupportTypeId; 7] = [
    SupportTypeId::Trunk,
    SupportTypeId::Branch,
    SupportTypeId::Twig,
    SupportTypeId::Stick,
    SupportTypeId::Brace,
    SupportTypeId::Stump,
    SupportTypeId::Kickstand,
];

/// Whether `name` names a kind a kickstand may stand against.
pub fn is_kickstand_host_type(name: &str) -> bool {
    SupportTypeId::parse(name).is_some_and(|t| KICKSTAND_HOST_TYPES.contains(&t))
}

/// The id of the root a support carries itself.
pub fn inline_root_id(entity_id: &str) -> String {
    format!("{entity_id}:root")
}

/// The kind and the support a prefixed segment id names, or none for a
/// segment of its own.
pub fn parse_prefixed_segment_id(segment_id: &str) -> Option<(SupportTypeId, &str)> {
    KINDS.iter().find_map(|k| {
        let rest = segment_id.strip_prefix(k.segment_selection_prefix?)?;
        Some((k.id, rest))
    })
}

/// Which rule placed a support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SupportOriginId {
    /// Near the plate.
    Stump,
    Overhang,
    Island,
    Standalone,
    /// The ring of contacts around a lowest point.
    Reinforcement,
    /// The detector that found a cavity's candidate.
    Voxel,
    Minima,
    Intersection,
    Stabilization,
}

impl SupportOriginId {
    pub const fn as_str(self) -> &'static str {
        match self {
            SupportOriginId::Stump => "stump",
            SupportOriginId::Overhang => "overhang",
            SupportOriginId::Island => "island",
            SupportOriginId::Standalone => "standalone",
            SupportOriginId::Reinforcement => "reinforcement",
            SupportOriginId::Voxel => "voxel",
            SupportOriginId::Minima => "minima",
            SupportOriginId::Intersection => "intersection",
            SupportOriginId::Stabilization => "stabilization",
        }
    }

    /// Whether a trunk placed by this rule may become a tree: not near the
    /// plate, not an island's, not a cavity's.
    pub const fn convertible_to_tree(self) -> bool {
        matches!(
            self,
            SupportOriginId::Overhang
                | SupportOriginId::Standalone
                | SupportOriginId::Reinforcement
        )
    }
}

/// The rule of supports placed near the plate.
pub fn near_plate_origin() -> SupportOriginId {
    SupportOriginId::Stump
}

pub fn is_origin_convertible_to_tree(origin: Option<SupportOriginId>) -> bool {
    origin.is_some_and(SupportOriginId::convertible_to_tree)
}

/// A support of any kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SupportEntityValue {
    Trunk(Trunk),
    Branch(Branch),
    Leaf(Leaf),
    Twig(Twig),
    Stick(Stick),
    Brace(Brace),
    Stump(Stump),
    Kickstand(Kickstand),
}

/// A root or a knot a placement brings with it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PlacementPrimitive {
    Roots(Roots),
    Knot(Knot),
}

/// The support a placed one hangs from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostRef {
    pub type_id: SupportTypeId,
    pub id: String,
}

/// A placed support: its kind and itself, the roots and knots it brings (by
/// the field naming each), and what it hangs from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlacedSupport {
    pub type_id: SupportTypeId,
    pub entity: SupportEntityValue,
    pub supplied: IndexMap<String, PlacementPrimitive>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hosted_by: Option<HostRef>,
}

pub fn placement_of_resolved(
    type_id: SupportTypeId,
    entity: SupportEntityValue,
    supplied: IndexMap<String, PlacementPrimitive>,
    hosted_by: Option<HostRef>,
) -> PlacedSupport {
    PlacedSupport {
        type_id,
        entity,
        supplied,
        hosted_by,
    }
}
