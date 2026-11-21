//! The support model: a set's roots, shafts, tips, braces and knots, as the
//! stages build and edit them and as a project stores them.
//!
//! Each record reads and writes its fields in camelCase. Fields a record no
//! longer has are skipped when an older document is read, so every saved set
//! still opens.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::support_kinds::{SupportOriginId, SupportTypeId};
use super::support_primitives::contact_cone::types::{ContactCone, ContactDiskProfile};
pub use crate::geom::{Quaternion, Vec3};

/// Why a support could not be placed where it was asked.
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

/// A placement that is allowed but worth a warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WarningCode {
    AngleVerticalWarning,
    ShaftAngleTooFlat,
}

/// Where a root stands, and how it is turned.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Transform {
    pub pos: Vec3,
    pub rot: Quaternion,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<Vec3>,
}

/// Which pass made a support; none when it was placed by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SupportGeneratedBy {
    AutoSupport,
    AutoBracing,
}

/// Which rule placed a support.
pub type SupportOrigin = SupportOriginId;

/// What every support has: its id, its model, and how it came to be.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportEntity {
    pub id: String,
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_by: Option<SupportGeneratedBy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<SupportOrigin>,
    /// Its kind; taken from the collection it is in when a document lacks it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_id: Option<SupportTypeId>,
}

/// A support's foot on the plate: a disk, then a cone (or a join cone) up to
/// where its shaft starts.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Roots {
    #[serde(flatten)]
    pub base: SupportEntity,
    pub transform: Transform,
    /// The disk's width.
    pub diameter: f64,
    pub disk_height: f64,
    /// The cone's height over the disk.
    pub cone_height: f64,
    /// The cone's height in documents that named it so.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    /// A 45° cone this tall from the shaft out, instead of the cone (#360).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join_cone_mm: Option<f64>,
}

/// A point on a shaft others hang from: branches, leaves, braces and
/// kickstands.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Knot {
    pub id: String,
    /// The support whose shaft it is on.
    pub parent_shaft_id: String,
    /// How far along its segment, 0 to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t: Option<f64>,
    pub pos: Vec3,
    /// Its host shaft's width plus 0.1 mm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diameter: Option<f64>,
}

/// A ball where two segments of a shaft meet.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Joint {
    pub id: String,
    pub pos: Vec3,
    pub diameter: f64,
}

/// A cubic curve a segment or a brace follows between its ends.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BezierCurve {
    pub control_point1: Vec3,
    pub control_point2: Vec3,
    pub start_tangent: Vec3,
    pub end_tangent: Vec3,
    pub tension: f64,
    /// Where the curve leans, 0 to 1 (0.5: even).
    pub bias: f64,
    pub resolution: f64,
}

/// What a segment is between its ends.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SegmentShape {
    Straight,
    Bezier(BezierCurve),
}

/// A piece of a shaft, with the joints at its ends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "StoredSegment", into = "StoredSegment")]
pub struct Segment {
    pub id: String,
    pub diameter: f64,
    pub top_joint: Option<Joint>,
    pub bottom_joint: Option<Joint>,
    pub shape: SegmentShape,
}

impl Segment {
    pub fn straight(id: impl Into<String>, diameter: f64) -> Self {
        Segment {
            id: id.into(),
            diameter,
            top_joint: None,
            bottom_joint: None,
            shape: SegmentShape::Straight,
        }
    }

    #[inline]
    pub fn is_bezier(&self) -> bool {
        matches!(self.shape, SegmentShape::Bezier(_))
    }

    #[inline]
    pub fn bezier(&self) -> Option<&BezierCurve> {
        match &self.shape {
            SegmentShape::Bezier(curve) => Some(curve),
            SegmentShape::Straight => None,
        }
    }

    #[inline]
    pub fn bezier_mut(&mut self) -> Option<&mut BezierCurve> {
        match &mut self.shape {
            SegmentShape::Bezier(curve) => Some(curve),
            SegmentShape::Straight => None,
        }
    }
}

/// A segment as stored: one record, a curve's fields present when `type` is
/// `bezier`.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredSegment {
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

impl TryFrom<StoredSegment> for Segment {
    type Error = String;

