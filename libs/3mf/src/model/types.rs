/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The document model types. Everything is plain data with value semantics;
//! invariants beyond simple field validity (manifoldness, reference resolution
//! across a whole document) are enforced by the parser and by domain adapters,
//! not here.

use super::resource_ids::{ResourceId, ResourceIndex};
use super::slice::{MeshResolution, SliceStack};

/// The unit all lengths in a model document are expressed in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Unit {
    Micron,
    #[default]
    Millimeter,
    Centimeter,
    Inch,
    Foot,
    Meter,
}

impl Unit {
    /// Multiplier turning a document-space length into millimetres.
    pub fn scale_to_mm(self) -> f64 {
        match self {
            Self::Micron => 0.001,
            Self::Millimeter => 1.0,
            Self::Centimeter => 10.0,
            Self::Inch => 25.4,
            Self::Foot => 304.8,
            Self::Meter => 1000.0,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Micron => "micron",
            Self::Millimeter => "millimeter",
            Self::Centimeter => "centimeter",
            Self::Inch => "inch",
            Self::Foot => "foot",
            Self::Meter => "meter",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "micron" => Some(Self::Micron),
            "millimeter" => Some(Self::Millimeter),
            "centimeter" => Some(Self::Centimeter),
            "inch" => Some(Self::Inch),
            "foot" => Some(Self::Foot),
            "meter" => Some(Self::Meter),
            _ => None,
        }
    }
}

/// Row-major 4x3 affine transform as 3MF stores it: the 12 values
/// `m00 m01 m02 m10 m11 m12 m20 m21 m22 m30 m31 m32`, where the last triple is
/// the translation row (row-vector convention).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix3D(pub [f64; 12]);

impl Matrix3D {
    pub const IDENTITY: Self = Self([
        1.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, //
        0.0, 0.0, 1.0, //
        0.0, 0.0, 0.0,
    ]);

    /// Parse the attribute form: 12 whitespace-separated decimal numbers
    /// (scientific notation allowed; surrounding/collapsed whitespace
    /// tolerated). Every entry must be finite: ST_Number is a decimal pattern
    /// with no NaN or infinity spelling.
    pub fn parse(value: &str) -> Option<Self> {
        let mut values = [0.0_f64; 12];
        let mut count = 0;
        for token in value.split_whitespace() {
            if count == 12 {
                return None;
            }
            let number: f64 = token.parse().ok()?;
            if !number.is_finite() {
                return None;
            }
            values[count] = number;
            count += 1;
        }
        if count != 12 {
            return None;
        }
        Some(Self(values))
    }

    /// The attribute form: 12 numbers joined by single spaces, each in Rust's
    /// shortest-round-trip decimal form.
    pub fn to_attribute(self) -> String {
        let strings: Vec<String> = self.0.iter().map(|v| v.to_string()).collect();
        strings.join(" ")
    }
}

/// One `<metadata>` entry.
#[derive(Clone, Debug, PartialEq)]
pub struct Metadata {
    /// The (possibly namespace-prefixed) metadata name, kept verbatim.
    pub name: String,
    pub value: String,
    pub preserve: bool,
    /// The declared XML simple type, when present.
    pub value_type: Option<String>,
}

/// One `<base>` material inside a `<basematerials>` group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaseMaterial {
    pub name: String,
    /// sRGB color as the spec's `#RRGGBB` / `#RRGGBBAA` string.
    pub display_color: String,
}

/// A `<basematerials>` property group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaseMaterialsGroup {
    pub id: ResourceId,
    pub bases: Vec<BaseMaterial>,
}

/// Reference from an object to a property group entry (`pid` + `pindex`).
/// Carried opaquely; this crate does not interpret property semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PropertyRef {
    pub group: ResourceId,
    pub index: ResourceIndex,
}

/// A vertex position in document units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// A triangle as vertex-list indices, counter-clockwise from outside.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TriangleRef {
    pub v1: ResourceIndex,
    pub v2: ResourceIndex,
    pub v3: ResourceIndex,
}

/// The `<mesh>` payload of an object.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshData {
    pub vertices: Vec<Vertex3>,
    pub triangles: Vec<TriangleRef>,
}

/// A `<component>` reference inside a components object.
#[derive(Clone, Debug, PartialEq)]
pub struct Component {
    pub object_id: ResourceId,
    pub transform: Option<Matrix3D>,
}

/// The function of an object in the model.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ObjectType {
    #[default]
    Model,
    SolidSupport,
    Support,
    Surface,
    Other,
}

impl ObjectType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::SolidSupport => "solidsupport",
            Self::Support => "support",
            Self::Surface => "surface",
            Self::Other => "other",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "model" => Some(Self::Model),
            "solidsupport" => Some(Self::SolidSupport),
            "support" => Some(Self::Support),
            "surface" => Some(Self::Surface),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

