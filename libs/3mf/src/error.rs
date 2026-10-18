/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Typed errors for the package (OPC) and model (XML) layers.

use std::fmt;
use std::io;

/// Why a string is not a valid OPC part name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MalformedPartNameError {
    /// The name is empty.
    Empty,
    /// The name does not start with `/`.
    NotAbsolute,
    /// A path segment between 2 `/` is empty.
    EmptySegment,
    /// The final segment ends with a dot, which OPC forbids.
    TrailingDot,
}

impl fmt::Display for MalformedPartNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "part name is empty"),
            Self::NotAbsolute => write!(f, "part name must start with '/'"),
            Self::EmptySegment => write!(f, "part name contains an empty segment"),
            Self::TrailingDot => write!(f, "part name segment ends with a dot"),
        }
    }
}

impl std::error::Error for MalformedPartNameError {}

/// Why reading an OPC package failed.
#[derive(Debug)]
pub enum PackageReadError {
    /// The ZIP container itself could not be opened or traversed.
    Zip(zip::result::ZipError),
    /// Reading an entry's bytes failed.
    PartRead { part: String, source: io::Error },
    /// The mandatory `[Content_Types].xml` part is absent.
    MissingContentTypes,
    /// `[Content_Types].xml` is not parseable XML.
    ContentTypesXml(quick_xml::errors::Error),
    /// A content-type entry lacks a required attribute.
    ContentTypeMissingAttribute { attribute: &'static str },
    /// `[Content_Types].xml` has 2 Default entries for 1 extension, which OPC
    /// forbids (i.e.: `png` and `PNG` are 1 extension). `extension` is the
    /// Extension attribute of the 2nd entry.
    DuplicateDefaultExtension { extension: String },
    /// `[Content_Types].xml` has 2 Override entries for 1 part, which OPC
    /// forbids (i.e.: `/a.bin` and `/A.BIN` name 1 part). `part_name` is the
    /// PartName attribute of the 2nd entry.
    DuplicateOverridePartName { part_name: String },
    /// A relationships part is not parseable XML.
    RelationshipsXml {
        part: String,
        source: quick_xml::errors::Error,
    },
    /// A `<Relationship>` element lacks a required attribute.
    RelationshipMissingAttribute {
        part: String,
        attribute: &'static str,
    },
    /// A stored part has no content type, neither by extension default nor by
    /// override.
    PartWithoutContentType { part: String },
    /// No root relationship of the StartPart type exists, so the primary 3D
    /// Model part cannot be located.
    MissingStartPartRelationship,
    /// The StartPart relationship points at a part that is not in the package.
    StartPartNotFound { target: String },
    /// An entry name in the archive is not a valid part name.
    MalformedPartName {
        name: String,
        source: MalformedPartNameError,
    },
    /// A part's decompressed bytes exceed the per-part ceiling, so the package
    /// was refused.
    OversizedPart { part: String, cap_bytes: u64 },
    /// The decompressed bytes of all entries together exceed the package
    /// ceiling, so the package was refused.
    OversizedPackage { cap_bytes: u64 },
}

impl fmt::Display for PackageReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Zip(err) => write!(f, "could not read the package archive: {err}"),
            Self::PartRead { part, source } => {
                write!(f, "could not read part \"{part}\": {source}")
            }
            Self::MissingContentTypes => {
                write!(f, "package has no [Content_Types].xml part")
            }
            Self::ContentTypesXml(err) => {
                write!(f, "[Content_Types].xml is not valid XML: {err}")
            }
            Self::ContentTypeMissingAttribute { attribute } => write!(
                f,
                "a content-type entry is missing its \"{attribute}\" attribute",
            ),
            Self::DuplicateDefaultExtension { extension } => write!(
                f,
                "[Content_Types].xml repeats the Default entry for the extension \"{extension}\"",
            ),
            Self::DuplicateOverridePartName { part_name } => write!(
                f,
                "[Content_Types].xml repeats the Override entry for the part \"{part_name}\"",
            ),
            Self::RelationshipsXml { part, source } => {
                write!(
                    f,
                    "relationships part \"{part}\" is not valid XML: {source}"
                )
            }
            Self::RelationshipMissingAttribute { part, attribute } => write!(
                f,
                "a relationship in \"{part}\" is missing its \"{attribute}\" attribute",
            ),
            Self::PartWithoutContentType { part } => {
                write!(f, "part \"{part}\" has no content type")
            }
            Self::MissingStartPartRelationship => write!(
                f,
                "package has no StartPart relationship pointing at a 3D Model part",
            ),
            Self::StartPartNotFound { target } => write!(
                f,
                "the StartPart relationship points at \"{target}\", which is not in the package",
            ),
            Self::MalformedPartName { name, source } => {
                write!(f, "entry \"{name}\" is not a valid part name: {source}")
            }
            Self::OversizedPart { part, cap_bytes } => write!(
                f,
                "part \"{part}\" decompresses past the {cap_bytes}-byte per-part cap; the package was refused",
            ),
            Self::OversizedPackage { cap_bytes } => write!(
                f,
                "the package decompresses past the {cap_bytes}-byte total cap and was refused",
            ),
        }
    }
}

