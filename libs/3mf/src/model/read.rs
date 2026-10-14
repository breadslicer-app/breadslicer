/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Streaming parser for the 3D Model part.
//!
//! Conformance behavior:
//! - DTD content is rejected (the spec forbids it).
//! - Slice-extension content (slicestack resources, the object attributes) is
//!   parsed into the document model.
//! - Elements and attributes from namespaces this crate does not know are
//!   ignored, as the spec instructs consumers to do, with 1 exception:
//!   production-extension `path` attributes are refused, because ignoring them
//!   would silently load an incomplete scene.
//! - `requiredextensions` prefixes are resolved against the xmlns declarations
//!   on `<model>` and checked against the crate's supported set; unsupported
//!   ones are refused by namespace name.
//! - Resources must be defined before they are referenced; resource ids must be
//!   unique; triangle indices are range-checked.
//!
//! Loop structure invariant used throughout: every child-parsing loop consumes
//! events until the End of ITS OWN container element. Leaf elements (`vertex`,
//! `base`, `item`, ...) are handled from their attributes and, when they arrive
//! in Start form, their subtree is skipped immediately, so an unmatched End
//! event always belongs to the enclosing container.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use quick_xml::XmlVersion;
use quick_xml::events::{BytesRef, BytesStart, Event};
use quick_xml::name::{PrefixDeclaration, QName, ResolveResult};
use quick_xml::reader::NsReader;

use crate::error::ModelParseError;
use crate::names::{CORE_NS, PRODUCTION_NS, SLICE_NS};

use super::element_run;
use super::numbers::{parse_f64, parse_index};
use super::resource_ids::{ResourceId, ResourceIndex};
use super::slice::{
    MeshResolution, Slice, SlicePolygon, SliceRef, SliceSegment, SliceStack, SliceStackContent,
    SliceVertex,
};
use super::types::{
    BaseMaterial, BaseMaterialsGroup, BuildItem, Component, Matrix3D, MeshData, Metadata,
    ModelDocument, ObjectPayload, ObjectResource, ObjectType, PropertyRef, TriangleRef, Unit,
    Vertex3,
};

/// Extension namespaces this parser fully understands. The core namespace is
/// implied and never listed in `requiredextensions`.
const SUPPORTED_EXTENSION_NAMESPACES: &[&str] = &[SLICE_NS];

type Reader<'a> = NsReader<&'a [u8]>;

/// Parse the XML of one 3D Model part into a [`ModelDocument`].
pub fn parse_model(bytes: &[u8]) -> Result<ModelDocument, ModelParseError> {
    let mut reader = NsReader::from_reader(bytes);
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::DocType(_) => return Err(ModelParseError::DtdForbidden),
            Event::Start(e) => {
                let owned = e.into_owned();
                return dispatch_root(&mut reader, &owned, false);
            }
            Event::Empty(e) => {
                let owned = e.into_owned();
                return dispatch_root(&mut reader, &owned, true);
            }
            Event::Eof => return Err(ModelParseError::MissingModelElement),
            _ => {}
        }
        buffer.clear();
    }
}

fn dispatch_root(
    reader: &mut Reader<'_>,
    root: &BytesStart<'static>,
    is_empty: bool,
) -> Result<ModelDocument, ModelParseError> {
    let (ns, local) = reader.resolver().resolve_element(root.name());
    if !(is_core_ns(&ns) && local.as_ref() == b"model") {
        return Err(ModelParseError::UnexpectedRootElement {
            found: String::from_utf8_lossy(root.name().as_ref()).into_owned(),
        });
    }
    parse_model_element(reader, root, is_empty)
}

fn is_core_ns(result: &ResolveResult<'_>) -> bool {
    matches!(result, ResolveResult::Bound(ns) if ns.as_ref() == CORE_NS.as_bytes())
}

fn is_production_ns(result: &ResolveResult<'_>) -> bool {
    matches!(result, ResolveResult::Bound(ns) if ns.as_ref() == PRODUCTION_NS.as_bytes())
}

fn is_slice_ns(result: &ResolveResult<'_>) -> bool {
    matches!(result, ResolveResult::Bound(ns) if ns.as_ref() == SLICE_NS.as_bytes())
}

/// The attributes of one element, split into locally-relevant ones
/// (unprefixed), slice-extension ones, and the xmlns declarations. Attributes
/// from other foreign namespaces are dropped here, after the production-path
/// guard.
struct ElementAttributes {
    local: BTreeMap<Vec<u8>, String>,
    slice: BTreeMap<Vec<u8>, String>,
    xmlns: BTreeMap<String, String>,
}

impl ElementAttributes {
    fn get(&self, name: &str) -> Option<&str> {
        self.local.get(name.as_bytes()).map(String::as_str)
    }