/// What an object is made of: exactly 1 of a mesh or a component list.
#[derive(Clone, Debug, PartialEq)]
pub enum ObjectPayload {
    Mesh(MeshData),
    Components(Vec<Component>),
}

/// An `<object>` resource.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectResource {
    pub id: ResourceId,
    pub object_type: ObjectType,
    pub name: Option<String>,
    pub partnumber: Option<String>,
    pub property_ref: Option<PropertyRef>,
    /// `s:slicestackid`: the slice stack carrying this object's per-layer
    /// contours, when the document uses the slice extension.
    pub slice_stack: Option<ResourceId>,
    /// `s:meshresolution`: whether the mesh payload is authoritative geometry
    /// (fullres, the default) or a decimated stand-in.
    pub mesh_resolution: MeshResolution,
    pub payload: ObjectPayload,
}

/// A `<build>` `<item>`: one object to manufacture, with an optional placement
/// transform.
#[derive(Clone, Debug, PartialEq)]
pub struct BuildItem {
    pub object_id: ResourceId,
    pub transform: Option<Matrix3D>,
    pub partnumber: Option<String>,
}

/// A parsed 3D Model part.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelDocument {
    pub unit: Unit,
    pub metadata: Vec<Metadata>,
    pub base_materials: Vec<BaseMaterialsGroup>,
    pub slice_stacks: Vec<SliceStack>,
    pub objects: Vec<ObjectResource>,
    pub build: Vec<BuildItem>,
}

impl ModelDocument {
    /// Look an object up by id.
    pub fn object(&self, id: ResourceId) -> Option<&ObjectResource> {
        self.objects.iter().find(|o| o.id == id)
    }

    /// Look a slice stack up by id.
    pub fn slice_stack(&self, id: ResourceId) -> Option<&SliceStack> {
        self.slice_stacks.iter().find(|s| s.id == id)
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn unit_scales_match_the_spec_table() {
        assert_eq!(Unit::Micron.scale_to_mm(), 0.001);
        assert_eq!(Unit::Millimeter.scale_to_mm(), 1.0);
        assert_eq!(Unit::Centimeter.scale_to_mm(), 10.0);
        assert_eq!(Unit::Inch.scale_to_mm(), 25.4);
        assert_eq!(Unit::Foot.scale_to_mm(), 304.8);
        assert_eq!(Unit::Meter.scale_to_mm(), 1000.0);
    }

    #[test]
    fn unit_parse_round_trips_every_variant() {
        for unit in [
            Unit::Micron,
            Unit::Millimeter,
            Unit::Centimeter,
            Unit::Inch,
            Unit::Foot,
            Unit::Meter,
        ] {
            assert_eq!(Unit::parse(unit.as_str()), Some(unit));
        }
        assert_eq!(Unit::parse("furlong"), None);
    }

    #[test]
    fn resource_id_rejects_zero_and_out_of_range() {
        assert!(ResourceId::new(0).is_none());
        assert!(ResourceId::new(1).is_some());
        assert!(ResourceId::new(i32::MAX as u32).is_some());
        assert!(ResourceId::new(i32::MAX as u32 + 1).is_none());
    }

    #[test]
    fn resource_id_min_is_one() {
        assert_eq!(ResourceId::MIN.get(), 1);
        assert_eq!(Some(ResourceId::MIN), ResourceId::new(1));
    }

    #[test]
    fn matrix_parses_collapsed_whitespace_and_scientific_notation() {
        let m = Matrix3D::parse("  1 0 0\t0 1 0  0 0 1 1.5e1 -2 0.25 ").unwrap();
        assert_eq!(m.0[9], 15.0);
        assert_eq!(m.0[10], -2.0);
        assert_eq!(m.0[11], 0.25);
    }

    #[test]
    fn matrix_rejects_wrong_arity_and_garbage() {
        assert!(Matrix3D::parse("1 0 0 0 1 0 0 0 1 0 0").is_none());
        assert!(Matrix3D::parse("1 0 0 0 1 0 0 0 1 0 0 0 0").is_none());
        assert!(Matrix3D::parse("1 0 0 0 1 0 0 0 x 0 0 0").is_none());
    }

    #[test]
    fn matrix_rejects_non_finite_values() {
        // ST_Matrix3D entries are ST_Number, a decimal pattern that cannot
        // express NaN or an infinity.
        assert!(Matrix3D::parse("1 0 0 0 1 0 0 0 1 NaN 0 0").is_none());
        assert!(Matrix3D::parse("1 0 0 0 1 0 0 0 1 inf 0 0").is_none());
        assert!(Matrix3D::parse("1 0 0 0 1 0 0 0 1 -inf 0 0").is_none());
    }

    #[test]
    fn matrix_attribute_form_round_trips() {
        let m = Matrix3D([
            1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, -19.999, -62.998, 0.125,
        ]);
        let attr = m.to_attribute();
        assert_eq!(Matrix3D::parse(&attr), Some(m));
    }
}
