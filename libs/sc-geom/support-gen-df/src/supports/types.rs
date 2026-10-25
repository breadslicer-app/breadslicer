//! DragonFruit `src/supports/types.ts`.
//!
//! The support data model. Every record serializes with DragonFruit's own field
//! names, so a DragonFruit snapshot (`DragonfruitImportFormat`, oracle dumps)
//! reads straight into these types and back out unchanged.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::support_primitives::contact_cone::types::{ContactCone, ContactDiskProfile};
use super::support_type_registry::{SupportOriginId, SupportSelectionCategory, SupportTypeId};
use super::support_types::kickstand::types::KickstandBuildResult;
pub use crate::three::{Quaternion, Vec3};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SupportMode {
    Prepare,
    Analysis,
    Support,
    Export,
    Printing,
}

// --- Basic Math Types ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LimitationCode {
    AngleTooSteep,
    KnotAboveTip,
    StumpBelowRoot,
    CollisionWithModel,
    TooCloseToExisting,
    OutOfBounds,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WarningCode {
    AngleVerticalWarning,
    ShaftAngleTooFlat,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Transform {
    pub pos: Vec3,
    pub rot: Quaternion,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<Vec3>,
}

// --- Core Anatomy Entities ---

/// Who created a support.
///
/// Absent means a person placed it by hand. A tool that regenerates its own
/// output filters on this rather than on the support's type -- before it existed,
/// auto-bracing knew kickstands by an `autoBracingGenerated` flag but had no way
/// to tell a hand-drawn brace from one of its own, so it removed every brace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SupportGeneratedBy {
    AutoSupport,
    AutoBracing,
}

/// Where an auto-placed support came from. Declared in `support_type_registry`
/// alongside what each origin implies; re-exported here for entity records.
pub type SupportOrigin = SupportOriginId;

/// Base record for all top-level support entities.
/// Ensures every support element is linked to a specific model.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportEntity {
    pub id: String,
    /// The model this support belongs to
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings_code_hex: Option<String>,
    /// Set when a tool created this; absent when a person did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_by: Option<SupportGeneratedBy>,
    /// Auto-support origin, for debug origin colouring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<SupportOrigin>,
    /// Which support type this is.
    ///
    /// Optional only for files written before the field existed: those have it
    /// derived on load from the array they came out of. Everything the store
    /// hands out carries it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_id: Option<SupportTypeId>,
}

/// Roots: The anchor point on the build plate or raft.
/// It does NOT contain the vertical shaft (that's the Trunk).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Roots {
    #[serde(flatten)]
    pub base: SupportEntity,
    /// Position on the plate
    pub transform: Transform,
    /// Base diameter (bottom of cone)
    pub diameter: f64,
    /// Flat disk thickness
    pub disk_height: f64,
    /// Height of transition cone alone
    pub cone_height: f64,
    /// Legacy spelling of `coneHeight`, read by `rootTop` when the latter is absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KnotNormalizationHint {
    Preserve,
    Project,
    BraceImported,
}

/// Knot (Stump): A connection point on a Shaft.
/// Branches and Braces attach here.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Knot {
    pub id: String,
    /// The shaft this knot belongs to
    pub parent_shaft_id: String,
    /// 0-1 position along the shaft segment (preferred representation)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t: Option<f64>,
    /// World position on the host shaft
    pub pos: Vec3,
    /// Host shaft diameter + 0.1mm (computed at creation if absent)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diameter: Option<f64>,
    /// Persistent normalization intent that should survive save/load roundtrips.
    /// Used when imported support geometry needs deterministic preserve/project behavior
    /// instead of falling back to heuristics on subsequent reloads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normalization_hint: Option<KnotNormalizationHint>,
    /// Import-time hint stamped by converters (e.g. LYS).
    /// Consumed by normalizeLoadedKnotAndLeafGeometry and promoted into
    /// `normalizationHint` for roundtrip-stable persistence.
    /// 'preserve' -> keep authored pos; 'project' -> project to shaft geometry.
    /// Not present for runtime-created knots.
    #[serde(
        rename = "_importHint",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub import_hint: Option<KnotNormalizationHint>,
}