impl std::error::Error for PackageReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Zip(err) => Some(err),
            Self::PartRead { source, .. } => Some(source),
            Self::ContentTypesXml(err) => Some(err),
            Self::RelationshipsXml { source, .. } => Some(source),
            Self::MalformedPartName { source, .. } => Some(source),
            Self::ContentTypeMissingAttribute { .. }
            | Self::DuplicateDefaultExtension { .. }
            | Self::DuplicateOverridePartName { .. }
            | Self::MissingContentTypes
            | Self::RelationshipMissingAttribute { .. }
            | Self::PartWithoutContentType { .. }
            | Self::MissingStartPartRelationship
            | Self::StartPartNotFound { .. }
            | Self::OversizedPart { .. }
            | Self::OversizedPackage { .. } => None,
        }
    }
}

impl From<zip::result::ZipError> for PackageReadError {
    fn from(err: zip::result::ZipError) -> Self {
        Self::Zip(err)
    }
}

/// Why writing an OPC package failed.
#[derive(Debug)]
pub enum PackageWriteError {
    /// The ZIP container could not be written.
    Zip(zip::result::ZipError),
    /// An I/O failure while writing entry bytes or XML.
    Io(io::Error),
    /// 2 parts share 1 name.
    DuplicatePartName { name: String },
}

impl fmt::Display for PackageWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Zip(err) => write!(f, "could not write the package archive: {err}"),
            Self::Io(err) => write!(f, "could not write package data: {err}"),
            Self::DuplicatePartName { name } => {
                write!(f, "package already contains a part named \"{name}\"")
            }
        }
    }
}

impl std::error::Error for PackageWriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Zip(err) => Some(err),
            Self::Io(err) => Some(err),
            Self::DuplicatePartName { .. } => None,
        }
    }
}

impl From<zip::result::ZipError> for PackageWriteError {
    fn from(err: zip::result::ZipError) -> Self {
        Self::Zip(err)
    }
}

impl From<io::Error> for PackageWriteError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

