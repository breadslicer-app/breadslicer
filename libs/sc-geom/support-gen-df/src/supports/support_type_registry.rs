//! DragonFruit `src/supports/supportTypeRegistry.ts`.
//!
//! What a support type IS: one descriptor per type, plus everything derived
//! from the descriptors. Registry order is observable (it is the order every
//! derived list walks in) and is kept exactly.
//!
//! DragonFruit fills several per-type slots at module load (`registerSupportUpdater`,
//! `registerContactBridgeBuilder`, `registerLateralStabiliser`, ...). Rust has no
//! load-time registration, so each slot's membership is declared here as the
//! fixed list DragonFruit's registrations produce, and the behaviour is called
//! directly from the type's own module (`support_types::<type>`) as those are
//! ported. The type-level helpers DragonFruit derives in TypeScript generics
//! (`SupportRemovalResult`, `PlacedSupport`, ...) become ordinary data where a
//! caller needs them.

use std::fmt;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::auto_support::constants::ANCHOR_HEIGHT_THRESHOLD_MM;
use super::types::{PlacementSurface, SupportCollections};

/// The sidebar's tabs, by the PAGE each opens rather than by a type.
///
/// The support-info tab carries the contact cone, cone angle and root settings
/// that apply to supports generally, so it is not any one type's page. Two of
/// the four are also tool panel ids, where page and tool are the same thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SidebarTab {
    SupportInfo,
    Raft,
    Grid,
    Bracing,
}

/// Every declared support type, named once in `SupportFieldsByType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SupportTypeId {
    Trunk,
    Branch,
    Leaf,
    Twig,
    Stick,
    Brace,
    Stump,
    Kickstand,
}

impl SupportTypeId {
    /// Every type, in registry order.
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

    /// Parses a type id as DragonFruit spells it. `None` for anything else.
    pub fn parse(id: &str) -> Option<SupportTypeId> {
        SupportTypeId::ALL.into_iter().find(|t| t.as_str() == id)
    }
}

impl fmt::Display for SupportTypeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `SupportTypeId | 'root' | 'joint' | 'knot' | 'segment' | 'contactDisk'`.
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

impl SupportSelectionCategory {
    pub const fn of_type(id: SupportTypeId) -> Self {
        match id {
            SupportTypeId::Trunk => SupportSelectionCategory::Trunk,
            SupportTypeId::Branch => SupportSelectionCategory::Branch,
            SupportTypeId::Leaf => SupportSelectionCategory::Leaf,
            SupportTypeId::Twig => SupportSelectionCategory::Twig,
            SupportTypeId::Stick => SupportSelectionCategory::Stick,
            SupportTypeId::Brace => SupportSelectionCategory::Brace,
            SupportTypeId::Stump => SupportSelectionCategory::Stump,
            SupportTypeId::Kickstand => SupportSelectionCategory::Kickstand,
        }
    }
}

/// Entity collections on SupportState, named once in SupportEntityByCollection.
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

impl SupportCollectionKey {
    pub const fn as_str(self) -> &'static str {
        match self {
            SupportCollectionKey::Roots => "roots",
            SupportCollectionKey::Trunks => "trunks",
            SupportCollectionKey::Branches => "branches",
            SupportCollectionKey::Leaves => "leaves",
            SupportCollectionKey::Twigs => "twigs",
            SupportCollectionKey::Sticks => "sticks",
            SupportCollectionKey::Braces => "braces",
            SupportCollectionKey::Stumps => "stumps",
            SupportCollectionKey::Kickstands => "kickstands",
            SupportCollectionKey::Knots => "knots",
        }
    }
}

/// Where a type's instances live. Kept as a discriminated shape rather than a
/// bare key so a second store would be a compile error at every read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportCollectionLocation {
    Support { key: SupportCollectionKey },
}

impl SupportCollectionLocation {
    pub const fn key(self) -> SupportCollectionKey {
        match self {
            SupportCollectionLocation::Support { key } => key,
        }
    }
}

/// What sits at one end of a support. A type is two endpoints plus whether a
/// shaft joins them; the vocabulary is closed over all eight.
///
/// - `PlateRoot`  -- a Roots row on the plate, via `rootId`.
/// - `InlineRoot` -- plate geometry on the entity, as an anchor carries.
/// - `Knot`       -- hangs from a knot on another support's shaft.
/// - `Cone` / `Disk` -- a contact primitive against the model.
/// - `None`       -- nothing declared at this end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SupportEndpointKind {
    PlateRoot,
    InlineRoot,
    Knot,
    Cone,
    Disk,
    None,
}

/// One end of a support: what kind it is, and the entity field carrying it.
///
/// `field` is absent for `None`, and for an endpoint whose link is already a
/// declared edge (`PlateRoot`, `Knot`) -- the edge names the field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupportEndpoint {
    pub kind: SupportEndpointKind,
    pub field: Option<&'static str>,
    /// For `InlineRoot`: the field holding the base's radius.
    ///
    /// An inline root is plate geometry on the entity itself, so its width is a
    /// field of that entity rather than the `diameter` of a shared `Roots` record.
    pub radius_field: Option<&'static str>,
    /// For `InlineRoot`: the fields holding the rest of the base's dimensions,
    /// which a `Roots` row would carry as `diameter` / `coneHeight`. Declared so
    /// the settings reach an inline root the same way they reach a shared one.
    pub top_radius_field: Option<&'static str>,
    pub height_field: Option<&'static str>,
}

impl SupportEndpoint {
    const fn of(kind: SupportEndpointKind) -> Self {
        SupportEndpoint {
            kind,
            field: None,
            radius_field: None,
            top_radius_field: None,
            height_field: None,
        }
    }

    const fn at(kind: SupportEndpointKind, field: &'static str) -> Self {
        SupportEndpoint {
            kind,
            field: Some(field),
            radius_field: None,
            top_radius_field: None,
            height_field: None,
        }
    }
}

/// What a placement rule measures.
///
/// - `ContactSpan` -- distance between the two model contacts a support bridges.
/// - `TipHeight`   -- height of the model contact above the plate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SupportPlacementMetric {
    ContactSpan,
    TipHeight,
}

/// A question answered by consulting live placement previews in turn, taking
/// the first that has an answer.
///
/// - `ContactGuideWidth`  -- how wide to draw the placement guide.
/// - `LimitationFeedback` -- which preview's error or warning to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SupportPreviewPurpose {
    ContactGuideWidth,
    LimitationFeedback,
}

/// A settings path a placement threshold may read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SupportPlacementSettingPath {
    /// `meshToMesh.stickVsTwigCutoffMm`
    MeshToMeshStickVsTwigCutoffMm,
}

/// A settings path a threshold reads from, with the fallback used when the
/// setting is absent. The union keeps a typo a compile error.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SupportPlacementThreshold {
    Mm(f64),
    Setting {
        setting: SupportPlacementSettingPath,
        fallback: f64,
    },
}

/// How densely auto-placement may lay a type down, and how it sizes the shafts.
/// `placement_rule` picks WHICH type serves a candidate; this is the policy for
/// placing it. Read via [`auto_placement_for`].
///
/// Declared but not yet read -- these were unreferenced constants when they
/// moved here. See docs/dev/support-registry-findings.md.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SupportAutoPlacement {
    /// Closest two of this type may sit (mm).
    pub min_spacing_mm: Option<f64>,
    /// Smallest region to densify: both XY extents must exceed this.
    pub min_xy_mm: Option<f64>,
    /// Smallest region area to densify (mm²); slivers are not load-bearing.
    pub min_area_mm2: Option<f64>,
    /// Multiplier over the sizing band; anchors carry the peel and run thick.
    pub shaft_multiplier: Option<f64>,
}

/// Which type claims a value exactly on a shared bound. Default `Lower`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PlacementBoundary {
    #[default]
    Lower,
    Upper,
}

/// The range of a measurement this type serves. The two rules disagree about
/// which side owns a value sitting exactly on a shared bound, so `boundary`
/// declares it per rule.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SupportPlacementRule {
    pub metric: SupportPlacementMetric,
    pub min_mm: Option<SupportPlacementThreshold>,
    pub max_mm: Option<SupportPlacementThreshold>,
    /// `None` is DragonFruit's absent field, read as `Lower`.
    pub boundary: Option<PlacementBoundary>,
}

/// Where an edge's id points: a collection, or `'segment'` (part of an entity).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SupportEdgeTarget {
    Collection(SupportCollectionKey),
    Segment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EdgeOwnership {
    Owns,
    HostedBy,
}

/// For a `HostedBy` edge, whether removing this entity removes the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TakeHost {
    Never,
    /// Only when nothing else references it.
    IfUnused,
    Always,
}

/// One link between entities. `field` holds the id, `to` names the collection it
/// points into (or `Segment`), and `ownership` is the cascade direction:
/// `Owns` removes the target with this entity, `HostedBy` removes this with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupportEdge {
    pub field: &'static str,
    pub to: SupportEdgeTarget,
    pub ownership: EdgeOwnership,
    pub take_host: Option<TakeHost>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PreviewShape {
    Support,
    Segment,
}

/// Where a type sits when several live previews answer the same question;
/// lower ranks first. `while_active` is an earlier rank used only while this
/// type's placement mode is active; `only_while_active` drops it otherwise.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreviewPriorityEntry {
    pub rank: f64,
    pub while_active: Option<f64>,
    pub only_while_active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PreviewPriority {
    pub contact_guide_width: Option<PreviewPriorityEntry>,
    pub limitation_feedback: Option<PreviewPriorityEntry>,
}

impl PreviewPriority {
    pub fn get(&self, purpose: SupportPreviewPurpose) -> Option<PreviewPriorityEntry> {
        match purpose {
            SupportPreviewPurpose::ContactGuideWidth => self.contact_guide_width,
            SupportPreviewPurpose::LimitationFeedback => self.limitation_feedback,
        }
    }
}

/// `'all'` tapers every segment; `'last'` only the terminal one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShaftTaperSegments {
    All,
    Last,
}

/// Where a tapering shaft reads its two end diameters, and on which segment.
/// A taper whose ends differ cannot be instanced, so the whole support drops
/// out of the batched pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShaftTaper {
    pub segments: ShaftTaperSegments,
    /// Entity paths holding the start and end diameters.
    pub from: [&'static str; 2],
}

/// Which shaft this type's contact-cone body follows. The resize pass
/// thickens shafts after cones are built, so `syncContactConeDiameters` sets
/// the body back to the shaft; the tip diameter never moves. Declaring a
/// source also makes this type's segments readable as a host for a cone
/// hosted through a knot.
///
/// - `OwnLastSegment` -- the shaft climbs into the cone;
/// - `OwnFirstSegment` -- the shaft leaves the cone and hangs down;
/// - `HostKnotSegment` -- the cone sits on the model, and the host shaft is
///   read through the knot this entity hangs from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConeBodyFollows {
    OwnFirstSegment,
    OwnLastSegment,
    HostKnotSegment,
}

/// How interior view decides whether an instance is inside the cavity.
///
/// - `Contacts`  -- test the type's own declared contacts.
/// - `Inherited` -- it has none of its own; resolve through its host knots.
/// - `Hidden`    -- never shown in interior view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InteriorVisibility {
    Contacts,
    Inherited,
    Hidden,
}

/// Shaft diameter when the entity carries no segments. A number is the
/// constant; a `Path` reads the diameter off the entity (a dotted path,
/// resolved like `shaft_taper.from`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FallbackDiameter {
    Mm(f64),
    Path(&'static str),
}