    fn try_from(s: StoredSegment) -> Result<Self, Self::Error> {
        let shape = match s.kind.as_deref() {
            None | Some("straight") => SegmentShape::Straight,
            Some("bezier") => {
                let curve = (|| {
                    Some(BezierCurve {
                        control_point1: s.control_point1?,
                        control_point2: s.control_point2?,
                        start_tangent: s.start_tangent?,
                        end_tangent: s.end_tangent?,
                        tension: s.tension?,
                        bias: s.bias?,
                        resolution: s.resolution?,
                    })
                })();
                SegmentShape::Bezier(
                    curve.ok_or_else(|| format!("curved segment {} lacks its curve", s.id))?,
                )
            }
            Some(other) => return Err(format!("segment {} is of unknown type {other:?}", s.id)),
        };
        Ok(Segment {
            id: s.id,
            diameter: s.diameter,
            top_joint: s.top_joint,
            bottom_joint: s.bottom_joint,
            shape,
        })
    }
}

impl From<Segment> for StoredSegment {
    fn from(segment: Segment) -> Self {
        let curve = segment.bezier().copied();
        StoredSegment {
            id: segment.id,
            diameter: segment.diameter,
            top_joint: segment.top_joint,
            bottom_joint: segment.bottom_joint,
            kind: curve.map(|_| "bezier".to_owned()),
            control_point1: curve.map(|c| c.control_point1),
            control_point2: curve.map(|c| c.control_point2),
            start_tangent: curve.map(|c| c.start_tangent),
            end_tangent: curve.map(|c| c.end_tangent),
            tension: curve.map(|c| c.tension),
            bias: curve.map(|c| c.bias),
            resolution: curve.map(|c| c.resolution),
        }
    }
}

/// A shaft from a root on the plate up to the model.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trunk {
    #[serde(flatten)]
    pub base: SupportEntity,
    pub root_id: String,
    /// Its shaft's width when it was made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_diameter_mm: Option<f64>,
    pub segments: Vec<Segment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_cone: Option<ContactCone>,
}

/// A shaft from a knot on another shaft up to the model.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Branch {
    #[serde(flatten)]
    pub base: SupportEntity,
    pub parent_knot_id: String,
    pub segments: Vec<Segment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_cone: Option<ContactCone>,
}

/// A tip straight off a knot on another shaft, with no shaft of its own.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Leaf {
    #[serde(flatten)]
    pub base: SupportEntity,
    pub parent_knot_id: String,
    pub contact_cone: ContactCone,
}

/// Which side of the model's surface a support stands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlacementSurface {
    Interior,
    Exterior,
}

/// A flat contact on the model, a short nib off its surface.
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

/// A short shaft from the model to the model, a disk at each end.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Twig {
    #[serde(flatten)]
    pub base: SupportEntity,
    pub segments: Vec<Segment>,
    pub contact_disk_a: ContactDisk,
    pub contact_disk_b: ContactDisk,
}

/// A longer shaft from the model to the model, a cone at each end.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stick {
    #[serde(flatten)]
    pub base: SupportEntity,
    pub segments: Vec<Segment>,
    pub contact_cone_a: ContactCone,
    pub contact_cone_b: ContactCone,
}

/// The `bezier` tag a brace's curve carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BezierTag {
    #[default]
    #[serde(rename = "bezier")]
    Bezier,
}

/// The curve a brace follows.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BraceCurve {
    #[serde(rename = "type")]
    pub kind: BezierTag,
    #[serde(flatten)]
    pub curve: BezierCurve,
}

/// A short support near the plate: its own root, a joint, one segment and a
/// tip.
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

/// Which part of a run of braces a brace is: the first, or a repeat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BraceDebugSection {
    Initial,
    Repeating,
}

/// A shaft between knots on two other shafts.
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

/// A slanted shaft from its own root on the plate to a knot on another
/// shaft, holding it sideways.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Kickstand {
    #[serde(flatten)]
    pub base: SupportEntity,
    pub root_id: String,
    pub host_knot_id: String,
    pub host_segment_id: String,
    pub host_min_t: f64,
    pub segments: Vec<Segment>,
    pub profile: KickstandProfile,
}

/// A set's supports, each kind by id in the order they were made.
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

/// A set's supports, and the one picked when a pass runs (it keeps a picked
/// brace picked when it is redrawn).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportState {
    #[serde(flatten)]
    pub collections: SupportCollections,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_id: Option<String>,
}