/// Why parsing a 3D Model part failed.
#[derive(Debug)]
pub enum ModelParseError {
    /// The XML itself is unreadable.
    Xml(quick_xml::errors::Error),
    /// An attribute list is malformed.
    AttrSyntax(quick_xml::events::attributes::AttrError),
    /// The document contains DTD content, which the 3MF core specification
    /// forbids.
    DtdForbidden,
    /// The document references an entity that is neither predefined nor
    /// numeric. With DTDs forbidden it can never be declared.
    UndeclaredEntity { name: String },
    /// The root element is not a core-namespace `<model>`.
    UnexpectedRootElement { found: String },
    /// The document ends without a `<model>` element.
    MissingModelElement,
    /// A required attribute is absent.
    MissingRequiredAttribute {
        element: &'static str,
        attribute: &'static str,
    },
    /// The `unit` attribute value is not 1 of the 6 spec units.
    InvalidUnit { value: String },
    /// A numeric attribute did not parse.
    InvalidNumber {
        element: &'static str,
        attribute: &'static str,
        value: String,
    },
    /// A transform attribute is not 12 whitespace-separated numbers.
    InvalidMatrix { value: String },
    /// A resource id is not a positive 32-bit integer.
    InvalidResourceId { value: String },
    /// The value of an object `type` attribute is unknown.
    InvalidObjectType { value: String },
    /// 2 resources share 1 id.
    DuplicateResourceId { id: u32 },
    /// A component or build item references an object id that has not been
    /// defined before the reference (the spec requires definition prior to
    /// use).
    UnknownObjectReference { id: u32 },
    /// A triangle references a vertex index outside the mesh's vertex list.
    TriangleIndexOutOfRange {
        object_id: u32,
        index: u32,
        vertex_count: usize,
    },
    /// An object carries neither a mesh nor components.
    ObjectWithoutGeometry { id: u32 },
    /// An object carries more than 1 geometry element.
    DuplicateGeometry { id: u32 },
    /// A build item references an object of type "other", which the spec
    /// forbids.
    BuildItemReferencesOtherType { id: u32 },
    /// The model requires an extension this crate does not implement.
    UnsupportedRequiredExtension { namespace: String },
    /// A `requiredextensions` prefix has no matching xmlns declaration.
    UnknownRequiredExtensionPrefix { prefix: String },
    /// The document uses production-extension part references (`p:path`);
    /// loading it without that extension would silently produce an incomplete
    /// scene, so it is refused.
    ProductionPathUnsupported,
    /// A slice stack contains both inline slices and slicerefs, which the slice
    /// extension forbids.
    SliceStackMixesSlicesAndRefs { id: u32 },
    /// An object's `s:slicestackid` references a slice stack that has not been
    /// defined before the reference.
    UnknownSliceStackReference { object_id: u32, stack_id: u32 },
    /// The value of an `s:meshresolution` attribute is unknown.
    InvalidMeshResolution { value: String },
    /// A slice polygon carries no segments.
    PolygonWithoutSegments { stack_id: u32 },
    /// A slice polygon references a vertex index outside the slice's vertex
    /// list.
    SliceIndexOutOfRange {
        stack_id: u32,
        index: u32,
        vertex_count: usize,
    },
}

impl fmt::Display for ModelParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Xml(err) => write!(f, "model XML is unreadable: {err}"),
            Self::AttrSyntax(err) => write!(f, "model XML has a malformed attribute: {err}"),
            Self::DtdForbidden => write!(f, "model XML contains DTD content, which 3MF forbids",),
            Self::UndeclaredEntity { name } => {
                write!(f, "model XML references undeclared entity \"&{name};\"")
            }
            Self::UnexpectedRootElement { found } => write!(
                f,
                "expected a core-namespace <model> root element, found <{found}>",
            ),
            Self::MissingModelElement => {
                write!(f, "document contains no <model> element")
            }
            Self::MissingRequiredAttribute { element, attribute } => {
                write!(f, "<{element}> is missing its \"{attribute}\" attribute")
            }
            Self::InvalidUnit { value } => {
                write!(f, "\"{value}\" is not a valid model unit")
            }
            Self::InvalidNumber {
                element,
                attribute,
                value,
            } => write!(
                f,
                "<{element}> attribute \"{attribute}\" is not a number: \"{value}\"",
            ),
            Self::InvalidMatrix { value } => write!(
                f,
                "transform is not 12 whitespace-separated numbers: \"{value}\"",
            ),
            Self::InvalidResourceId { value } => write!(
                f,
                "\"{value}\" is not a valid resource id (positive 32-bit integer)",
            ),
            Self::InvalidObjectType { value } => {
                write!(f, "\"{value}\" is not a valid object type")
            }
            Self::DuplicateResourceId { id } => {
                write!(f, "resource id {id} is defined more than once")
            }
            Self::UnknownObjectReference { id } => write!(
                f,
                "reference to object id {id}, which is not defined at this point",
            ),
            Self::TriangleIndexOutOfRange {
                object_id,
                index,
                vertex_count,
            } => write!(
                f,
                "object {object_id} has a triangle referencing vertex {index}, but only {vertex_count} vertices exist",
            ),
            Self::ObjectWithoutGeometry { id } => {
                write!(f, "object {id} has neither a mesh nor components")
            }
            Self::DuplicateGeometry { id } => {
                write!(f, "object {id} has more than one geometry element")
            }
            Self::BuildItemReferencesOtherType { id } => write!(
                f,
                "build item references object {id} of type \"other\", which 3MF forbids",
            ),
            Self::UnsupportedRequiredExtension { namespace } => write!(
                f,
                "model requires the unsupported 3MF extension \"{namespace}\"",
            ),
            Self::UnknownRequiredExtensionPrefix { prefix } => write!(
                f,
                "requiredextensions names prefix \"{prefix}\" with no xmlns declaration",
            ),
            Self::ProductionPathUnsupported => write!(
                f,
                "model splits objects across parts via the production extension (p:path), which is not yet supported",
            ),
            Self::SliceStackMixesSlicesAndRefs { id } => write!(
                f,
                "slice stack {id} mixes inline slices with slicerefs, which the slice extension forbids",
            ),
            Self::UnknownSliceStackReference {
                object_id,
                stack_id,
            } => write!(
                f,
                "object {object_id} references slice stack {stack_id}, which is not defined at this point",
            ),
            Self::InvalidMeshResolution { value } => {
                write!(f, "\"{value}\" is not a valid mesh resolution")
            }
            Self::PolygonWithoutSegments { stack_id } => {
                write!(f, "slice stack {stack_id} has a polygon with no segments")
            }
            Self::SliceIndexOutOfRange {
                stack_id,
                index,
                vertex_count,
            } => write!(
                f,
                "slice stack {stack_id} references slice vertex {index}, but only {vertex_count} vertices exist",
            ),
        }
    }
}