/// How a shaft behaves when its geometry cannot be resolved. Only reached
/// on malformed geometry, and the values differ per type.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShaftFallback {
    /// No top joint and no contact: 10 for a trunk, 5 elsewhere. Inherited drift.
    pub stub_length_mm: f64,
    /// Whether an unresolvable start falls back to the split point. True for
    /// self-contained types; a hosted type stays straight instead.
    pub start_falls_back_to_split_point: bool,
    pub fallback_diameter_mm: Option<FallbackDiameter>,
}

/// Names this type was known by before, for reading payloads written then.
/// `migrateSupportPayload` is the only reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenamedFrom {
    /// Type ids this type's entities were stamped with.
    pub ids: &'static [&'static str],
    /// SupportState collection keys its entities were stored under.
    pub collection_keys: &'static [&'static str],
}

/// What a support type IS — not how it renders, builds or places. Adding a
/// renderer or builder reference here turns a mechanical refactor into a rewrite.
///
/// Field documentation is DragonFruit's; see `supportTypeRegistry.ts` for the
/// full prose on each flag.
#[derive(Debug, Clone, PartialEq)]
pub struct SupportTypeDescriptor {
    pub id: SupportTypeId,
    /// Plural display name, so panels listing collections need no label table.
    /// RENAMING A TYPE MUST UPDATE THIS.
    pub label: &'static str,
    /// Singular lower-case name, for internal labels and logs.
    pub singular: &'static str,
    pub location: SupportCollectionLocation,
    pub selection_category: SupportSelectionCategory,
    /// Whether this type is serialised as a bundle rather than a bare entity
    /// (kickstand: `{ kickstand, root, hostKnot }`).
    pub serialised_as_bundle: bool,
    /// The action a before/after edit to this type records, when it has one of its own.
    pub history_update: Option<&'static str>,
    /// The sidebar tab this type is edited under.
    pub sidebar_tab: SidebarTab,
    /// Whether this type's diameter is re-solved from the members it carries.
    pub recomputes_diameter_from_attachments: bool,
    /// Whether adding one of these re-solves the diameter of the host it hangs from.
    pub repairs_host_diameter_on_add: bool,
    /// Whether the bridge search may reach sideways to land this type.
    pub may_reach_sideways: bool,
    /// Whether instances can host a fan link off their shaft.
    pub can_be_grid_host: bool,
    /// Whether a kickstand's host knot may ride this type's segments.
    pub hosts_kickstand: bool,
    /// Whether the auto-support pass places this type and the ledger reports it.
    pub is_auto_placed: bool,
    /// Whether instances carry real shafts, for segment and joint walks.
    pub has_segments: bool,
    /// Contact primitive fields, lower end first.
    pub contact_fields: &'static [&'static str],
    /// Prefix a selection id carries when one of this type's segments is selected.
    pub segment_selection_prefix: Option<&'static str>,
    /// Prefix a knot's `parentShaftId` carries when it rides this type rather
    /// than a real shaft segment: a leaf's contact cone, a brace's span.
    pub knot_host_prefix: Option<&'static str>,
    /// Whether instances record an auto-support `origin`.
    pub has_origin: bool,
    /// Whether the type has an interactive placement hook, and so a live placement preview.
    pub has_placement_preview: bool,
    /// Whether the placement router hands this type pointer gestures on a model face.
    pub claims_model_surface_gestures: bool,
    /// What a live placement preview carries.
    pub preview_shape: Option<PreviewShape>,
    /// Whether auto-bracing may brace this type's shafts.
    pub is_auto_braceable: bool,
    /// Whether the auto-bracing hotkey is available while this type's tool is active.
    pub has_auto_bracing_hotkey: bool,
    /// Whether a brace endpoint snaps to this type's contact cone.
    pub hosts_brace_snap_cone: bool,
    /// The measurement range this type serves, when chosen automatically.
    pub placement_rule: Option<SupportPlacementRule>,
    /// Names this type was known by before.
    pub renamed_from: Option<RenamedFrom>,
    /// Auto-placement density and shaft sizing.
    pub auto_placement: Option<SupportAutoPlacement>,
    /// Whether this type's preview yields to any other placement mode.
    pub preview_yields_to_other_modes: bool,
    /// Whether the preview shows only while this type's own placement mode is active.
    pub preview_requires_own_mode: bool,
    /// Whether this type's placement mode displaces the default tool's preview.
    pub placement_mode_displaces_default: bool,
    /// Where this type sits when several live previews answer the same question.
    pub preview_priority: Option<PreviewPriority>,
    /// Where a tapering shaft reads its two end diameters.
    pub shaft_taper: Option<ShaftTaper>,
    /// Which shaft this type's contact-cone body follows.
    pub cone_body_follows: Option<ConeBodyFollows>,
    /// What sits at the bottom of this type.
    pub lower: SupportEndpoint,
    /// What sits at the top of this type.
    pub upper: SupportEndpoint,
    /// Whether every segment carries both its own joints.
    pub segments_carry_both_joints: bool,
    /// Whether the type is placed by its own dedicated snap pass.
    pub has_dedicated_snap_pass: bool,
    /// Whether a contact cone on this type carries `diskLengthOverride`.
    pub has_contact_disk_length_override: bool,
    /// Whether a model transform marks this type's segments as moved.
    pub transform_propagates_to_shaft: bool,
    /// Whether an edit gizmo records its own before/after history entry.
    pub owns_edit_history_entry: bool,
    /// Whether a joint drag seeds and reads a live preview of the entity.
    pub joint_drag_uses_live_preview: bool,
    /// Whether a curve drag with no preview re-reads the entity before committing.
    pub curve_drag_reconciles_from_store: bool,
    /// Prefix for this type's bezier handle context ids.
    pub bezier_context_id_prefix: &'static str,
    /// Whether reshaping this type's curve broadcasts its attached knots and leaves live.
    pub broadcasts_attachments_while_dragging: bool,
    /// Whether a knot riding this shaft renders at the joint diameter.
    pub knot_takes_joint_diameter: bool,
    /// Whether a knot carrying no `t` is projected onto the segment.
    pub projects_unparameterised_knots: bool,
    /// Whether a knot drag on this type's shaft defers elastic-chain solving to release.
    pub knot_drag_defers_elastic_preview: bool,
    /// Whether a knot drag on this type's shaft computes its preview inline.
    pub knot_drag_computes_inline: bool,
    /// Whether this type's shaft flexes when the knot it hangs from is dragged.
    pub flexes_on_host_knot_drag: bool,
    /// Whether a knot drag updates the attached leaf cones' wide-end diameter.
    pub knot_drag_updates_leaf_cone_diameter: bool,
    /// How interior view decides whether an instance is inside the cavity.
    pub interior_visibility: InteriorVisibility,
    /// Whether unselected contact cones are drawn by the shared batched pass.
    pub batches_contact_cones: bool,
    /// Whether the shared batched passes draw this type's shaft.
    pub batches_shaft: bool,
    /// Whether dragging a joint re-solves the type's contact primitives.
    pub joint_drag_moves_contacts: bool,
    /// Whether a joint drag may turn this type's segments into curves.
    pub joint_drag_can_curve_shaft: bool,
    /// Whether instances own a Roots entry, via a `rootId` field.
    pub owns_root: bool,
    /// How instances link to other entities.
    pub edges: &'static [SupportEdge],
    /// How a shaft behaves when its geometry cannot be resolved.
    pub shaft_fallback: ShaftFallback,
    /// Whether instances have per-entity editable settings.
    pub has_editable_settings: bool,
    /// Whether the settings sidebar offers a panel for this type.
    pub offers_sidebar_panel: bool,
}

impl SupportTypeDescriptor {
    /// `support:add-<id>`, derived from the id for every type.
    pub fn history_add(&self) -> String {
        add_action(self.id)
    }

    /// `support:remove-<id>`, derived from the id for every type.
    pub fn history_remove(&self) -> String {
        remove_action(self.id)
    }
}

/// `history/actionTypes.ts` `addAction`.
pub fn add_action(type_id: SupportTypeId) -> String {
    format!("support:add-{type_id}")
}

/// `history/actionTypes.ts` `removeAction`.
pub fn remove_action(type_id: SupportTypeId) -> String {
    format!("support:remove-{type_id}")
}

pub const SUPPORT_UPDATE_TRUNK: &str = "support:update-trunk";
pub const SUPPORT_UPDATE_BRANCH: &str = "support:update-branch";

use EdgeOwnership::{HostedBy, Owns};
use SupportCollectionKey as Key;
use SupportEndpointKind as End;

const fn edge(
    field: &'static str,
    to: SupportEdgeTarget,
    ownership: EdgeOwnership,
    take_host: Option<TakeHost>,
) -> SupportEdge {
    SupportEdge {
        field,
        to,
        ownership,
        take_host,
    }
}

const ROOTS: SupportEdgeTarget = SupportEdgeTarget::Collection(Key::Roots);
const KNOTS: SupportEdgeTarget = SupportEdgeTarget::Collection(Key::Knots);

const fn priority(
    rank: f64,
    while_active: Option<f64>,
    only_while_active: bool,
) -> PreviewPriorityEntry {
    PreviewPriorityEntry {
        rank,
        while_active,
        only_while_active,
    }
}

const STICK_VS_TWIG_CUTOFF: SupportPlacementThreshold = SupportPlacementThreshold::Setting {
    setting: SupportPlacementSettingPath::MeshToMeshStickVsTwigCutoffMm,
    fallback: 5.0,
};