    fn get_slice(&self, name: &str) -> Option<&str> {
        self.slice.get(name.as_bytes()).map(String::as_str)
    }

    fn require(&self, element: &'static str, name: &'static str) -> Result<&str, ModelParseError> {
        self.get(name)
            .ok_or(ModelParseError::MissingRequiredAttribute {
                element,
                attribute: name,
            })
    }
}

fn collect_attributes(
    reader: &Reader<'_>,
    e: &BytesStart<'_>,
) -> Result<ElementAttributes, ModelParseError> {
    let mut local = BTreeMap::new();
    let mut slice = BTreeMap::new();
    let mut xmlns = BTreeMap::new();
    for attr in e.attributes() {
        let attr = attr?;
        if let Some(declaration) = attr.key.as_namespace_binding() {
            // 3MF mandates XML 1.0; the reader never checks the declaration, so
            // the 1.0 rules are assumed.
            let ns = attr.normalized_value(XmlVersion::Implicit1_0)?.into_owned();
            let prefix = match declaration {
                PrefixDeclaration::Default => String::new(),
                PrefixDeclaration::Named(p) => String::from_utf8_lossy(p).into_owned(),
            };
            xmlns.insert(prefix, ns);
            continue;
        }
        let (ns, local_name) = reader.resolver().resolve_attribute(attr.key);
        match ns {
            ResolveResult::Unbound => {
                local.insert(
                    local_name.as_ref().to_vec(),
                    attr.normalized_value(XmlVersion::Implicit1_0)?.into_owned(),
                );
            }
            _ if is_slice_ns(&ns) => {
                slice.insert(
                    local_name.as_ref().to_vec(),
                    attr.normalized_value(XmlVersion::Implicit1_0)?.into_owned(),
                );
            }
            _ if is_production_ns(&ns) && local_name.as_ref() == b"path" => {
                return Err(ModelParseError::ProductionPathUnsupported);
            }
            _ => {}
        }
    }
    Ok(ElementAttributes {
        local,
        slice,
        xmlns,
    })
}

fn parse_resource_id(value: &str) -> Result<ResourceId, ModelParseError> {
    value
        .trim()
        .parse::<u32>()
        .ok()
        .and_then(ResourceId::new)
        .ok_or_else(|| ModelParseError::InvalidResourceId {
            value: value.to_string(),
        })
}

fn parse_transform(value: &str) -> Result<Matrix3D, ModelParseError> {
    Matrix3D::parse(value).ok_or_else(|| ModelParseError::InvalidMatrix {
        value: value.to_string(),
    })
}

/// Skip an element subtree whose Start event was just read.
fn skip_subtree(reader: &mut Reader<'_>, start: &BytesStart<'_>) -> Result<(), ModelParseError> {
    let name = start.name().as_ref().to_vec();
    let mut buffer = Vec::new();
    reader.read_to_end_into(QName(&name), &mut buffer)?;
    Ok(())
}

fn parse_model_element(
    reader: &mut Reader<'_>,
    model: &BytesStart<'static>,
    is_empty: bool,
) -> Result<ModelDocument, ModelParseError> {
    let attributes = collect_attributes(reader, model)?;

    let unit = match attributes.get("unit") {
        None => Unit::default(),
        Some(value) => Unit::parse(value).ok_or_else(|| ModelParseError::InvalidUnit {
            value: value.to_string(),
        })?,
    };

    if let Some(required) = attributes.get("requiredextensions") {
        for prefix in required.split_whitespace() {
            let namespace = attributes.xmlns.get(prefix).ok_or_else(|| {
                ModelParseError::UnknownRequiredExtensionPrefix {
                    prefix: prefix.to_string(),
                }
            })?;
            let supported = namespace == CORE_NS
                || SUPPORTED_EXTENSION_NAMESPACES.contains(&namespace.as_str());
            if !supported {
                return Err(ModelParseError::UnsupportedRequiredExtension {
                    namespace: namespace.clone(),
                });
            }
        }
    }

    let mut document = ModelDocument {
        unit,
        ..ModelDocument::default()
    };
    if is_empty {
        return Ok(document);
    }

    let mut seen_ids: BTreeSet<u32> = BTreeSet::new();
    let mut declared = DeclaredObjects::default();
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::DocType(_) => return Err(ModelParseError::DtdForbidden),
            Event::Start(e) => {
                let owned = e.into_owned();
                let (ns, local) = reader.resolver().resolve_element(owned.name());
                if !is_core_ns(&ns) {
                    skip_subtree(reader, &owned)?;
                } else {
                    match local.as_ref() {
                        b"metadata" => {
                            document.metadata.push(parse_metadata(reader, &owned)?);
                        }
                        b"resources" => {
                            parse_resources(reader, &mut document, &mut seen_ids, &mut declared)?;
                        }
                        b"build" => parse_build(reader, &mut document, &declared)?,
                        _ => skip_subtree(reader, &owned)?,
                    }
                }
            }
            Event::Empty(e) => {
                let owned = e.into_owned();
                let (ns, local) = reader.resolver().resolve_element(owned.name());
                if is_core_ns(&ns) && local.as_ref() == b"metadata" {
                    let attributes = collect_attributes(reader, &owned)?;
                    document
                        .metadata
                        .push(metadata_from_attributes(&attributes, String::new())?);
                }
            }
            Event::End(_) => break,
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(document)
}