/// Joint: A spherical articulation point between shaft segments.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Joint {
    pub id: String,
    pub pos: Vec3,
    pub diameter: f64,
}

/// The cubic Bezier a `BezierSegment` (and a brace `curve`) carries.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BezierCurve {
    pub control_point1: Vec3,
    pub control_point2: Vec3,
    pub start_tangent: Vec3,
    pub end_tangent: Vec3,
    pub tension: f64,
    /// 0..1, 0.5 = balanced
    pub bias: f64,
    pub resolution: f64,
}

/// What a segment is between its joints: `StraightSegment | BezierSegment`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SegmentShape {
    /// `type?: 'straight'`; `explicit` records whether the tag was written.
    /// `stray_bias` is the `bias` DragonFruit leaves on a segment it
    /// de-curved (`removeSegmentCurve` does not strip it); nothing reads it,
    /// but it is part of the record DragonFruit writes.
    Straight {
        explicit: bool,
        stray_bias: Option<f64>,
    },
    Bezier(BezierCurve),
}

/// Segment: A section of a support (straight or curved).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawSegment", into = "RawSegment")]
pub struct Segment {
    pub id: String,
    pub diameter: f64,
    /// If null, it might be the tip
    pub top_joint: Option<Joint>,
    /// If null, it connects to Root or Knot
    pub bottom_joint: Option<Joint>,
    pub shape: SegmentShape,
}

impl Segment {
    /// A straight segment with no `type` tag, as most builders write them.
    pub fn straight(id: impl Into<String>, diameter: f64) -> Self {
        Segment {
            id: id.into(),
            diameter,
            top_joint: None,
            bottom_joint: None,
            shape: SegmentShape::Straight {
                explicit: false,
                stray_bias: None,
            },
        }
    }

    /// `segment.type === 'bezier'`.
    #[inline]
    pub fn is_bezier(&self) -> bool {
        matches!(self.shape, SegmentShape::Bezier(_))
    }

    #[inline]
    pub fn bezier(&self) -> Option<&BezierCurve> {
        match &self.shape {
            SegmentShape::Bezier(curve) => Some(curve),
            SegmentShape::Straight { .. } => None,
        }
    }

    #[inline]
    pub fn bezier_mut(&mut self) -> Option<&mut BezierCurve> {
        match &mut self.shape {
            SegmentShape::Bezier(curve) => Some(curve),
            SegmentShape::Straight { .. } => None,
        }
    }
}

/// The wire form of a segment: one flat record, `type` deciding the variant.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSegment {
    id: String,
    diameter: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    top_joint: Option<Joint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bottom_joint: Option<Joint>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    control_point1: Option<Vec3>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    control_point2: Option<Vec3>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start_tangent: Option<Vec3>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    end_tangent: Option<Vec3>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tension: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bias: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resolution: Option<f64>,
}

impl TryFrom<RawSegment> for Segment {
    type Error = String;

    fn try_from(raw: RawSegment) -> Result<Self, Self::Error> {
        let shape = match raw.kind.as_deref() {
            Some("bezier") => {
                let missing = |field: &str| format!("bezier segment {} has no {field}", raw.id);
                SegmentShape::Bezier(BezierCurve {
                    control_point1: raw.control_point1.ok_or_else(|| missing("controlPoint1"))?,
                    control_point2: raw.control_point2.ok_or_else(|| missing("controlPoint2"))?,
                    start_tangent: raw.start_tangent.ok_or_else(|| missing("startTangent"))?,
                    end_tangent: raw.end_tangent.ok_or_else(|| missing("endTangent"))?,
                    tension: raw.tension.ok_or_else(|| missing("tension"))?,
                    bias: raw.bias.ok_or_else(|| missing("bias"))?,
                    resolution: raw.resolution.ok_or_else(|| missing("resolution"))?,
                })
            }
            Some("straight") => SegmentShape::Straight {
                explicit: true,
                stray_bias: raw.bias,
            },
            None => SegmentShape::Straight {
                explicit: false,
                stray_bias: raw.bias,
            },
            Some(other) => return Err(format!("segment {} has unknown type {other:?}", raw.id)),
        };
        Ok(Segment {
            id: raw.id,
            diameter: raw.diameter,
            top_joint: raw.top_joint,
            bottom_joint: raw.bottom_joint,
            shape,
        })
    }
}