/// Each type's declaration, in registry order.
pub static SUPPORT_TYPES: [SupportTypeDescriptor; 8] = [
    SupportTypeDescriptor {
        id: SupportTypeId::Trunk,
        sidebar_tab: SidebarTab::SupportInfo,
        has_editable_settings: true,
        offers_sidebar_panel: true,
        recomputes_diameter_from_attachments: true,
        repairs_host_diameter_on_add: false,
        may_reach_sideways: false,
        can_be_grid_host: true,
        hosts_kickstand: true,
        edges: &[edge("rootId", ROOTS, Owns, None)],
        owns_root: true,
        segments_carry_both_joints: false,
        has_dedicated_snap_pass: true,
        has_contact_disk_length_override: true,
        transform_propagates_to_shaft: true,
        owns_edit_history_entry: true,
        joint_drag_uses_live_preview: true,
        curve_drag_reconciles_from_store: false,
        batches_contact_cones: true,
        batches_shaft: true,
        bezier_context_id_prefix: "",
        broadcasts_attachments_while_dragging: false,
        knot_takes_joint_diameter: true,
        projects_unparameterised_knots: true,
        knot_drag_defers_elastic_preview: true,
        knot_drag_computes_inline: true,
        flexes_on_host_knot_drag: false,
        knot_drag_updates_leaf_cone_diameter: false,
        interior_visibility: InteriorVisibility::Contacts,
        joint_drag_moves_contacts: false,
        joint_drag_can_curve_shaft: true,
        contact_fields: &["contactCone"],
        shaft_fallback: ShaftFallback {
            stub_length_mm: 10.0,
            start_falls_back_to_split_point: false,
            fallback_diameter_mm: Some(FallbackDiameter::Mm(1.5)),
        },
        segment_selection_prefix: None,
        knot_host_prefix: None,
        has_origin: true,
        has_placement_preview: true,
        claims_model_surface_gestures: false,
        preview_shape: Some(PreviewShape::Support),
        preview_yields_to_other_modes: true,
        preview_requires_own_mode: false,
        placement_mode_displaces_default: false,
        preview_priority: Some(PreviewPriority {
            contact_guide_width: Some(priority(0.0, None, false)),
            limitation_feedback: Some(priority(2.0, None, false)),
        }),
        placement_rule: Some(SupportPlacementRule {
            metric: SupportPlacementMetric::TipHeight,
            min_mm: Some(SupportPlacementThreshold::Mm(ANCHOR_HEIGHT_THRESHOLD_MM)),
            max_mm: None,
            boundary: Some(PlacementBoundary::Upper),
        }),
        renamed_from: None,
        auto_placement: None,
        is_auto_braceable: true,
        has_auto_bracing_hotkey: false,
        hosts_brace_snap_cone: false,
        shaft_taper: None,
        // The shaft climbs INTO the cone, so the terminal segment is the one
        // under it.
        cone_body_follows: Some(ConeBodyFollows::OwnLastSegment),
        lower: SupportEndpoint::of(End::PlateRoot),
        upper: SupportEndpoint::at(End::Cone, "contactCone"),
        is_auto_placed: true,
        has_segments: true,
        label: "Trunks",
        singular: "trunk",
        location: SupportCollectionLocation::Support { key: Key::Trunks },
        selection_category: SupportSelectionCategory::Trunk,
        serialised_as_bundle: false,
        history_update: Some(SUPPORT_UPDATE_TRUNK),
    },
    SupportTypeDescriptor {
        id: SupportTypeId::Branch,
        sidebar_tab: SidebarTab::SupportInfo,
        has_editable_settings: true,
        offers_sidebar_panel: true,
        flexes_on_host_knot_drag: true,
        edges: &[edge(
            "parentKnotId",
            KNOTS,
            HostedBy,
            Some(TakeHost::Always),
        )],
        owns_root: false,
        segments_carry_both_joints: false,
        has_dedicated_snap_pass: true,
        has_contact_disk_length_override: true,
        transform_propagates_to_shaft: true,
        owns_edit_history_entry: false,
        joint_drag_uses_live_preview: true,
        curve_drag_reconciles_from_store: true,
        batches_contact_cones: true,
        batches_shaft: true,
        bezier_context_id_prefix: "branch-",
        broadcasts_attachments_while_dragging: false,
        knot_takes_joint_diameter: false,
        projects_unparameterised_knots: false,
        knot_drag_defers_elastic_preview: false,
        knot_drag_computes_inline: false,
        knot_drag_updates_leaf_cone_diameter: false,
        interior_visibility: InteriorVisibility::Contacts,
        joint_drag_moves_contacts: false,
        joint_drag_can_curve_shaft: false,
        contact_fields: &["contactCone"],
        shaft_fallback: ShaftFallback {
            stub_length_mm: 5.0,
            start_falls_back_to_split_point: false,
            fallback_diameter_mm: None,
        },
        segment_selection_prefix: None,
        knot_host_prefix: None,
        has_origin: true,
        has_placement_preview: true,
        claims_model_surface_gestures: true,
        preview_shape: Some(PreviewShape::Support),
        preview_yields_to_other_modes: false,
        preview_requires_own_mode: true,
        placement_mode_displaces_default: true,
        preview_priority: Some(PreviewPriority {
            contact_guide_width: Some(priority(1.0, Some(-3.0), false)),
            limitation_feedback: Some(priority(1.0, None, true)),
        }),
        placement_rule: None,
        renamed_from: None,
        auto_placement: None,
        is_auto_braceable: true,
        has_auto_bracing_hotkey: false,
        hosts_brace_snap_cone: false,
        shaft_taper: None,
        // The shaft leaves the cone and hangs DOWN, so the first segment is
        // the one under it.
        cone_body_follows: Some(ConeBodyFollows::OwnFirstSegment),
        lower: SupportEndpoint::of(End::Knot),
        upper: SupportEndpoint::at(End::Cone, "contactCone"),
        recomputes_diameter_from_attachments: false,
        repairs_host_diameter_on_add: true,
        may_reach_sideways: false,
        can_be_grid_host: false,
        hosts_kickstand: true,
        is_auto_placed: true,
        has_segments: true,
        label: "Branches",
        singular: "branch",
        location: SupportCollectionLocation::Support { key: Key::Branches },
        selection_category: SupportSelectionCategory::Branch,
        serialised_as_bundle: false,
        history_update: Some(SUPPORT_UPDATE_BRANCH),
    },
    SupportTypeDescriptor {
        id: SupportTypeId::Leaf,
        sidebar_tab: SidebarTab::SupportInfo,
        has_editable_settings: true,
        offers_sidebar_panel: true,
        knot_host_prefix: Some("leafCone:"),
        hosts_brace_snap_cone: true,
        edges: &[edge(
            "parentKnotId",
            KNOTS,
            HostedBy,
            Some(TakeHost::IfUnused),
        )],
        owns_root: false,
        segments_carry_both_joints: true,
        has_dedicated_snap_pass: false,
        has_contact_disk_length_override: false,
        transform_propagates_to_shaft: true,
        owns_edit_history_entry: false,
        joint_drag_uses_live_preview: true,
        curve_drag_reconciles_from_store: false,
        batches_contact_cones: true,
        batches_shaft: false,
        bezier_context_id_prefix: "leaf-",
        broadcasts_attachments_while_dragging: false,
        knot_takes_joint_diameter: false,
        projects_unparameterised_knots: false,
        knot_drag_defers_elastic_preview: false,
        knot_drag_computes_inline: false,
        flexes_on_host_knot_drag: false,
        knot_drag_updates_leaf_cone_diameter: false,
        interior_visibility: InteriorVisibility::Contacts,
        joint_drag_moves_contacts: false,
        joint_drag_can_curve_shaft: false,
        contact_fields: &["contactCone"],
        shaft_fallback: ShaftFallback {
            stub_length_mm: 5.0,
            start_falls_back_to_split_point: false,
            fallback_diameter_mm: None,
        },
        segment_selection_prefix: None,
        has_origin: true,
        has_placement_preview: true,
        claims_model_surface_gestures: true,
        preview_shape: Some(PreviewShape::Support),
        preview_yields_to_other_modes: false,
        preview_requires_own_mode: false,
        placement_mode_displaces_default: true,
        preview_priority: Some(PreviewPriority {
            contact_guide_width: Some(priority(2.0, Some(-2.0), false)),
            limitation_feedback: Some(priority(0.0, None, false)),
        }),
        placement_rule: None,
        renamed_from: None,
        auto_placement: None,
        is_auto_braceable: false,
        has_auto_bracing_hotkey: false,
        shaft_taper: None,
        // The cone sits on the model while the leaf hangs off a knot on someone
        // else's shaft, so the host is read through that knot.
        cone_body_follows: Some(ConeBodyFollows::HostKnotSegment),
        lower: SupportEndpoint::of(End::Knot),
        upper: SupportEndpoint::at(End::Cone, "contactCone"),
        recomputes_diameter_from_attachments: false,
        repairs_host_diameter_on_add: false,
        may_reach_sideways: false,
        can_be_grid_host: false,
        hosts_kickstand: false,
        is_auto_placed: true,
        has_segments: false,
        label: "Leaves",
        singular: "leaf",
        location: SupportCollectionLocation::Support { key: Key::Leaves },
        selection_category: SupportSelectionCategory::Leaf,
        serialised_as_bundle: false,
        history_update: None,
    },
    SupportTypeDescriptor {
        id: SupportTypeId::Twig,
        sidebar_tab: SidebarTab::SupportInfo,
        has_editable_settings: false,
        offers_sidebar_panel: true,
        edges: &[],
        owns_root: false,
        segments_carry_both_joints: true,
        has_dedicated_snap_pass: false,
        has_contact_disk_length_override: false,
        transform_propagates_to_shaft: true,
        owns_edit_history_entry: false,
        joint_drag_uses_live_preview: true,
        curve_drag_reconciles_from_store: false,
        batches_contact_cones: false,
        batches_shaft: true,
        bezier_context_id_prefix: "twig-",
        broadcasts_attachments_while_dragging: true,
        knot_drag_updates_leaf_cone_diameter: true,
        knot_takes_joint_diameter: false,
        projects_unparameterised_knots: false,
        knot_drag_defers_elastic_preview: false,
        knot_drag_computes_inline: false,
        flexes_on_host_knot_drag: false,
        interior_visibility: InteriorVisibility::Contacts,
        joint_drag_moves_contacts: true,
        joint_drag_can_curve_shaft: false,
        contact_fields: &["contactDiskA", "contactDiskB"],
        shaft_fallback: ShaftFallback {
            stub_length_mm: 5.0,
            start_falls_back_to_split_point: true,
            fallback_diameter_mm: None,
        },
        segment_selection_prefix: None,
        knot_host_prefix: None,
        has_origin: false,
        shaft_taper: Some(ShaftTaper {
            segments: ShaftTaperSegments::All,
            from: [
                "contactDiskA.contactDiameterMm",
                "contactDiskB.contactDiameterMm",
            ],
        }),
        has_placement_preview: false,
        claims_model_surface_gestures: false,
        preview_shape: None,
        preview_yields_to_other_modes: false,
        preview_requires_own_mode: false,
        placement_mode_displaces_default: false,
        preview_priority: None,
        placement_rule: Some(SupportPlacementRule {
            metric: SupportPlacementMetric::ContactSpan,
            min_mm: None,
            max_mm: Some(STICK_VS_TWIG_CUTOFF),
            boundary: None,
        }),
        renamed_from: None,
        auto_placement: None,
        is_auto_braceable: false,
        has_auto_bracing_hotkey: false,
        hosts_brace_snap_cone: false,
        cone_body_follows: None,
        lower: SupportEndpoint::at(End::Disk, "contactDiskA"),
        upper: SupportEndpoint::at(End::Disk, "contactDiskB"),
        recomputes_diameter_from_attachments: false,
        repairs_host_diameter_on_add: false,
        may_reach_sideways: true,
        can_be_grid_host: false,
        hosts_kickstand: true,
        is_auto_placed: true,
        has_segments: true,
        label: "Twigs",
        singular: "twig",
        location: SupportCollectionLocation::Support { key: Key::Twigs },
        selection_category: SupportSelectionCategory::Twig,
        serialised_as_bundle: false,
        history_update: None,
    },
    SupportTypeDescriptor {
        id: SupportTypeId::Stick,
        sidebar_tab: SidebarTab::Bracing,
        has_editable_settings: false,
        offers_sidebar_panel: true,
        edges: &[],
        owns_root: false,
        segments_carry_both_joints: true,
        has_dedicated_snap_pass: false,
        has_contact_disk_length_override: false,
        transform_propagates_to_shaft: true,
        owns_edit_history_entry: false,
        joint_drag_uses_live_preview: true,
        curve_drag_reconciles_from_store: false,
        batches_contact_cones: true,
        batches_shaft: true,
        bezier_context_id_prefix: "stick-",
        has_auto_bracing_hotkey: true,
        broadcasts_attachments_while_dragging: false,
        knot_takes_joint_diameter: false,
        projects_unparameterised_knots: false,
        knot_drag_defers_elastic_preview: false,
        knot_drag_computes_inline: false,
        flexes_on_host_knot_drag: false,
        knot_drag_updates_leaf_cone_diameter: false,
        interior_visibility: InteriorVisibility::Contacts,
        joint_drag_moves_contacts: true,
        joint_drag_can_curve_shaft: false,
        contact_fields: &["contactConeA", "contactConeB"],
        shaft_fallback: ShaftFallback {
            stub_length_mm: 5.0,
            start_falls_back_to_split_point: true,
            fallback_diameter_mm: None,
        },
        segment_selection_prefix: None,
        knot_host_prefix: None,
        has_origin: false,
        has_placement_preview: false,
        claims_model_surface_gestures: false,
        preview_shape: None,
        preview_yields_to_other_modes: false,
        preview_requires_own_mode: false,
        placement_mode_displaces_default: false,
        preview_priority: None,
        placement_rule: Some(SupportPlacementRule {
            metric: SupportPlacementMetric::ContactSpan,
            min_mm: Some(STICK_VS_TWIG_CUTOFF),
            max_mm: None,
            boundary: None,
        }),
        renamed_from: None,
        auto_placement: None,
        is_auto_braceable: false,
        hosts_brace_snap_cone: false,
        shaft_taper: None,
        cone_body_follows: None,
        lower: SupportEndpoint::at(End::Cone, "contactConeA"),
        upper: SupportEndpoint::at(End::Cone, "contactConeB"),
        recomputes_diameter_from_attachments: false,
        repairs_host_diameter_on_add: false,
        may_reach_sideways: false,
        can_be_grid_host: false,
        hosts_kickstand: true,
        is_auto_placed: true,
        has_segments: true,
        label: "Sticks",
        singular: "stick",
        location: SupportCollectionLocation::Support { key: Key::Sticks },
        selection_category: SupportSelectionCategory::Stick,
        serialised_as_bundle: false,
        history_update: None,
    },
    SupportTypeDescriptor {
        id: SupportTypeId::Brace,
        sidebar_tab: SidebarTab::SupportInfo,
        // Two named knot fields rather than a list: the history payload and its
        // undo handler read them by name, and start/end are not interchangeable.
        has_editable_settings: false,
        offers_sidebar_panel: false,
        edges: &[
            edge("startKnotId", KNOTS, HostedBy, Some(TakeHost::IfUnused)),
            edge("endKnotId", KNOTS, HostedBy, Some(TakeHost::IfUnused)),
        ],
        owns_root: false,
        segments_carry_both_joints: true,
        has_dedicated_snap_pass: true,
        has_contact_disk_length_override: false,
        transform_propagates_to_shaft: true,
        owns_edit_history_entry: false,
        joint_drag_uses_live_preview: true,
        curve_drag_reconciles_from_store: false,
        batches_contact_cones: false,
        batches_shaft: false,
        bezier_context_id_prefix: "brace-",
        broadcasts_attachments_while_dragging: false,
        knot_takes_joint_diameter: false,
        projects_unparameterised_knots: false,
        knot_drag_defers_elastic_preview: false,
        knot_drag_computes_inline: false,
        flexes_on_host_knot_drag: false,
        knot_drag_updates_leaf_cone_diameter: false,
        interior_visibility: InteriorVisibility::Inherited,
        joint_drag_moves_contacts: false,
        joint_drag_can_curve_shaft: false,
        contact_fields: &[],
        shaft_fallback: ShaftFallback {
            stub_length_mm: 5.0,
            start_falls_back_to_split_point: false,
            fallback_diameter_mm: None,
        },
        segment_selection_prefix: Some("braceSegment:"),
        knot_host_prefix: Some("braceSegment:"),
        has_origin: false,
        has_placement_preview: true,
        claims_model_surface_gestures: false,
        preview_shape: Some(PreviewShape::Segment),
        preview_yields_to_other_modes: false,
        preview_requires_own_mode: false,
        placement_mode_displaces_default: false,
        preview_priority: None,
        placement_rule: None,
        renamed_from: None,
        auto_placement: None,
        is_auto_braceable: false,
        has_auto_bracing_hotkey: false,
        hosts_brace_snap_cone: false,
        shaft_taper: None,
        cone_body_follows: None,
        lower: SupportEndpoint::of(End::Knot),
        upper: SupportEndpoint::of(End::Knot),
        recomputes_diameter_from_attachments: false,
        repairs_host_diameter_on_add: false,
        may_reach_sideways: false,
        can_be_grid_host: false,
        hosts_kickstand: true,
        is_auto_placed: false,
        has_segments: false,
        label: "Braces",
        singular: "brace",
        location: SupportCollectionLocation::Support { key: Key::Braces },
        selection_category: SupportSelectionCategory::Brace,
        serialised_as_bundle: false,
        history_update: None,
    },
    SupportTypeDescriptor {
        id: SupportTypeId::Stump,
        // Written before the rename, so payloads saved then still load.
        renamed_from: Some(RenamedFrom {
            ids: &["anchor"],
            collection_keys: &["anchors"],
        }),
        sidebar_tab: SidebarTab::SupportInfo,
        has_editable_settings: true,
        offers_sidebar_panel: false,
        edges: &[],
        owns_root: false,
        segments_carry_both_joints: true,
        has_dedicated_snap_pass: false,
        has_contact_disk_length_override: false,
        transform_propagates_to_shaft: false,
        owns_edit_history_entry: false,
        joint_drag_uses_live_preview: true,
        curve_drag_reconciles_from_store: false,
        batches_contact_cones: false,
        batches_shaft: false,
        bezier_context_id_prefix: "stump-",
        broadcasts_attachments_while_dragging: false,
        knot_takes_joint_diameter: false,
        projects_unparameterised_knots: false,
        knot_drag_defers_elastic_preview: false,
        knot_drag_computes_inline: false,
        flexes_on_host_knot_drag: false,
        knot_drag_updates_leaf_cone_diameter: false,
        interior_visibility: InteriorVisibility::Contacts,
        joint_drag_moves_contacts: false,
        joint_drag_can_curve_shaft: false,
        contact_fields: &["contactCone"],
        shaft_fallback: ShaftFallback {
            stub_length_mm: 5.0,
            start_falls_back_to_split_point: true,
            fallback_diameter_mm: None,
        },
        segment_selection_prefix: None,
        knot_host_prefix: None,
        has_origin: true,
        has_placement_preview: false,
        claims_model_surface_gestures: false,
        preview_shape: None,
        preview_yields_to_other_modes: false,
        preview_requires_own_mode: false,
        placement_mode_displaces_default: false,
        preview_priority: None,
        placement_rule: Some(SupportPlacementRule {
            metric: SupportPlacementMetric::TipHeight,
            min_mm: None,
            max_mm: Some(SupportPlacementThreshold::Mm(ANCHOR_HEIGHT_THRESHOLD_MM)),
            boundary: Some(PlacementBoundary::Upper),
        }),
        auto_placement: Some(SupportAutoPlacement {
            min_spacing_mm: Some(1.8),
            min_xy_mm: Some(4.0),
            min_area_mm2: Some(12.0),
            shaft_multiplier: Some(1.25),
        }),
        is_auto_braceable: false,
        has_auto_bracing_hotkey: false,
        hosts_brace_snap_cone: false,
        shaft_taper: None,
        cone_body_follows: None,
        lower: SupportEndpoint {
            kind: End::InlineRoot,
            field: Some("rootPos"),
            radius_field: Some("rootBaseDiameter"),
            top_radius_field: Some("rootTopDiameter"),
            height_field: Some("rootHeight"),
        },
        upper: SupportEndpoint::at(End::Cone, "contactCone"),
        recomputes_diameter_from_attachments: false,
        repairs_host_diameter_on_add: false,
        may_reach_sideways: false,
        can_be_grid_host: false,
        hosts_kickstand: true,
        is_auto_placed: true,
        has_segments: true,
        label: "Stumps",
        singular: "stump",
        location: SupportCollectionLocation::Support { key: Key::Stumps },
        selection_category: SupportSelectionCategory::Stump,
        serialised_as_bundle: false,
        history_update: None,
    },
    SupportTypeDescriptor {
        id: SupportTypeId::Kickstand,
        sidebar_tab: SidebarTab::SupportInfo,
        has_editable_settings: true,
        offers_sidebar_panel: false,
        edges: &[
            edge("rootId", ROOTS, Owns, None),
            edge("hostKnotId", KNOTS, HostedBy, Some(TakeHost::Always)),
            edge(
                "hostSegmentId",
                SupportEdgeTarget::Segment,
                HostedBy,
                Some(TakeHost::Always),
            ),
        ],
        owns_root: true,
        segments_carry_both_joints: false,
        has_dedicated_snap_pass: false,
        has_contact_disk_length_override: false,
        transform_propagates_to_shaft: true,
        owns_edit_history_entry: false,
        joint_drag_uses_live_preview: false,
        curve_drag_reconciles_from_store: false,
        batches_contact_cones: false,
        batches_shaft: true,
        bezier_context_id_prefix: "kickstand-",
        broadcasts_attachments_while_dragging: false,
        knot_takes_joint_diameter: false,
        projects_unparameterised_knots: false,
        knot_drag_defers_elastic_preview: false,
        knot_drag_computes_inline: false,
        flexes_on_host_knot_drag: false,
        knot_drag_updates_leaf_cone_diameter: false,
        interior_visibility: InteriorVisibility::Hidden,
        joint_drag_moves_contacts: false,
        joint_drag_can_curve_shaft: true,
        contact_fields: &[],
        shaft_fallback: ShaftFallback {
            stub_length_mm: 5.0,
            start_falls_back_to_split_point: false,
            fallback_diameter_mm: Some(FallbackDiameter::Path("profile.bodyDiameterMm")),
        },
        segment_selection_prefix: None,
        knot_host_prefix: None,
        has_origin: false,
        shaft_taper: Some(ShaftTaper {
            segments: ShaftTaperSegments::Last,
            from: [
                "profile.terminalStartDiameterMm",
                "profile.terminalEndDiameterMm",
            ],
        }),
        has_placement_preview: true,
        claims_model_surface_gestures: false,
        preview_shape: Some(PreviewShape::Support),
        preview_yields_to_other_modes: false,
        preview_requires_own_mode: false,
        placement_mode_displaces_default: true,
        preview_priority: Some(PreviewPriority {
            contact_guide_width: Some(priority(3.0, Some(-1.0), false)),
            limitation_feedback: None,
        }),
        placement_rule: None,
        renamed_from: None,
        auto_placement: None,
        is_auto_braceable: true,
        has_auto_bracing_hotkey: false,
        hosts_brace_snap_cone: false,
        cone_body_follows: None,
        lower: SupportEndpoint::of(End::PlateRoot),
        upper: SupportEndpoint::of(End::Knot),
        recomputes_diameter_from_attachments: false,
        repairs_host_diameter_on_add: false,
        may_reach_sideways: false,
        can_be_grid_host: false,
        hosts_kickstand: true,
        is_auto_placed: false,
        has_segments: true,
        label: "Kickstands",
        singular: "kickstand",
        location: SupportCollectionLocation::Support {
            key: Key::Kickstands,
        },
        selection_category: SupportSelectionCategory::Kickstand,
        serialised_as_bundle: true,
        history_update: None,
    },
];