fn metadata_from_attributes(
    attributes: &ElementAttributes,
    value: String,
) -> Result<Metadata, ModelParseError> {
    Ok(Metadata {
        name: attributes.require("metadata", "name")?.to_string(),
        value,
        preserve: matches!(attributes.get("preserve"), Some("1" | "true")),
        value_type: attributes.get("type").map(str::to_string),
    })
}

/// Parse a `<metadata>` element's attributes and text content. Entity
/// references arrive as separate events and are resolved here; undeclared
/// entities are an error since DTDs are forbidden.
fn parse_metadata(
    reader: &mut Reader<'_>,
    start: &BytesStart<'static>,
) -> Result<Metadata, ModelParseError> {
    let attributes = collect_attributes(reader, start)?;
    let mut value = String::new();
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Text(t) => value.push_str(&t.decode()?),
            Event::CData(t) => value.push_str(&t.decode()?),
            Event::GeneralRef(r) => value.push_str(&resolve_general_ref(&r)?),
            Event::Start(e) => {
                let owned = e.into_owned();
                skip_subtree(reader, &owned)?;
            }
            Event::End(_) => break,
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    metadata_from_attributes(&attributes, value)
}

fn resolve_general_ref(r: &BytesRef<'_>) -> Result<String, ModelParseError> {
    if let Some(ch) = r.resolve_char_ref()? {
        return Ok(ch.to_string());
    }
    let name = r.decode()?;
    let resolved = match name.as_ref() {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        other => {
            return Err(ModelParseError::UndeclaredEntity {
                name: other.to_string(),
            });
        }
    };
    Ok(resolved.to_string())
}

/// The objects parsed so far, with the type each one declared.
///
/// An object joins this set only once its own element has been fully parsed,
/// which is what makes a reference to a later object, or to the enclosing
/// object itself, resolve to nothing.
#[derive(Default)]
struct DeclaredObjects(HashMap<ResourceId, ObjectType>);

impl DeclaredObjects {
    fn declare(&mut self, object: &ObjectResource) {
        self.0.insert(object.id, object.object_type);
    }

    /// `None` when no object with `id` has been declared yet.
    fn object_type(&self, id: ResourceId) -> Option<ObjectType> {
        self.0.get(&id).copied()
    }
}

fn register_id(seen_ids: &mut BTreeSet<u32>, id: ResourceId) -> Result<(), ModelParseError> {
    if !seen_ids.insert(id.get()) {
        return Err(ModelParseError::DuplicateResourceId { id: id.get() });
    }
    Ok(())
}

fn parse_resources(
    reader: &mut Reader<'_>,
    document: &mut ModelDocument,
    seen_ids: &mut BTreeSet<u32>,
    declared: &mut DeclaredObjects,
) -> Result<(), ModelParseError> {
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(e) => {
                let owned = e.into_owned();
                let (ns, local) = reader.resolver().resolve_element(owned.name());
                if is_core_ns(&ns) {
                    match local.as_ref() {
                        b"basematerials" => {
                            let group = parse_base_materials(reader, &owned)?;
                            register_id(seen_ids, group.id)?;
                            document.base_materials.push(group);
                        }
                        b"object" => {
                            let object = parse_object(reader, &owned, document, declared)?;
                            register_id(seen_ids, object.id)?;
                            declared.declare(&object);
                            document.objects.push(object);
                        }
                        _ => skip_subtree(reader, &owned)?,
                    }
                } else if is_slice_ns(&ns) && local.as_ref() == b"slicestack" {
                    let stack = parse_slice_stack(reader, &owned)?;
                    register_id(seen_ids, stack.id)?;
                    document.slice_stacks.push(stack);
                } else {
                    skip_subtree(reader, &owned)?;
                }
            }
            Event::Empty(e) => {
                let owned = e.into_owned();
                let (ns, local) = reader.resolver().resolve_element(owned.name());
                if is_core_ns(&ns) {
                    match local.as_ref() {
                        b"basematerials" => {
                            let attributes = collect_attributes(reader, &owned)?;
                            let id = parse_resource_id(attributes.require("basematerials", "id")?)?;
                            register_id(seen_ids, id)?;
                            document.base_materials.push(BaseMaterialsGroup {
                                id,
                                bases: Vec::new(),
                            });
                        }
                        b"object" => {
                            // An empty-element object cannot carry a mesh or
                            // components.
                            let attributes = collect_attributes(reader, &owned)?;
                            let id = parse_resource_id(attributes.require("object", "id")?)?;
                            return Err(ModelParseError::ObjectWithoutGeometry { id: id.get() });
                        }
                        _ => {}
                    }
                } else if is_slice_ns(&ns) && local.as_ref() == b"slicestack" {
                    let (id, z_bottom) = parse_slice_stack_attributes(reader, &owned)?;
                    register_id(seen_ids, id)?;
                    document.slice_stacks.push(SliceStack {
                        id,
                        z_bottom,
                        content: SliceStackContent::Slices(Vec::new()),
                    });
                }
            }
            Event::End(_) => break,
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(())
}