impl From<Segment> for RawSegment {
    fn from(segment: Segment) -> Self {
        let mut raw = RawSegment {
            id: segment.id,
            diameter: segment.diameter,
            top_joint: segment.top_joint,
            bottom_joint: segment.bottom_joint,
            kind: None,
            control_point1: None,
            control_point2: None,
            start_tangent: None,
            end_tangent: None,
            tension: None,
            bias: None,
            resolution: None,
        };
        match segment.shape {
            SegmentShape::Straight {
                explicit,
                stray_bias,
            } => {
                if explicit {
                    raw.kind = Some("straight".to_owned());
                }
                raw.bias = stray_bias;
            }
            SegmentShape::Bezier(curve) => {
                raw.kind = Some("bezier".to_owned());
                raw.control_point1 = Some(curve.control_point1);
                raw.control_point2 = Some(curve.control_point2);
                raw.start_tangent = Some(curve.start_tangent);
                raw.end_tangent = Some(curve.end_tangent);
                raw.tension = Some(curve.tension);
                raw.bias = Some(curve.bias);
                raw.resolution = Some(curve.resolution);
            }
        }
        raw
    }
}

/// Trunk: A vertical column extending from Roots.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trunk {
    #[serde(flatten)]
    pub base: SupportEntity,
    /// Link to the Roots anchor
    pub root_id: String,
    /// Baseline shaft diameter captured at creation/promotion
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_diameter_mm: Option<f64>,
    pub segments: Vec<Segment>,
    /// Terminal piece at model interface
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_cone: Option<ContactCone>,
}

/// Branch: A column extending from a Knot on another support.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Branch {
    #[serde(flatten)]
    pub base: SupportEntity,
    /// Link to the Knot on the parent
    pub parent_knot_id: String,
    pub segments: Vec<Segment>,
    /// Terminal piece at model interface
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_cone: Option<ContactCone>,
}