/// The store's id -> type scan (`state.ts` `getSupportTypeOf`).
pub type SupportTypeResolver = dyn Fn(&str) -> Option<SupportTypeId>;

pub fn get_support_type_descriptor(id: SupportTypeId) -> &'static SupportTypeDescriptor {
    &SUPPORT_TYPES[id as usize]
}

/// The type an entity is, so a reader can dispatch without the caller restating
/// it. Falls back to the store's membership scan for an entity that lost its
/// `typeId` -- a whole-store payload restored through `setSnapshot` bypasses the
/// writers that stamp it. The scan is the store's (`state.ts`
/// `registerSupportTypeResolver`), passed in by the caller.
pub fn resolve_support_type_id_of(
    type_id: Option<SupportTypeId>,
    id: &str,
    resolve: Option<&SupportTypeResolver>,
) -> Option<SupportTypeId> {
    type_id.or_else(|| resolve.and_then(|resolve| resolve(id)))
}

/// What each type's removal returns, so undo can rebuild what it deleted.
/// `self_field` is the field the entity arrives under; `cascade` maps a
/// collection to the field(s) its removed members arrive under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupportRemovalShape {
    pub self_field: &'static str,
    pub cascade: RemovalCascade,
}

/// A collection and the field(s) its removed members arrive under.
pub type RemovalCascade = &'static [(SupportCollectionKey, &'static [&'static str])];

pub fn removal_shape_for(type_id: SupportTypeId) -> SupportRemovalShape {
    use SupportCollectionKey::*;
    let (self_field, cascade): (&'static str, RemovalCascade) = match type_id {
        SupportTypeId::Trunk => (
            "trunk",
            &[
                (Roots, &["roots"]),
                (Branches, &["branches"]),
                (Braces, &["braces"]),
                (Kickstands, &["kickstands"]),
                (Leaves, &["leaves"]),
                (Knots, &["knots"]),
            ],
        ),
        SupportTypeId::Branch => (
            "branch",
            &[
                (Branches, &["branches"]),
                (Braces, &["braces"]),
                (Kickstands, &["kickstands"]),
                (Leaves, &["leaves"]),
                (Knots, &["knots"]),
            ],
        ),
        SupportTypeId::Leaf => ("leaf", &[(Knots, &["knot"])]),
        SupportTypeId::Twig => ("twig", &[(Knots, &["knots"]), (Leaves, &["leaves"])]),
        SupportTypeId::Stick => ("stick", &[(Knots, &["knots"]), (Leaves, &["leaves"])]),
        SupportTypeId::Brace => ("brace", &[(Knots, &["startKnot", "endKnot"])]),
        SupportTypeId::Stump => ("stump", &[(Knots, &["knots"]), (Leaves, &["leaves"])]),
        SupportTypeId::Kickstand => (
            "kickstand",
            &[
                (Roots, &["roots"]),
                (Knots, &["knots"]),
                (Braces, &["braces"]),
                (Leaves, &["leaves"]),
                (Branches, &["branches"]),
                (Kickstands, &["kickstands"]),
            ],
        ),
    };
    SupportRemovalShape {
        self_field,
        cascade,
    }
}

/// Type id -> the collection its entities live in.
pub const fn support_type_collection(type_id: SupportTypeId) -> SupportCollectionKey {
    match type_id {
        SupportTypeId::Trunk => Key::Trunks,
        SupportTypeId::Branch => Key::Branches,
        SupportTypeId::Leaf => Key::Leaves,
        SupportTypeId::Twig => Key::Twigs,
        SupportTypeId::Stick => Key::Sticks,
        SupportTypeId::Brace => Key::Braces,
        SupportTypeId::Stump => Key::Stumps,
        SupportTypeId::Kickstand => Key::Kickstands,
    }
}

/// Types whose `registerKnotDiameterRule` is filled: a twig tapers along its
/// length, so a knot on one is sized from that taper (`twigRegistration.ts`).
pub const KNOT_DIAMETER_RULE_TYPES: &[SupportTypeId] = &[SupportTypeId::Twig];

/// Types that registered their own settings inference (`registerSettingsInference`),
/// in registration order. Every editable type also takes the generic rule in `state.ts`.
pub const SETTINGS_INFERENCE_TYPES: &[SupportTypeId] = &[
    SupportTypeId::Branch,
    SupportTypeId::Leaf,
    SupportTypeId::Trunk,
];

/// Types that registered a lateral stabiliser generator (`kickstandRegistration.ts`).
pub const LATERAL_STABILISER_REGISTRATIONS: &[SupportTypeId] = &[SupportTypeId::Kickstand];

/// Types that registered a contact bridge builder (`twigRegistration.ts`, `stickRegistration.ts`).
pub const CONTACT_BRIDGE_REGISTRATIONS: &[SupportTypeId] =
    &[SupportTypeId::Stick, SupportTypeId::Twig];

/// Types that registered a contact override (`stumpAutoPlacement.ts`).
pub const CONTACT_OVERRIDE_REGISTRATIONS: &[SupportTypeId] = &[SupportTypeId::Stump];

/// Whether a type has a registered knot diameter rule; the rule itself is the
/// type's own (`support_types::twig`). Null means the generic segment rule.
pub fn has_knot_diameter_rule(type_id: SupportTypeId) -> bool {
    KNOT_DIAMETER_RULE_TYPES.contains(&type_id)
}

/// Whether a type registered its own settings inference, rather than taking the generic one.
pub fn has_settings_inference(type_id: SupportTypeId) -> bool {
    SETTINGS_INFERENCE_TYPES.contains(&type_id)
}

/// One contact a type carries: which end, which primitive, which field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContactEndpoint {
    pub end: EndpointEnd,
    pub kind: ContactKind,
    pub field: &'static str,
}

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