fn parse_base_materials(
    reader: &mut Reader<'_>,
    start: &BytesStart<'static>,
) -> Result<BaseMaterialsGroup, ModelParseError> {
    let attributes = collect_attributes(reader, start)?;
    let id = parse_resource_id(attributes.require("basematerials", "id")?)?;
    let mut bases = Vec::new();
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(e) => {
                let owned = e.into_owned();
                let base = parse_base_attributes(reader, &owned)?;
                skip_subtree(reader, &owned)?;
                if let Some(base) = base {
                    bases.push(base);
                }
            }
            Event::Empty(e) => {
                let owned = e.into_owned();
                if let Some(base) = parse_base_attributes(reader, &owned)? {
                    bases.push(base);
                }
            }
            Event::End(_) => break,
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(BaseMaterialsGroup { id, bases })
}

fn parse_base_attributes(
    reader: &Reader<'_>,
    e: &BytesStart<'static>,
) -> Result<Option<BaseMaterial>, ModelParseError> {
    let (ns, local) = reader.resolver().resolve_element(e.name());
    if !(is_core_ns(&ns) && local.as_ref() == b"base") {
        return Ok(None);
    }
    let attributes = collect_attributes(reader, e)?;
    Ok(Some(BaseMaterial {
        name: attributes.require("base", "name")?.to_string(),
        display_color: attributes.require("base", "displaycolor")?.to_string(),
    }))
}

fn parse_slice_stack_attributes(
    reader: &Reader<'_>,
    e: &BytesStart<'static>,
) -> Result<(ResourceId, f64), ModelParseError> {
    let attributes = collect_attributes(reader, e)?;
    let id = parse_resource_id(attributes.require("slicestack", "id")?)?;
    let z_bottom = match attributes.get("zbottom") {
        None => 0.0,
        Some(value) => parse_f64("slicestack", "zbottom", value)?,
    };
    Ok((id, z_bottom))
}

fn parse_slice_stack(
    reader: &mut Reader<'_>,
    start: &BytesStart<'static>,
) -> Result<SliceStack, ModelParseError> {
    let (id, z_bottom) = parse_slice_stack_attributes(reader, start)?;
    let mut slices: Vec<Slice> = Vec::new();
    let mut refs: Vec<SliceRef> = Vec::new();
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(e) => {
                let owned = e.into_owned();
                let (ns, local) = reader.resolver().resolve_element(owned.name());
                if is_slice_ns(&ns) {
                    match local.as_ref() {
                        b"slice" => slices.push(parse_slice(reader, &owned, id)?),
                        b"sliceref" => {
                            refs.push(parse_sliceref_attributes(reader, &owned)?);
                            skip_subtree(reader, &owned)?;
                        }
                        _ => skip_subtree(reader, &owned)?,
                    }
                } else {
                    skip_subtree(reader, &owned)?;
                }
            }
            Event::Empty(e) => {
                let owned = e.into_owned();
                let (ns, local) = reader.resolver().resolve_element(owned.name());
                if is_slice_ns(&ns) {
                    match local.as_ref() {
                        b"slice" => {
                            // An empty-element slice is a void layer.
                            let attributes = collect_attributes(reader, &owned)?;
                            let z_top =
                                parse_f64("slice", "ztop", attributes.require("slice", "ztop")?)?;
                            slices.push(Slice {
                                z_top,
                                vertices: Vec::new(),
                                polygons: Vec::new(),
                            });
                        }
                        b"sliceref" => refs.push(parse_sliceref_attributes(reader, &owned)?),
                        _ => {}
                    }
                }
            }
            Event::End(_) => break,
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let content = match (slices.is_empty(), refs.is_empty()) {
        (false, false) => {
            return Err(ModelParseError::SliceStackMixesSlicesAndRefs { id: id.get() });
        }
        (true, false) => SliceStackContent::Refs(refs),
        (_, true) => SliceStackContent::Slices(slices),
    };
    Ok(SliceStack {
        id,
        z_bottom,
        content,
    })
}