impl std::error::Error for ModelParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Xml(err) => Some(err),
            Self::AttrSyntax(err) => Some(err),
            _ => None,
        }
    }
}

impl From<quick_xml::errors::Error> for ModelParseError {
    fn from(err: quick_xml::errors::Error) -> Self {
        Self::Xml(err)
    }
}

impl From<quick_xml::events::attributes::AttrError> for ModelParseError {
    fn from(err: quick_xml::events::attributes::AttrError) -> Self {
        Self::AttrSyntax(err)
    }
}

impl From<quick_xml::encoding::EncodingError> for ModelParseError {
    fn from(err: quick_xml::encoding::EncodingError) -> Self {
        Self::Xml(quick_xml::errors::Error::Encoding(err))
    }
}

/// Why serializing a 3D Model part failed: the output channel broke, or the
/// document violates a slice-extension invariant the writer must not put on
/// disk.
#[derive(Debug)]
pub enum ModelWriteError {
    Io(io::Error),
    /// Slice boundaries must strictly increase, starting above the stack's
    /// zbottom. `index` is the offending slice's position.
    SliceZNotIncreasing {
        stack_id: u32,
        index: usize,
    },
    /// A slice's ztop is NaN or infinite. Checked before the increasing-z
    /// comparison, which is false against NaN and would let a non-finite
    /// boundary through into the XML.
    SliceZTopNotFinite {
        stack_id: u32,
        index: usize,
    },
    /// A stack's zbottom is NaN or infinite. It seeds the increasing-z
    /// comparison, so a non-finite value would disable that check for the first
    /// slice.
    SliceZBottomNotFinite {
        stack_id: u32,
    },
    /// A slice polygon carries no segments.
    PolygonWithoutSegments {
        stack_id: u32,
    },
    /// A slice polygon references a vertex index outside the slice's vertex
    /// list.
    SliceIndexOutOfRange {
        stack_id: u32,
        index: u32,
        vertex_count: usize,
    },
    /// A stack referenced by a model or solidsupport object contains an open
    /// polygon; such objects require closed contours.
    OpenPolygonOnModelObject {
        object_id: u32,
        stack_id: u32,
    },
    /// An object's `slice_stack` id has no matching stack in the document.
    UnknownSliceStackReference {
        object_id: u32,
        stack_id: u32,
    },
    /// A transform applied (directly or through components) to a sliced object
    /// is not planar: the slice extension requires m02, m12, m20, m21 to equal
    /// 0 and m22 to equal 1.
    NonPlanarSlicedTransform {
        object_id: u32,
    },
}