/// The contact primitives this type carries, lower end first, each with its end
/// and kind -- so a caller never infers either from the field name.
pub fn contact_endpoints_for(type_id: SupportTypeId) -> Vec<ContactEndpoint> {
    let descriptor = get_support_type_descriptor(type_id);
    let mut contacts = Vec::new();
    for (end, endpoint) in [
        (EndpointEnd::Lower, descriptor.lower),
        (EndpointEnd::Upper, descriptor.upper),
    ] {
        let kind = match endpoint.kind {
            End::Cone => ContactKind::Cone,
            End::Disk => ContactKind::Disk,
            _ => continue,
        };
        if let Some(field) = endpoint.field {
            contacts.push(ContactEndpoint { end, kind, field });
        }
    }
    contacts
}

/// Resolves a threshold, reading the named setting when there is one.
fn threshold_mm(
    threshold: Option<SupportPlacementThreshold>,
    read_setting: &dyn Fn(SupportPlacementSettingPath) -> Option<f64>,
) -> Option<f64> {
    match threshold? {
        SupportPlacementThreshold::Mm(mm) => Some(mm),
        SupportPlacementThreshold::Setting { setting, fallback } => {
            Some(read_setting(setting).unwrap_or(fallback))
        }
    }
}

/// Whether a bridge of `type_id` may land where the search ended up. The search
/// runs near radii first; a type declaring `may_reach_sideways` searches wider,
/// and landing beyond the near cutoff is a lateral prop.
pub fn bridge_may_land_sideways(
    type_id: SupportTypeId,
    dist_mm: f64,
    cutoff_mm: f64,
    reached_sideways: bool,
) -> bool {
    if !reached_sideways {
        return true;
    }
    if dist_mm <= cutoff_mm {
        return true;
    }
    get_support_type_descriptor(type_id).may_reach_sideways
}

/// Which type serves a measurement, or `None` when none declares a range for it.
///
/// Bounds are half-open, so adjacent types meet without overlapping and the
/// answer is unambiguous. `read_setting` is DragonFruit's default reader over
/// the live settings (`readPlacementSetting`), passed in by the caller.
pub fn select_type_for_placement(
    metric: SupportPlacementMetric,
    value_mm: f64,
    read_setting: &dyn Fn(SupportPlacementSettingPath) -> Option<f64>,
) -> Option<SupportTypeId> {
    // A NaN measurement satisfies no comparison, so an unbounded side would
    // otherwise let it through.
    if !value_mm.is_finite() {
        return None;
    }

    for descriptor in &SUPPORT_TYPES {
        let Some(rule) = descriptor.placement_rule else {
            continue;
        };
        if rule.metric != metric {
            continue;
        }

        let min = threshold_mm(rule.min_mm, read_setting);
        let max = threshold_mm(rule.max_mm, read_setting);
        let boundary_owner = rule.boundary.unwrap_or_default();
        if let Some(min) = min {
            let below = match boundary_owner {
                PlacementBoundary::Lower => value_mm <= min,
                PlacementBoundary::Upper => value_mm < min,
            };
            if below {
                continue;
            }
        }
        if let Some(max) = max {
            let above = match boundary_owner {
                PlacementBoundary::Lower => value_mm > max,
                PlacementBoundary::Upper => value_mm >= max,
            };
            if above {
                continue;
            }
        }
        return Some(descriptor.id);
    }
    None
}

/// Every type declaring a rule for this metric, in registry order.
pub fn types_for_placement_metric(
    metric: SupportPlacementMetric,
) -> Vec<&'static SupportTypeDescriptor> {
    SUPPORT_TYPES
        .iter()
        .filter(|d| d.placement_rule.is_some_and(|rule| rule.metric == metric))
        .collect()
}

/// One type's auto-placement policy, empty when it declares none -- so a caller
/// reads a field and falls back without testing the type by name.
pub fn auto_placement_for(type_id: SupportTypeId) -> SupportAutoPlacement {
    get_support_type_descriptor(type_id)
        .auto_placement
        .unwrap_or_default()
}

/// Every type declaring an auto-placement policy, in registry order.
pub fn types_with_auto_placement() -> Vec<SupportTypeId> {
    SUPPORT_TYPES
        .iter()
        .filter(|d| d.auto_placement.is_some())
        .map(|d| d.id)
        .collect()
}

/// The order to consult live placement previews in for one question, given
/// which placement modes are active.
///
/// A type whose own mode is active can rank earlier -- the preview the user is
/// currently steering answers first. A type may appear twice for that reason,
/// which the originals did too; the caller takes the first with an answer, so a
/// repeat is harmless.
pub fn preview_types_by_priority(
    purpose: SupportPreviewPurpose,
    active_modes: &dyn Fn(SupportTypeId) -> bool,
) -> Vec<SupportTypeId> {
    let mut entries: Vec<(SupportTypeId, f64)> = Vec::new();

    for descriptor in &SUPPORT_TYPES {
        let Some(priority) = descriptor.preview_priority.and_then(|p| p.get(purpose)) else {
            continue;
        };

        let active = active_modes(descriptor.id);
        if priority.only_while_active && !active {
            continue;
        }

        if active && let Some(while_active) = priority.while_active {
            entries.push((descriptor.id, while_active));
        }
        entries.push((descriptor.id, priority.rank));
    }

    // Array.prototype.sort is stable, as is sort_by.
    entries.sort_by(|a, b| a.1.total_cmp(&b.1));
    entries.into_iter().map(|(id, _)| id).collect()
}

/// Every registered stabiliser, in registry order.
pub fn lateral_stabiliser_types() -> Vec<SupportTypeId> {
    SUPPORT_TYPES
        .iter()
        .filter(|d| LATERAL_STABILISER_REGISTRATIONS.contains(&d.id))
        .map(|d| d.id)
        .collect()
}

/// Whether this type is a lateral stabiliser, generated beside a group.
pub fn is_lateral_stabiliser_type(type_id: SupportTypeId) -> bool {
    lateral_stabiliser_types().contains(&type_id)
}

/// The type whose brace snap target is its contact cone (leaf), if any.
pub fn brace_snap_cone_type() -> Option<SupportTypeId> {
    SUPPORT_TYPES
        .iter()
        .find(|d| d.hosts_brace_snap_cone)
        .map(|d| d.id)
}

/// Types whose shafts auto-bracing may brace to each other: `is_auto_braceable`
/// plus `has_segments`, less the lateral stabilisers, which the pass offers
/// beside a group rather than bracing as members of one.
pub fn auto_braceable_shaft_types() -> Vec<SupportTypeId> {
    let stabilisers = lateral_stabiliser_types();
    SUPPORT_TYPES
        .iter()
        .filter(|d| d.is_auto_braceable && d.has_segments && !stabilisers.contains(&d.id))
        .map(|d| d.id)
        .collect()
}