/// `slicestackid` here names a stack inside the TARGET part, so it is
/// deliberately not resolved against this document's resources.
fn parse_sliceref_attributes(
    reader: &Reader<'_>,
    e: &BytesStart<'static>,
) -> Result<SliceRef, ModelParseError> {
    let attributes = collect_attributes(reader, e)?;
    Ok(SliceRef {
        slice_stack: parse_resource_id(attributes.require("sliceref", "slicestackid")?)?,
        slice_path: attributes.require("sliceref", "slicepath")?.to_string(),
    })
}

fn parse_slice(
    reader: &mut Reader<'_>,
    start: &BytesStart<'static>,
    stack_id: ResourceId,
) -> Result<Slice, ModelParseError> {
    let attributes = collect_attributes(reader, start)?;
    let z_top = parse_f64("slice", "ztop", attributes.require("slice", "ztop")?)?;
    let mut vertices: Vec<SliceVertex> = Vec::new();
    let mut polygons: Vec<SlicePolygon> = Vec::new();
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(e) => {
                let owned = e.into_owned();
                let (ns, local) = reader.resolver().resolve_element(owned.name());
                if is_slice_ns(&ns) {
                    match local.as_ref() {
                        b"vertices" => parse_slice_vertices(reader, &mut vertices)?,
                        b"polygon" => {
                            polygons.push(parse_slice_polygon(
                                reader,
                                &owned,
                                stack_id,
                                vertices.len(),
                            )?);
                        }
                        _ => skip_subtree(reader, &owned)?,
                    }
                } else {
                    skip_subtree(reader, &owned)?;
                }
            }
            Event::Empty(e) => {
                let owned = e.into_owned();
                let (ns, local) = reader.resolver().resolve_element(owned.name());
                if is_slice_ns(&ns) && local.as_ref() == b"polygon" {
                    // A polygon without children cannot carry the required
                    // segment chain.
                    return Err(ModelParseError::PolygonWithoutSegments {
                        stack_id: stack_id.get(),
                    });
                }
            }
            Event::End(_) => break,
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(Slice {
        z_top,
        vertices,
        polygons,
    })
}