/// Leaf: A minimal model -> support connection.
/// Uses a contact tip on the model and a Knot on a host shaft.
/// No segments, no joints.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Leaf {
    #[serde(flatten)]
    pub base: SupportEntity,
    pub parent_knot_id: String,
    pub contact_cone: ContactCone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlacementSurface {
    Interior,
    Exterior,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactDisk {
    pub id: String,
    pub pos: Vec3,
    pub surface_normal: Vec3,
    pub cone_axis: Vec3,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_length_override: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement_surface: Option<PlacementSurface>,
    pub profile: ContactDiskProfile,
    pub contact_diameter_mm: f64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Twig {
    #[serde(flatten)]
    pub base: SupportEntity,
    pub segments: Vec<Segment>,
    pub contact_disk_a: ContactDisk,
    pub contact_disk_b: ContactDisk,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stick {
    #[serde(flatten)]
    pub base: SupportEntity,
    pub segments: Vec<Segment>,
    pub contact_cone_a: ContactCone,
    pub contact_cone_b: ContactCone,
}

/// The `type: 'bezier'` tag a brace curve always carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BezierTag {
    #[default]
    #[serde(rename = "bezier")]
    Bezier,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BraceCurve {
    #[serde(rename = "type")]
    pub kind: BezierTag,
    #[serde(flatten)]
    pub curve: BezierCurve,
}

/// Stump: A minimal near-plate support for contact points below 5mm.
/// Bypasses grid system entirely. Not a target for branches, leaves, or braces.
/// Geometry: frustum root -> joint -> single segment -> contact cone.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stump {
    #[serde(flatten)]
    pub base: SupportEntity,
    pub root_pos: Vec3,
    pub root_base_diameter: f64,
    pub root_top_diameter: f64,
    pub root_height: f64,
    pub joint: Joint,
    pub segments: Vec<Segment>,
    pub contact_cone: ContactCone,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct BraceProfile {
    pub diameter: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BraceDebugSection {
    Initial,
    Repeating,
}

/// Brace: A stabilizer bar connecting two supports.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Brace {
    #[serde(flatten)]
    pub base: SupportEntity,
    pub start_knot_id: String,
    pub end_knot_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement_surface: Option<PlacementSurface>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curve: Option<BraceCurve>,
    pub profile: BraceProfile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug_section: Option<BraceDebugSection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KickstandProfile {
    pub body_diameter_mm: f64,
    pub terminal_start_diameter_mm: f64,
    pub terminal_end_diameter_mm: f64,
}

/// Kickstand: A grounded column bracing a shaft it attaches to.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Kickstand {
    #[serde(flatten)]
    pub base: SupportEntity,
    pub root_id: String,
    pub host_knot_id: String,
    pub host_segment_id: String,
    pub host_min_t: f64,
    /// Deprecated: superseded by `generatedBy` on SupportEntity. Kept so scenes
    /// saved before the change still identify their auto-generated kickstands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_bracing_generated: Option<bool>,
    pub segments: Vec<Segment>,
    pub profile: KickstandProfile,
}

// --- Collection State ---

/// The interaction category under the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HoveredCategory {
    Model,
    Support,
    ContactDisk,
    Segment,
    Joint,
    Knot,
    Raft,
    Gizmo,
    None,
}

/// Every entity collection, keyed by id in insertion order (a JavaScript
/// object's key order, which DragonFruit's walks depend on).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SupportCollections {
    #[serde(default)]
    pub roots: IndexMap<String, Roots>,
    #[serde(default)]
    pub trunks: IndexMap<String, Trunk>,
    #[serde(default)]
    pub branches: IndexMap<String, Branch>,
    #[serde(default)]
    pub leaves: IndexMap<String, Leaf>,
    #[serde(default)]
    pub twigs: IndexMap<String, Twig>,
    #[serde(default)]
    pub sticks: IndexMap<String, Stick>,
    #[serde(default)]
    pub braces: IndexMap<String, Brace>,
    #[serde(default)]
    pub stumps: IndexMap<String, Stump>,
    #[serde(default)]
    pub kickstands: IndexMap<String, Kickstand>,
    #[serde(default)]
    pub knots: IndexMap<String, Knot>,
}

/// `SupportState`: the collections plus interaction state.
///
/// DragonFruit's `supports` field is a non-enumerable by-id view over the named
/// collections; it is derived, never stored, so it has no field here.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportState {
    #[serde(flatten)]
    pub collections: SupportCollections,
    // Interaction State
    pub selected_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_category: Option<SupportSelectionCategory>,
    pub hovered_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hovered_category: Option<HoveredCategory>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_warning: Option<WarningCode>,
}

// --- Import/Export Format ---

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DragonfruitImportMeta {
    pub source: String,
    pub object_center: Vec3,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DragonfruitImportFormat {
    pub version: f64,
    pub meta: DragonfruitImportMeta,
    pub roots: Vec<Roots>,
    pub trunks: Vec<Trunk>,
    pub branches: Vec<Branch>,
    pub leaves: Vec<Leaf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub twigs: Option<Vec<Twig>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sticks: Option<Vec<Stick>>,
    pub braces: Vec<Brace>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stumps: Option<Vec<Stump>>,
    pub knots: Vec<Knot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kickstands: Option<Vec<KickstandBuildResult>>,
}