/// Whether this type's shafts may be braced to each other.
pub fn is_auto_braceable_shaft_type(type_id: SupportTypeId) -> bool {
    auto_braceable_shaft_types().contains(&type_id)
}

/// Every type that has registered a bridge builder, in registry order.
pub fn contact_bridge_types() -> Vec<SupportTypeId> {
    SUPPORT_TYPES
        .iter()
        .filter(|d| CONTACT_BRIDGE_REGISTRATIONS.contains(&d.id))
        .map(|d| d.id)
        .collect()
}

/// Types that claim a `tipHeight` band but registered no override. The default
/// tool is excluded, being what the fallback already builds; every other
/// claimant needs one, or the engine selects a type it cannot construct.
pub fn types_missing_contact_override() -> Vec<SupportTypeId> {
    let default_tool_id = default_placement_tool_type_id();
    SUPPORT_TYPES
        .iter()
        .filter(|d| {
            d.placement_rule
                .is_some_and(|rule| rule.metric == SupportPlacementMetric::TipHeight)
                && d.id != default_tool_id
        })
        .filter(|d| !CONTACT_OVERRIDE_REGISTRATIONS.contains(&d.id))
        .map(|d| d.id)
        .collect()
}

/// Position-bearing fields a model transform must move, beyond the segments and
/// contact fields every shafted type shares.
///
/// Declared because they are the only per-type difference in the transform's
/// apply phase: a brace carries a bezier curve, an anchor its own root position
/// and joint. Everything else is derived from `has_segments` and `contact_fields`.
pub fn transform_extras_for(type_id: SupportTypeId) -> &'static [&'static str] {
    match type_id {
        SupportTypeId::Brace => &["curve"],
        SupportTypeId::Stump => &["rootPos", "joint"],
        _ => &[],
    }
}

/// Types whose entities have editable settings, and can be a sidebar target.
pub fn editable_support_types() -> Vec<&'static SupportTypeDescriptor> {
    SUPPORT_TYPES
        .iter()
        .filter(|d| d.has_editable_settings)
        .collect()
}

/// The order the sidebar offers a type's own panel in. Observable, and not
/// registry order: `panelForTab` opens the first panel declaring a tab. Which
/// types are offered is `offers_sidebar_panel`, held to this list by
/// `sidebar_panel_order_drift`.
pub const SIDEBAR_PANEL_TYPE_ORDER: [SupportTypeId; 5] = [
    SupportTypeId::Trunk,
    SupportTypeId::Leaf,
    SupportTypeId::Branch,
    SupportTypeId::Twig,
    SupportTypeId::Stick,
];

/// How `SIDEBAR_PANEL_TYPE_ORDER` and the `offers_sidebar_panel` flag disagree.
pub fn sidebar_panel_order_drift() -> Vec<String> {
    let derived: Vec<SupportTypeId> = SUPPORT_TYPES
        .iter()
        .filter(|d| d.offers_sidebar_panel)
        .map(|d| d.id)
        .collect();
    let declared = &SIDEBAR_PANEL_TYPE_ORDER;
    let mut drift = Vec::new();
    for id in &derived {
        if !declared.contains(id) {
            drift.push(format!(
                "{id} offers a sidebar panel but is missing from SIDEBAR_PANEL_TYPE_ORDER"
            ));
        }
    }
    for id in declared {
        if !derived.contains(id) {
            drift.push(format!(
                "{id} is in SIDEBAR_PANEL_TYPE_ORDER but does not offer a sidebar panel"
            ));
        }
    }
    if declared.len() != derived.len() {
        let mut repeated: Vec<String> = Vec::new();
        for (index, id) in declared.iter().enumerate() {
            if declared.iter().position(|other| other == id) != Some(index)
                && !repeated.contains(&id.to_string())
            {
                repeated.push(id.to_string());
            }
        }
        drift.push(format!(
            "SIDEBAR_PANEL_TYPE_ORDER lists {} more than once",
            repeated.join(", ")
        ));
    }
    drift
}

/// Whether `id` names a type with editable settings.
pub fn is_editable_support_type(id: &str) -> bool {
    SupportTypeId::parse(id).is_some_and(|t| get_support_type_descriptor(t).has_editable_settings)
}

/// Where an auto-placed support came from, and what that implies.
///
/// `convertible_to_tree` gates trunk-to-tree conversion: anchors sit near the
/// plate and island trunks carry their own geometry, so neither converts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SupportOriginId {
    Stump,
    Overhang,
    Island,
    Standalone,
    /// A mesh minima's reinforcement ring: one to six contacts 2.5mm apart,
    /// the closest neighbours in the whole forest. Converting them onto the
    /// pillar they ring turns a crown of pillars into one plate contact.
    Reinforcement,
}

impl SupportOriginId {
    /// `SUPPORT_ORIGINS` key order.
    pub const ALL: [SupportOriginId; 5] = [
        SupportOriginId::Stump,
        SupportOriginId::Overhang,
        SupportOriginId::Island,
        SupportOriginId::Standalone,
        SupportOriginId::Reinforcement,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            SupportOriginId::Stump => "stump",
            SupportOriginId::Overhang => "overhang",
            SupportOriginId::Island => "island",
            SupportOriginId::Standalone => "standalone",
            SupportOriginId::Reinforcement => "reinforcement",
        }
    }

    pub const fn convertible_to_tree(self) -> bool {
        match self {
            SupportOriginId::Stump | SupportOriginId::Island => false,
            SupportOriginId::Overhang
            | SupportOriginId::Standalone
            | SupportOriginId::Reinforcement => true,
        }
    }
}

/// The origin a support is stamped with when placed in the near-plate band. The
/// band is claimed by a type through its `placement_rule`, and the origin is named
/// after that type.
///
/// Origin keys are their own vocabulary and do not follow `SupportTypeId`, so
/// callers compare against this rather than spelling the name.
pub fn near_plate_origin() -> SupportOriginId {
    let matches: Vec<SupportOriginId> = SupportOriginId::ALL
        .into_iter()
        .filter(|origin| {
            SUPPORT_TYPES
                .iter()
                .any(|d| d.id.as_str() == origin.as_str())
        })
        .collect();
    match matches.as_slice() {
        [origin] => *origin,
        _ => panic!("expected exactly one origin named after a support type, found: {matches:?}."),
    }
}

/// Whether a trunk with this origin may be converted into a tree.
pub fn is_origin_convertible_to_tree(origin: Option<SupportOriginId>) -> bool {
    origin.is_some_and(SupportOriginId::convertible_to_tree)
}

/// Ids of every Roots entry some entity still claims.
///
/// A root outlives the entity that made it unless something culls it, so callers
/// need the live set. Derived from `owns_root` rather than named types: a new
/// root-owning type would otherwise have its roots collected as garbage.
pub fn collect_owned_root_ids(collections: &SupportCollections) -> indexmap::IndexSet<String> {
    let mut owned = indexmap::IndexSet::new();
    for descriptor in &SUPPORT_TYPES {
        if !descriptor.owns_root {
            continue;
        }
        match descriptor.location.key() {
            Key::Trunks => owned.extend(
                collections
                    .trunks
                    .values()
                    .map(|t| t.root_id.clone())
                    .filter(|id| !id.is_empty()),
            ),
            Key::Kickstands => owned.extend(
                collections
                    .kickstands
                    .values()
                    .map(|k| k.root_id.clone())
                    .filter(|id| !id.is_empty()),
            ),
            other => unreachable!("{other:?} declares ownsRoot but carries no rootId field"),
        }
    }
    owned
}

pub fn get_support_type_by_selection_category(
    category: Option<SupportSelectionCategory>,
) -> Option<&'static SupportTypeDescriptor> {
    let category = category?;
    SUPPORT_TYPES
        .iter()
        .find(|d| d.selection_category == category)
}

/// The placement surface an entity contacts, from whichever contact field is set.
/// `surfaces` yields each declared contact field's `placementSurface`, in
/// `contact_fields` order. Braces and kickstands declare no contact fields and
/// always return `None`.
pub fn get_placement_surface(
    descriptor: &SupportTypeDescriptor,
    surface_of: &dyn Fn(&str) -> Option<PlacementSurface>,
) -> Option<PlacementSurface> {
    descriptor
        .contact_fields
        .iter()
        .find_map(|field| surface_of(field))
}

/// Collections whose entities have real shafts, for segment and joint walks.
pub fn shafted_collection_keys() -> Vec<SupportCollectionKey> {
    SUPPORT_TYPES
        .iter()
        .filter(|d| d.has_segments)
        .map(|d| d.location.key())
        .collect()
}

/// Types held in SupportState, in registry order.
///
/// Every type, while there is one store. Kept as its own name because the
/// question is "what does the store hold", not "what types exist".
pub fn support_state_types() -> Vec<&'static SupportTypeDescriptor> {
    SUPPORT_TYPES
        .iter()
        .filter(|d| matches!(d.location, SupportCollectionLocation::Support { .. }))
        .collect()
}

/// Types whose shafts the fan host pool offers, in registry order.
///
/// The pool, the merge search, the attachment-capacity check and the forest
/// report's host index all walk this one list, so extending `can_be_grid_host`
/// to a second type reaches every one of them.
pub fn grid_host_types() -> Vec<&'static SupportTypeDescriptor> {
    SUPPORT_TYPES
        .iter()
        .filter(|d| d.can_be_grid_host)
        .collect()
}

/// Every type the auto-placement pass can place, in registry order.
pub fn auto_placed_type_ids() -> Vec<SupportTypeId> {
    SUPPORT_TYPES
        .iter()
        .filter(|d| d.is_auto_placed)
        .map(|d| d.id)
        .collect()
}

/// Whether a placed kind is one the ledger reports. `None` is `'reject'`.
pub fn is_auto_placed_type(kind: Option<SupportTypeId>) -> bool {
    kind.is_some_and(|kind| get_support_type_descriptor(kind).is_auto_placed)
}

/// Collections whose entities can host a fan link, in registry order.
pub fn grid_host_collection_keys() -> Vec<SupportCollectionKey> {
    grid_host_types()
        .into_iter()
        .map(|d| d.location.key())
        .collect()
}

/// A collection on SupportState that is not a support type. Roots and knots are
/// primitives — they belong to a support rather than being one — but they are
/// still selectable and still need an empty collection at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupportPrimitiveCollection {
    pub key: SupportCollectionKey,
    pub selection_category: SupportSelectionCategory,
    /// How the primitive links to the rest of the graph.
    ///
    /// A knot's `parentShaftId` is the busiest edge there is -- nearly every
    /// cascade travels it -- so a walk that only reads SUPPORT_TYPES misses the
    /// majority of what a removal should take.
    pub edges: &'static [SupportEdge],
}

/// ADDING A SUPPORT TYPE: this list is not the place. Add a descriptor to
/// SUPPORT_TYPES above instead.
pub static SUPPORT_PRIMITIVE_COLLECTIONS: [SupportPrimitiveCollection; 2] = [
    SupportPrimitiveCollection {
        key: Key::Roots,
        selection_category: SupportSelectionCategory::Root,
        edges: &[],
    },
    SupportPrimitiveCollection {
        key: Key::Knots,
        selection_category: SupportSelectionCategory::Knot,
        edges: &[edge(
            "parentShaftId",
            SupportEdgeTarget::Segment,
            HostedBy,
            None,
        )],
    },
];

/// One collection in the dependency graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupportGraphNode {
    pub key: SupportCollectionKey,
    pub edges: &'static [SupportEdge],
    pub has_segments: bool,
}