fn parse_slice_vertices(
    reader: &mut Reader<'_>,
    vertices: &mut Vec<SliceVertex>,
) -> Result<(), ModelParseError> {
    let mut buffer = Vec::new();
    loop {
        take_run(
            reader,
            vertices,
            is_slice_ns,
            b"vertex",
            element_run::slice_vertex_run,
        );
        match reader.read_event_into(&mut buffer)? {
            Event::Start(e) => {
                let owned = e.into_owned();
                parse_slice_vertex_attributes(reader, &owned, vertices)?;
                skip_subtree(reader, &owned)?;
            }
            Event::Empty(e) => {
                let owned = e.into_owned();
                parse_slice_vertex_attributes(reader, &owned, vertices)?;
            }
            Event::End(_) => break,
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(())
}

fn parse_slice_vertex_attributes(
    reader: &Reader<'_>,
    e: &BytesStart<'static>,
    vertices: &mut Vec<SliceVertex>,
) -> Result<(), ModelParseError> {
    let (ns, local) = reader.resolver().resolve_element(e.name());
    if !(is_slice_ns(&ns) && local.as_ref() == b"vertex") {
        return Ok(());
    }
    let attributes = collect_attributes(reader, e)?;
    vertices.push(SliceVertex {
        x: parse_f64("vertex", "x", attributes.require("vertex", "x")?)?,
        y: parse_f64("vertex", "y", attributes.require("vertex", "y")?)?,
    });
    Ok(())
}

fn parse_slice_polygon(
    reader: &mut Reader<'_>,
    start: &BytesStart<'static>,
    stack_id: ResourceId,
    vertex_count: usize,
) -> Result<SlicePolygon, ModelParseError> {
    let check_index = |index: ResourceIndex| -> Result<ResourceIndex, ModelParseError> {
        if index.get() as usize >= vertex_count {
            return Err(ModelParseError::SliceIndexOutOfRange {
                stack_id: stack_id.get(),
                index: index.get(),
                vertex_count,
            });
        }
        Ok(index)
    };
    let attributes = collect_attributes(reader, start)?;
    let polygon_start = check_index(parse_index(
        "polygon",
        "startv",
        attributes.require("polygon", "startv")?,
    )?)?;

    let mut segments: Vec<SliceSegment> = Vec::new();
    let mut buffer = Vec::new();
    loop {
        take_run(
            reader,
            &mut segments,
            is_slice_ns,
            b"segment",
            |input, name| element_run::slice_segment_run(input, name, vertex_count),
        );
        match reader.read_event_into(&mut buffer)? {
            Event::Start(e) => {
                let owned = e.into_owned();
                if let Some(end) = parse_segment_end(reader, &owned)? {
                    segments.push(SliceSegment {
                        end: check_index(end)?,
                    });
                }
                skip_subtree(reader, &owned)?;
            }
            Event::Empty(e) => {
                let owned = e.into_owned();
                if let Some(end) = parse_segment_end(reader, &owned)? {
                    segments.push(SliceSegment {
                        end: check_index(end)?,
                    });
                }
            }
            Event::End(_) => break,
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if segments.is_empty() {
        return Err(ModelParseError::PolygonWithoutSegments {
            stack_id: stack_id.get(),
        });
    }
    Ok(SlicePolygon {
        start: polygon_start,
        segments,
    })
}

/// The optional segment properties (p1, p2, pid) land in the attribute map and
/// are dropped: this crate does not interpret property semantics.
fn parse_segment_end(
    reader: &Reader<'_>,
    e: &BytesStart<'static>,
) -> Result<Option<ResourceIndex>, ModelParseError> {
    let (ns, local) = reader.resolver().resolve_element(e.name());
    if !(is_slice_ns(&ns) && local.as_ref() == b"segment") {
        return Ok(None);
    }
    let attributes = collect_attributes(reader, e)?;
    Ok(Some(parse_index(
        "segment",
        "v2",
        attributes.require("segment", "v2")?,
    )?))
}

fn parse_object(
    reader: &mut Reader<'_>,
    start: &BytesStart<'static>,
    document: &ModelDocument,
    declared: &DeclaredObjects,
) -> Result<ObjectResource, ModelParseError> {
    let attributes = collect_attributes(reader, start)?;
    let id = parse_resource_id(attributes.require("object", "id")?)?;
    let object_type = match attributes.get("type") {
        None => ObjectType::default(),
        Some(value) => {
            ObjectType::parse(value).ok_or_else(|| ModelParseError::InvalidObjectType {
                value: value.to_string(),
            })?
        }
    };
    let property_ref = match (attributes.get("pid"), attributes.get("pindex")) {
        (Some(pid), Some(pindex)) => Some(PropertyRef {
            group: parse_resource_id(pid)?,
            index: parse_index("object", "pindex", pindex)?,
        }),
        _ => None,
    };
    let slice_stack = match attributes.get_slice("slicestackid") {
        None => None,
        Some(value) => {
            let stack_id = parse_resource_id(value)?;
            if document.slice_stack(stack_id).is_none() {
                return Err(ModelParseError::UnknownSliceStackReference {
                    object_id: id.get(),
                    stack_id: stack_id.get(),
                });
            }
            Some(stack_id)
        }
    };
    let mesh_resolution = match attributes.get_slice("meshresolution") {
        None => MeshResolution::default(),
        Some(value) => {
            MeshResolution::parse(value).ok_or_else(|| ModelParseError::InvalidMeshResolution {
                value: value.to_string(),
            })?
        }
    };

    let mut payload: Option<ObjectPayload> = None;
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(e) => {
                let owned = e.into_owned();
                let (ns, local) = reader.resolver().resolve_element(owned.name());
                if !is_core_ns(&ns) {
                    skip_subtree(reader, &owned)?;
                } else {
                    match local.as_ref() {
                        b"mesh" => {
                            if payload.is_some() {
                                return Err(ModelParseError::DuplicateGeometry { id: id.get() });
                            }
                            payload = Some(ObjectPayload::Mesh(parse_mesh(reader, id)?));
                        }
                        b"components" => {
                            if payload.is_some() {
                                return Err(ModelParseError::DuplicateGeometry { id: id.get() });
                            }
                            payload = Some(ObjectPayload::Components(parse_components(
                                reader, declared,
                            )?));
                        }
                        _ => skip_subtree(reader, &owned)?,
                    }
                }
            }
            Event::Empty(e) => {
                // Empty-element forms of the geometry containers: `<mesh/>` and
                // `<components/>` carry no children but still decide the
                // payload kind.
                let owned = e.into_owned();
                let (ns, local) = reader.resolver().resolve_element(owned.name());
                if is_core_ns(&ns) {
                    match local.as_ref() {
                        b"mesh" => {
                            if payload.is_some() {
                                return Err(ModelParseError::DuplicateGeometry { id: id.get() });
                            }
                            payload = Some(ObjectPayload::Mesh(MeshData::default()));
                        }
                        b"components" => {
                            if payload.is_some() {
                                return Err(ModelParseError::DuplicateGeometry { id: id.get() });
                            }
                            payload = Some(ObjectPayload::Components(Vec::new()));
                        }
                        _ => {}
                    }
                }
            }
            Event::End(_) => break,
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }

    let payload = payload.ok_or(ModelParseError::ObjectWithoutGeometry { id: id.get() })?;
    Ok(ObjectResource {
        id,
        object_type,
        name: attributes.get("name").map(str::to_string),
        partnumber: attributes.get("partnumber").map(str::to_string),
        property_ref,
        slice_stack,
        mesh_resolution,
        payload,
    })
}

fn parse_mesh(reader: &mut Reader<'_>, object_id: ResourceId) -> Result<MeshData, ModelParseError> {
    let mut mesh = MeshData::default();
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(e) => {
                let owned = e.into_owned();
                let (ns, local) = reader.resolver().resolve_element(owned.name());
                if !is_core_ns(&ns) {
                    skip_subtree(reader, &owned)?;
                } else {
                    match local.as_ref() {
                        b"vertices" => parse_vertices(reader, &mut mesh)?,
                        b"triangles" => parse_triangles(reader, &mut mesh, object_id)?,
                        _ => skip_subtree(reader, &owned)?,
                    }
                }
            }
            Event::End(_) => break,
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(mesh)
}

/// The name of the element at the reader's cursor, when it resolves to `local`
/// in the namespace `in_namespace` accepts. Resolution goes through the
/// reader's own bindings, so a prefixed spelling is found and an element that
/// merely looks the part is not.
fn leading_element<'a>(
    reader: &Reader<'a>,
    in_namespace: fn(&ResolveResult<'_>) -> bool,
    local: &[u8],
) -> Option<&'a [u8]> {
    let input: &'a [u8] = reader.get_ref();
    let name = element_run::leading_element_name(input)?;
    let (namespace, found) = reader.resolver().resolve_element(QName(name));
    (in_namespace(&namespace) && found.as_ref() == local).then_some(name)
}

/// Take the leaf elements that are spelled plainly enough to read straight from
/// the input, and move the reader past them. Whatever the run does not cover,
/// including the element that ended it, is left to the event loop, so every
/// diagnosis still comes from the ordinary path.
///
/// Moving the reader rewrites its source slice. quick-xml offers
/// `Reader::stream` for reading raw bytes out of the source mid-document, but
/// `NsReader` exposes neither it nor a way through to it. The one documented
/// consequence is that `buffer_position` stops counting those bytes; nothing
/// here reads it, and no error this crate reports carries a byte offset.
fn take_run<'a, T>(
    reader: &mut Reader<'a>,
    target: &mut Vec<T>,
    in_namespace: fn(&ResolveResult<'_>) -> bool,
    local: &[u8],
    read: impl Fn(&'a [u8], &'a [u8]) -> element_run::ElementRun<T>,
) {
    let Some(name) = leading_element(reader, in_namespace, local) else {
        return;
    };
    let input: &'a [u8] = reader.get_ref();
    let run = read(input, name);
    if run.consumed == 0 {
        return;
    }
    target.extend(run.items);
    *reader.get_mut() = &input[run.consumed..];
}

fn parse_vertices(reader: &mut Reader<'_>, mesh: &mut MeshData) -> Result<(), ModelParseError> {
    let mut buffer = Vec::new();
    loop {
        take_run(
            reader,
            &mut mesh.vertices,
            is_core_ns,
            b"vertex",
            element_run::vertex_run,
        );
        match reader.read_event_into(&mut buffer)? {
            Event::Start(e) => {
                let owned = e.into_owned();
                parse_vertex_attributes(reader, &owned, mesh)?;
                skip_subtree(reader, &owned)?;
            }
            Event::Empty(e) => {
                let owned = e.into_owned();
                parse_vertex_attributes(reader, &owned, mesh)?;
            }
            Event::End(_) => break,
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(())
}

fn parse_vertex_attributes(
    reader: &Reader<'_>,
    e: &BytesStart<'static>,
    mesh: &mut MeshData,
) -> Result<(), ModelParseError> {
    let (ns, local) = reader.resolver().resolve_element(e.name());
    if !(is_core_ns(&ns) && local.as_ref() == b"vertex") {
        return Ok(());
    }
    let attributes = collect_attributes(reader, e)?;
    mesh.vertices.push(Vertex3 {
        x: parse_f64("vertex", "x", attributes.require("vertex", "x")?)?,
        y: parse_f64("vertex", "y", attributes.require("vertex", "y")?)?,
        z: parse_f64("vertex", "z", attributes.require("vertex", "z")?)?,
    });
    Ok(())
}

fn parse_triangles(
    reader: &mut Reader<'_>,
    mesh: &mut MeshData,
    object_id: ResourceId,
) -> Result<(), ModelParseError> {
    let mut buffer = Vec::new();
    loop {
        // The run checks indices against the vertices read so far, which is the
        // list, at the point in the document, the event path checks against.
        let vertex_count = mesh.vertices.len();
        take_run(
            reader,
            &mut mesh.triangles,
            is_core_ns,
            b"triangle",
            |input, name| element_run::triangle_run(input, name, vertex_count),
        );
        match reader.read_event_into(&mut buffer)? {
            Event::Start(e) => {
                let owned = e.into_owned();
                parse_triangle_attributes(reader, &owned, mesh, object_id)?;
                skip_subtree(reader, &owned)?;
            }
            Event::Empty(e) => {
                let owned = e.into_owned();
                parse_triangle_attributes(reader, &owned, mesh, object_id)?;
            }
            Event::End(_) => break,
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(())
}

fn parse_triangle_attributes(
    reader: &Reader<'_>,
    e: &BytesStart<'static>,
    mesh: &mut MeshData,
    object_id: ResourceId,
) -> Result<(), ModelParseError> {
    let (ns, local) = reader.resolver().resolve_element(e.name());
    if !(is_core_ns(&ns) && local.as_ref() == b"triangle") {
        return Ok(());
    }
    let attributes = collect_attributes(reader, e)?;
    let triangle = TriangleRef {
        v1: parse_index("triangle", "v1", attributes.require("triangle", "v1")?)?,
        v2: parse_index("triangle", "v2", attributes.require("triangle", "v2")?)?,
        v3: parse_index("triangle", "v3", attributes.require("triangle", "v3")?)?,
    };
    for index in [triangle.v1, triangle.v2, triangle.v3] {
        if index.get() as usize >= mesh.vertices.len() {
            return Err(ModelParseError::TriangleIndexOutOfRange {
                object_id: object_id.get(),
                index: index.get(),
                vertex_count: mesh.vertices.len(),
            });
        }
    }
    mesh.triangles.push(triangle);
    Ok(())
}

fn parse_components(
    reader: &mut Reader<'_>,
    declared: &DeclaredObjects,
) -> Result<Vec<Component>, ModelParseError> {
    let mut components = Vec::new();
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(e) => {
                let owned = e.into_owned();
                let component = parse_component_attributes(reader, &owned, declared)?;
                skip_subtree(reader, &owned)?;
                if let Some(component) = component {
                    components.push(component);
                }
            }
            Event::Empty(e) => {
                let owned = e.into_owned();
                if let Some(component) = parse_component_attributes(reader, &owned, declared)? {
                    components.push(component);
                }
            }
            Event::End(_) => break,
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(components)
}

fn parse_component_attributes(
    reader: &Reader<'_>,
    e: &BytesStart<'static>,
    declared: &DeclaredObjects,
) -> Result<Option<Component>, ModelParseError> {
    let (ns, local) = reader.resolver().resolve_element(e.name());
    if !(is_core_ns(&ns) && local.as_ref() == b"component") {
        return Ok(None);
    }
    let attributes = collect_attributes(reader, e)?;
    let object_id = parse_resource_id(attributes.require("component", "objectid")?)?;
    if declared.object_type(object_id).is_none() {
        return Err(ModelParseError::UnknownObjectReference {
            id: object_id.get(),
        });
    }
    let transform = attributes
        .get("transform")
        .map(parse_transform)
        .transpose()?;
    Ok(Some(Component {
        object_id,
        transform,
    }))
}

fn parse_build(
    reader: &mut Reader<'_>,
    document: &mut ModelDocument,
    declared: &DeclaredObjects,
) -> Result<(), ModelParseError> {
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(e) => {
                let owned = e.into_owned();
                let item = parse_item_attributes(reader, &owned, declared)?;
                skip_subtree(reader, &owned)?;
                if let Some(item) = item {
                    document.build.push(item);
                }
            }
            Event::Empty(e) => {
                let owned = e.into_owned();
                if let Some(item) = parse_item_attributes(reader, &owned, declared)? {
                    document.build.push(item);
                }
            }
            Event::End(_) => break,
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(())
}

fn parse_item_attributes(
    reader: &Reader<'_>,
    e: &BytesStart<'static>,
    declared: &DeclaredObjects,
) -> Result<Option<BuildItem>, ModelParseError> {
    let (ns, local) = reader.resolver().resolve_element(e.name());
    if !(is_core_ns(&ns) && local.as_ref() == b"item") {
        return Ok(None);
    }
    let attributes = collect_attributes(reader, e)?;
    let object_id = parse_resource_id(attributes.require("item", "objectid")?)?;
    let Some(object_type) = declared.object_type(object_id) else {
        return Err(ModelParseError::UnknownObjectReference {
            id: object_id.get(),
        });
    };
    if object_type == ObjectType::Other {
        return Err(ModelParseError::BuildItemReferencesOtherType {
            id: object_id.get(),
        });
    }
    let transform = attributes
        .get("transform")
        .map(parse_transform)
        .transpose()?;
    Ok(Some(BuildItem {
        object_id,
        transform,
        partnumber: attributes.get("partnumber").map(str::to_string),
    }))
}