impl fmt::Display for ModelWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "could not write model XML: {err}"),
            Self::SliceZNotIncreasing { stack_id, index } => write!(
                f,
                "slice stack {stack_id}: slice {index} does not increase the stack's z",
            ),
            Self::SliceZTopNotFinite { stack_id, index } => write!(
                f,
                "slice stack {stack_id}: slice {index} has a non-finite ztop",
            ),
            Self::SliceZBottomNotFinite { stack_id } => {
                write!(f, "slice stack {stack_id}: the zbottom is not finite",)
            }
            Self::PolygonWithoutSegments { stack_id } => {
                write!(f, "slice stack {stack_id} has a polygon with no segments")
            }
            Self::SliceIndexOutOfRange {
                stack_id,
                index,
                vertex_count,
            } => write!(
                f,
                "slice stack {stack_id} references slice vertex {index}, but only {vertex_count} vertices exist",
            ),
            Self::OpenPolygonOnModelObject {
                object_id,
                stack_id,
            } => write!(
                f,
                "object {object_id} requires closed slice contours, but stack {stack_id} has an open polygon",
            ),
            Self::UnknownSliceStackReference {
                object_id,
                stack_id,
            } => write!(
                f,
                "object {object_id} references slice stack {stack_id}, which the document does not define",
            ),
            Self::NonPlanarSlicedTransform { object_id } => write!(
                f,
                "a transform applied to sliced object {object_id} is not planar",
            ),
        }
    }
}

impl std::error::Error for ModelWriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::SliceZNotIncreasing { .. }
            | Self::SliceZTopNotFinite { .. }
            | Self::SliceZBottomNotFinite { .. }
            | Self::PolygonWithoutSegments { .. }
            | Self::SliceIndexOutOfRange { .. }
            | Self::OpenPolygonOnModelObject { .. }
            | Self::UnknownSliceStackReference { .. }
            | Self::NonPlanarSlicedTransform { .. } => None,
        }
    }
}

impl From<io::Error> for ModelWriteError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

#[cfg(test)]
mod display_tests {
    use super::*;

    #[test]
    fn part_name_errors_render_their_reason() {
        assert!(format!("{}", MalformedPartNameError::NotAbsolute).contains("'/'"));
        assert!(format!("{}", MalformedPartNameError::TrailingDot).contains("dot"));
    }

    #[test]
    fn package_read_errors_name_the_part() {
        let msg = format!(
            "{}",
            PackageReadError::PartWithoutContentType {
                part: "/3D/3dmodel.model".to_string(),
            },
        );
        assert!(msg.contains("/3D/3dmodel.model"), "msg: {msg}");
    }

    #[test]
    fn oversized_errors_name_the_subject_and_the_cap() {
        let msg = format!(
            "{}",
            PackageReadError::OversizedPart {
                part: "/3D/3dmodel.model".to_string(),
                cap_bytes: 1024,
            },
        );
        assert!(
            msg.contains("/3D/3dmodel.model") && msg.contains("1024"),
            "msg: {msg}"
        );

        let msg = format!("{}", PackageReadError::OversizedPackage { cap_bytes: 4096 });
        assert!(msg.contains("4096"), "msg: {msg}");
    }

    #[test]
    fn duplicate_media_type_errors_name_the_repeated_value() {
        let msg = format!(
            "{}",
            PackageReadError::DuplicateDefaultExtension {
                extension: "PNG".to_string(),
            },
        );
        assert!(msg.contains("\"PNG\""), "msg: {msg}");

        let msg = format!(
            "{}",
            PackageReadError::DuplicateOverridePartName {
                part_name: "/A.BIN".to_string(),
            },
        );
        assert!(msg.contains("\"/A.BIN\""), "msg: {msg}");
    }

    #[test]
    fn model_parse_errors_carry_context() {
        let msg = format!(
            "{}",
            ModelParseError::TriangleIndexOutOfRange {
                object_id: 7,
                index: 99,
                vertex_count: 8,
            },
        );
        assert!(
            msg.contains('7') && msg.contains("99") && msg.contains('8'),
            "msg: {msg}"
        );

        let msg = format!(
            "{}",
            ModelParseError::UnsupportedRequiredExtension {
                namespace: "http://example.com/ext".to_string(),
            },
        );
        assert!(msg.contains("http://example.com/ext"), "msg: {msg}");
    }

    #[test]
    fn production_path_error_mentions_the_extension() {
        let msg = format!("{}", ModelParseError::ProductionPathUnsupported);
        assert!(msg.contains("p:path"), "msg: {msg}");
    }
}