/// Every collection in the dependency graph, types and primitives both.
pub fn support_graph_nodes() -> Vec<SupportGraphNode> {
    SUPPORT_TYPES
        .iter()
        .map(|d| SupportGraphNode {
            key: d.location.key(),
            edges: d.edges,
            has_segments: d.has_segments,
        })
        .chain(
            SUPPORT_PRIMITIVE_COLLECTIONS
                .iter()
                .map(|p| SupportGraphNode {
                    key: p.key,
                    edges: p.edges,
                    has_segments: false,
                }),
        )
        .collect()
}

/// Collections selection resolves by direct id lookup: roots, then support types.
pub fn support_state_collections() -> Vec<(SupportCollectionKey, SupportSelectionCategory)> {
    std::iter::once((Key::Roots, SupportSelectionCategory::Root))
        .chain(
            support_state_types()
                .into_iter()
                .map(|d| (d.location.key(), d.selection_category)),
        )
        .collect()
}

/// An empty collection per SupportState entity key, for initial and reset state.
pub fn create_empty_support_collections() -> SupportCollections {
    SupportCollections::default()
}

/// Entity collection keys on SupportState: primitives first, then support types.
pub fn support_collection_keys() -> Vec<SupportCollectionKey> {
    SUPPORT_PRIMITIVE_COLLECTIONS
        .iter()
        .map(|c| c.key)
        .chain(support_state_types().into_iter().map(|d| d.location.key()))
        .collect()
}

/// Categories that count as "a support is selected": every type, plus `root`.
///
/// Knots are excluded -- selecting one is selecting an attachment point, which
/// the multi-selection paths treat as not-a-support.
pub fn support_selection_categories() -> Vec<SupportSelectionCategory> {
    SUPPORT_TYPES
        .iter()
        .map(|d| d.selection_category)
        .chain(std::iter::once(SupportSelectionCategory::Root))
        .collect()
}

impl SupportCollections {
    /// How many entities a collection holds.
    pub fn len_of(&self, key: SupportCollectionKey) -> usize {
        match key {
            Key::Roots => self.roots.len(),
            Key::Trunks => self.trunks.len(),
            Key::Branches => self.branches.len(),
            Key::Leaves => self.leaves.len(),
            Key::Twigs => self.twigs.len(),
            Key::Sticks => self.sticks.len(),
            Key::Braces => self.braces.len(),
            Key::Stumps => self.stumps.len(),
            Key::Kickstands => self.kickstands.len(),
            Key::Knots => self.knots.len(),
        }
    }
}

/// Per-collection counts, for load diagnostics.
pub fn count_support_collections(snapshot: &SupportCollections) -> IndexMap<&'static str, usize> {
    support_collection_keys()
        .into_iter()
        .map(|key| (key.as_str(), snapshot.len_of(key)))
        .collect()
}

/// SupportState collection keys whose entities carry a modelId, in registry order.
///
/// Every support type does, and `roots` is the one primitive that does -- a
/// knot's model comes from its host shaft instead.
pub fn model_id_collection_keys() -> Vec<SupportCollectionKey> {
    std::iter::once(Key::Roots)
        .chain(support_state_types().into_iter().map(|d| d.location.key()))
        .collect()
}

/// The fields through which a type hangs off a host knot: its `HostedBy` edges
/// onto `knots`. Empty for a type that hangs off no knot -- one standing on its
/// own root, or spanning two model contacts.
pub fn host_knot_fields_for(type_id: SupportTypeId) -> Vec<&'static str> {
    get_support_type_descriptor(type_id)
        .edges
        .iter()
        .filter(|e| e.to == KNOTS && e.ownership == HostedBy)
        .map(|e| e.field)
        .collect()
}

/// Every type that flexes when its host knot is dragged, with the edge fields
/// naming that knot. Derived, so adding a flexing type needs no change here.
pub fn flexing_knot_host_types() -> Vec<(SupportTypeId, Vec<&'static str>)> {
    SUPPORT_TYPES
        .iter()
        .filter(|d| d.flexes_on_host_knot_drag)
        .map(|d| (d.id, host_knot_fields_for(d.id)))
        .collect()
}

/// Precedence `find_knot_host` resolves in when several types name one knot.
pub const KNOT_HOST_PRECEDENCE: [SupportTypeId; 4] = [
    SupportTypeId::Leaf,
    SupportTypeId::Branch,
    SupportTypeId::Brace,
    SupportTypeId::Kickstand,
];

/// The knot ids an entity hangs from through `field` (a `HostedBy` edge onto knots).
fn knot_field_value<'a>(
    collections: &'a SupportCollections,
    type_id: SupportTypeId,
    field: &str,
) -> Vec<(&'a str, &'a str)> {
    match (type_id, field) {
        (SupportTypeId::Branch, "parentKnotId") => collections
            .branches
            .values()
            .map(|e| (e.base.id.as_str(), e.parent_knot_id.as_str()))
            .collect(),
        (SupportTypeId::Leaf, "parentKnotId") => collections
            .leaves
            .values()
            .map(|e| (e.base.id.as_str(), e.parent_knot_id.as_str()))
            .collect(),
        (SupportTypeId::Brace, "startKnotId") => collections
            .braces
            .values()
            .map(|e| (e.base.id.as_str(), e.start_knot_id.as_str()))
            .collect(),
        (SupportTypeId::Brace, "endKnotId") => collections
            .braces
            .values()
            .map(|e| (e.base.id.as_str(), e.end_knot_id.as_str()))
            .collect(),
        (SupportTypeId::Kickstand, "hostKnotId") => collections
            .kickstands
            .values()
            .map(|e| (e.base.id.as_str(), e.host_knot_id.as_str()))
            .collect(),
        _ => unreachable!("{type_id} declares no knot field {field}"),
    }
}

/// The support hanging off a knot, from the declared `HostedBy` edges onto
/// `knots`. `order` decides which wins when several types name the same knot.
pub fn find_knot_host(
    state: &SupportCollections,
    knot_id: &str,
    order: &[SupportTypeId],
) -> Option<(SupportTypeId, String)> {
    for &type_id in order {
        let fields = host_knot_fields_for(type_id);
        if fields.is_empty() {
            continue;
        }
        // DragonFruit walks the entities, testing every field on each.
        let per_field: Vec<Vec<(&str, &str)>> = fields
            .iter()
            .map(|f| knot_field_value(state, type_id, f))
            .collect();
        let entity_count = per_field.first().map_or(0, Vec::len);
        for index in 0..entity_count {
            if per_field.iter().any(|values| values[index].1 == knot_id) {
                return Some((type_id, per_field[0][index].0.to_owned()));
            }
        }
    }
    None
}

/// The order to walk the types that hang off a host shaft by a knot. Observable,
/// and not registry order: the cull and the forest report both list leaves
/// before branches. Membership is held to the edge declarations by
/// `shaft_hosted_member_order_drift`.
pub const SHAFT_HOSTED_MEMBER_TYPE_ORDER: [SupportTypeId; 2] =
    [SupportTypeId::Leaf, SupportTypeId::Branch];

/// How `SHAFT_HOSTED_MEMBER_TYPE_ORDER` and the edge rule disagree, as messages.
pub fn shaft_hosted_member_order_drift() -> Vec<String> {
    // The rule: a shaft-hosted member hangs by a knot and by nothing else --
    // exactly one `hostedBy` edge onto `knots`, none onto a segment. Leaf and
    // branch. A brace names two knots, so it is not one; a kickstand names a
    // knot AND a segment, so it is not one either -- it rides a specific span.
    let derived: Vec<SupportTypeId> = SUPPORT_TYPES
        .iter()
        .filter(|d| {
            let hosted_by: Vec<&SupportEdge> =
                d.edges.iter().filter(|e| e.ownership == HostedBy).collect();
            hosted_by.iter().filter(|e| e.to == KNOTS).count() == 1
                && hosted_by
                    .iter()
                    .filter(|e| e.to == SupportEdgeTarget::Segment)
                    .count()
                    == 0
        })
        .map(|d| d.id)
        .collect();
    let declared = &SHAFT_HOSTED_MEMBER_TYPE_ORDER;
    let mut drift = Vec::new();
    for id in &derived {
        if !declared.contains(id) {
            drift.push(format!("{id} declares a shaft-hosted member but is missing from SHAFT_HOSTED_MEMBER_TYPE_ORDER"));
        }
    }
    for id in declared {
        if !derived.contains(id) {
            drift.push(format!(
                "{id} is in SHAFT_HOSTED_MEMBER_TYPE_ORDER but declares no shaft-hosted member"
            ));
        }
    }
    if declared.len() != derived.len() {
        drift.push("SHAFT_HOSTED_MEMBER_TYPE_ORDER lists a type more than once".to_owned());
    }
    drift
}

/// One shaft-hosted member type, as a walk over those members needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShaftHostedMemberType {
    /// The type id, which is also the `kind` a cull record for it carries.
    pub type_id: SupportTypeId,
    /// The entity field naming the knot this member hangs from.
    pub knot_field: &'static str,
    /// The SupportState collection its entities live in.
    pub collection_key: SupportCollectionKey,
}

/// The shaft-hosted member types, in the order to walk them.
pub fn shaft_hosted_member_types() -> Vec<ShaftHostedMemberType> {
    SHAFT_HOSTED_MEMBER_TYPE_ORDER
        .iter()
        .map(|&type_id| {
            let descriptor = get_support_type_descriptor(type_id);
            let knot_field = *host_knot_fields_for(type_id).first().unwrap_or_else(|| {
                panic!("{type_id} is walked as a shaft-hosted member but declares no hostedBy edge onto knots")
            });
            ShaftHostedMemberType { type_id, knot_field, collection_key: descriptor.location.key() }
        })
        .collect()
}

/// Which types own a placement mode of their own (`PLACEMENT_MODE_OWNER_BY_TYPE`).
pub const PLACEMENT_MODE_OWNER_TYPES: [SupportTypeId; 4] = [
    SupportTypeId::Branch,
    SupportTypeId::Leaf,
    SupportTypeId::Brace,
    SupportTypeId::Kickstand,
];

/// How `PLACEMENT_MODE_OWNER_TYPES` and the descriptor flags disagree.
pub fn placement_mode_owner_drift() -> Vec<String> {
    let derived: Vec<SupportTypeId> = SUPPORT_TYPES
        .iter()
        .filter(|d| d.has_placement_preview && !d.preview_yields_to_other_modes)
        .map(|d| d.id)
        .collect();
    let mut drift = Vec::new();
    for id in &derived {
        if !PLACEMENT_MODE_OWNER_TYPES.contains(id) {
            drift.push(format!(
                "{id} owns a placement mode but is missing from PLACEMENT_MODE_OWNER_BY_TYPE"
            ));
        }
    }
    for id in &PLACEMENT_MODE_OWNER_TYPES {
        if !derived.contains(id) {
            drift.push(format!(
                "{id} is marked as owning a placement mode but declares none"
            ));
        }
    }
    drift
}

/// The types sharing the ONE `branchFamily` placement binding.
///
/// Branch and brace are placed by the same binding, so the family they belong to
/// is named `branchFamily` rather than after either type.
pub const BRANCH_FAMILY_MEMBER_TYPES: [SupportTypeId; 2] =
    [SupportTypeId::Branch, SupportTypeId::Brace];

/// Whether each type's joint drags publish a live shaft preview (`JOINT_DRAG_PREVIEW_BY_TYPE`).
pub const JOINT_DRAG_PREVIEW_TYPES: [SupportTypeId; 4] = [
    SupportTypeId::Trunk,
    SupportTypeId::Branch,
    SupportTypeId::Brace,
    SupportTypeId::Kickstand,
];

/// Whether an untrusted `kind` off a preview event names such a type.
pub fn is_joint_drag_preview_type(kind: &str) -> bool {
    SupportTypeId::parse(kind).is_some_and(|t| JOINT_DRAG_PREVIEW_TYPES.contains(&t))
}

/// The types the placement router hands model-face gestures to (`MODEL_SURFACE_GESTURE_BY_TYPE`).
pub const MODEL_SURFACE_GESTURE_TYPES: [SupportTypeId; 2] =
    [SupportTypeId::Branch, SupportTypeId::Leaf];

/// How many segments one instance contributes when it carries none of its own.
///
/// A brace spans one implicit segment between its two knots, which is why it has
/// a `segment_selection_prefix` to select it by. A leaf's contact cone is a knot
/// host but not a segment, so `knot_host_prefix` cannot answer this.
pub fn implicit_segment_count(descriptor: &SupportTypeDescriptor) -> usize {
    if descriptor.has_segments {
        return 0;
    }
    usize::from(descriptor.segment_selection_prefix.is_some())
}

/// The name one of this type's groups carries in an exported mesh.
///
/// `Trunk_<id>`, `Kickstand_<id>` and so on: the singular, capitalised. These
/// strings land in exported 3MF and OBJ files, so a type's name reaching the
/// registry reaches the export too.
pub fn export_group_name(type_id: SupportTypeId, entity_id: &str) -> String {
    let singular = get_support_type_descriptor(type_id).singular;
    let mut chars = singular.chars();
    let first = chars
        .next()
        .map(|c| c.to_uppercase().collect::<String>())
        .unwrap_or_default();
    format!("{first}{}_{entity_id}", chars.as_str())
}

/// Split a knot's `parentShaftId` into the type it rides and that entity's id,
/// or `None` when it names a real shaft segment.
///
/// A knot rides either a real segment or a pseudo-shaft -- a leaf's contact
/// cone, a brace's span -- whose prefix each type declares. Callers ask rather
/// than spelling the prefix out, so renaming a type moves the string with it.
pub fn parse_knot_host_id(parent_shaft_id: &str) -> Option<(SupportTypeId, &str)> {
    for descriptor in &SUPPORT_TYPES {
        let Some(prefix) = descriptor.knot_host_prefix else {
            continue;
        };
        if let Some(entity_id) = parent_shaft_id.strip_prefix(prefix) {
            return Some((descriptor.id, entity_id));
        }
    }
    None
}

/// The selection id for an entity whose span stands in for a segment.
pub fn segment_selection_id(type_id: SupportTypeId, entity_id: &str) -> String {
    let prefix = get_support_type_descriptor(type_id)
        .segment_selection_prefix
        .unwrap_or_else(|| {
            panic!("{type_id} declares no segmentSelectionPrefix; its segments are real")
        });
    format!("{prefix}{entity_id}")
}

/// The primitive id for a type's inline root, which is geometry on the entity
/// rather than a `Roots` row, so it has no id of its own to select.
const INLINE_ROOT_SUFFIX: &str = ":root";

pub fn inline_root_id(entity_id: &str) -> String {
    format!("{entity_id}{INLINE_ROOT_SUFFIX}")
}

/// The entity owning an inline-root primitive id, or `None` for any other id.
pub fn parse_inline_root_id(primitive_id: &str) -> Option<&str> {
    primitive_id.strip_suffix(INLINE_ROOT_SUFFIX)
}

/// Split a segment selection id into its type and entity id, or `None` for a real segment.
pub fn parse_segment_selection_id(segment_id: &str) -> Option<(SupportTypeId, &str)> {
    for descriptor in &SUPPORT_TYPES {
        let Some(prefix) = descriptor.segment_selection_prefix else {
            continue;
        };
        if let Some(entity_id) = segment_id.strip_prefix(prefix) {
            return Some((descriptor.id, entity_id));
        }
    }
    None
}

/// The two kinds of pseudo-shaft a knot can ride, told apart by what they
/// declare rather than by name.
///
/// A SPAN is selectable as a segment, so it declares a `segment_selection_prefix`
/// too (a brace's span between its knots). A CONE host is a knot host only (a
/// leaf's contact cone), so it declares no segment prefix.
fn knot_host_types_where(wants_segment: bool) -> Vec<SupportTypeId> {
    SUPPORT_TYPES
        .iter()
        .filter(|d| {
            d.knot_host_prefix.is_some() && d.segment_selection_prefix.is_some() == wants_segment
        })
        .map(|d| d.id)
        .collect()
}

/// Types whose knot host is a cone, not a selectable span.
pub fn cone_knot_host_types() -> Vec<SupportTypeId> {
    knot_host_types_where(false)
}

/// Types whose knot host is a span that is also selectable as a segment.
pub fn span_knot_host_types() -> Vec<SupportTypeId> {
    knot_host_types_where(true)
}

/// Whether this type hosts knots on a cone rather than a span.
pub fn is_cone_knot_host(type_id: SupportTypeId) -> bool {
    cone_knot_host_types().contains(&type_id)
}

fn exactly_one(matches: Vec<SupportTypeId>, what: &str, hint: &str) -> SupportTypeId {
    match matches.as_slice() {
        [type_id] => *type_id,
        _ => panic!("expected exactly one {what}, found: {matches:?}.{hint}"),
    }
}

/// The single type whose knot host is a selectable span.
///
/// Callers that build such an id hold the entity but not its type; there is one
/// span host, and this asserts that rather than assuming it silently.
pub fn span_knot_host_type() -> SupportTypeId {
    exactly_one(
        span_knot_host_types(),
        "span knot host",
        " A caller builds these ids without holding a type; give it the type instead.",
    )
}

/// The single type whose knot host is a contact cone. Same contract as
/// `span_knot_host_type`: it asserts the "exactly one" rather than assuming it.
pub fn cone_knot_host_type() -> SupportTypeId {
    exactly_one(
        cone_knot_host_types(),
        "cone knot host",
        " A caller builds these ids without holding a type; give it the type instead.",
    )
}

/// The single type that is the default placement tool -- the one whose preview
/// yields to every other mode, and which auto-placement's fallback builds.
pub fn default_placement_tool_type_id() -> SupportTypeId {
    exactly_one(
        SUPPORT_TYPES
            .iter()
            .filter(|d| d.preview_yields_to_other_modes)
            .map(|d| d.id)
            .collect(),
        "default placement tool",
        "",
    )
}

/// The type whose entities live in `key`: the inverse of each descriptor's
/// `location.key`, for a caller walking a `SupportState` collection by key.
pub fn type_id_for_collection(key: SupportCollectionKey) -> SupportTypeId {
    SUPPORT_TYPES
        .iter()
        .find(|d| d.location.key() == key)
        .map(|d| d.id)
        .unwrap_or_else(|| {
            panic!(
                "no support type stores its entities in the \"{}\" collection",
                key.as_str()
            )
        })
}

/// The single type serialised as a bundle with the root and host knot it owns.
pub fn bundled_support_type_id() -> SupportTypeId {
    exactly_one(
        SUPPORT_TYPES
            .iter()
            .filter(|d| d.serialised_as_bundle)
            .map(|d| d.id)
            .collect(),
        "bundled support type",
        "",
    )
}

/// The types whose base is geometry on the entity rather than a shared `Roots`
/// record, and the fields that base position and radius live in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InlineRootPlacement {
    pub type_id: SupportTypeId,
    /// The collection the entities live in.
    pub collection_key: SupportCollectionKey,
    /// The field holding the base position.
    pub pos_field: &'static str,
    /// The field holding the base radius.
    pub radius_field: &'static str,
}

pub fn inline_root_types() -> Vec<InlineRootPlacement> {
    SUPPORT_TYPES
        .iter()
        .filter(|d| d.lower.kind == End::InlineRoot)
        .map(|d| {
            let (Some(pos_field), Some(radius_field)) = (d.lower.field, d.lower.radius_field)
            else {
                panic!(
                    "{} declares an inlineRoot without both a position and a radius field, \
                     so nothing can place its base on the raft",
                    d.id
                );
            };
            InlineRootPlacement {
                type_id: d.id,
                collection_key: d.location.key(),
                pos_field,
                radius_field,
            }
        })
        .collect()
}

/// Whether this type hosts knots on a selectable span.
pub fn is_span_knot_host(type_id: SupportTypeId) -> bool {
    span_knot_host_types().contains(&type_id)
}

/// Whether a `parentShaftId` names a pseudo-shaft rather than a real segment.
/// Asks every declared prefix, so a type gaining one is covered.
pub fn is_knot_host_id(parent_shaft_id: &str) -> bool {
    parse_knot_host_id(parent_shaft_id).is_some()
}

/// The `parentShaftId` a knot carries when it rides this type's pseudo-shaft.
pub fn knot_host_id(type_id: SupportTypeId, entity_id: &str) -> String {
    let prefix = get_support_type_descriptor(type_id)
        .knot_host_prefix
        .unwrap_or_else(|| {
            panic!("{type_id} declares no knotHostPrefix; its knots ride real segments")
        });
    format!("{prefix}{entity_id}")
}

/// Split a prefixed segment selection id into the type that owns it and the
/// entity id, or `None` when the id is a real segment id.
pub fn parse_prefixed_segment_id(segment_id: &str) -> Option<(SupportTypeId, &str)> {
    parse_segment_selection_id(segment_id)
}

/// Types that own their edit-history entry but declare no update action.
pub fn types_declaring_own_history_entry_without_update() -> Vec<SupportTypeId> {
    SUPPORT_TYPES
        .iter()
        .filter(|d| d.owns_edit_history_entry && d.history_update.is_none())
        .map(|d| d.id)
        .collect()
}

/// The types a kickstand's host knot may ride (`KICKSTAND_HOST_BY_TYPE`).
///
/// A kickstand's host knot rides a shaft, so the types that carry one are the
/// hosts: those with real segments, plus a brace, whose span is a selectable
/// pseudo-shaft. A leaf is the one knot host left out -- its pseudo-shaft is a
/// contact cone, not a span.
pub const KICKSTAND_HOST_TYPES: [SupportTypeId; 7] = [
    SupportTypeId::Trunk,
    SupportTypeId::Branch,
    SupportTypeId::Twig,
    SupportTypeId::Stick,
    SupportTypeId::Brace,
    SupportTypeId::Stump,
    SupportTypeId::Kickstand,
];

/// The types leaf sprouting may hang a knot on.
///
/// Sprouting walks each host's real `segments`, so this is the kickstand hosts
/// that declare `has_segments`. A brace hosts a kickstand on its selectable span
/// but carries no segments, so it is excluded here.
pub fn leaf_sprout_host_types() -> Vec<SupportTypeId> {
    KICKSTAND_HOST_TYPES
        .into_iter()
        .filter(|&id| get_support_type_descriptor(id).has_segments)
        .collect()
}

/// Whether an untrusted `kind` names a type a kickstand may host on.
pub fn is_kickstand_host_type(kind: &str) -> bool {
    SupportTypeId::parse(kind).is_some_and(|t| KICKSTAND_HOST_TYPES.contains(&t))
}

/// Whether one joint of this type can be removed on its own, keeping the support
/// (`JOINT_REMOVAL_BY_TYPE`).
///
/// Such a segment resolves an endpoint from a root, a host knot or its neighbour,
/// so a joint removal merges two segments.
pub const JOINT_REMOVAL_TYPES: [SupportTypeId; 3] = [
    SupportTypeId::Trunk,
    SupportTypeId::Branch,
    SupportTypeId::Kickstand,
];
