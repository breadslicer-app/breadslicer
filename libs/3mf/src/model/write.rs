/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Serializer for the 3D Model part. Assumes a structurally
//! well-formed [`ModelDocument`] (adapters construct them), but
//! enforces the slice-extension invariants a conforming file must
//! satisfy: finite, strictly increasing slice boundaries, closed contours
//! for model-type objects, resolvable stack references, and planar
//! transforms on sliced objects. Numbers use a shortest-round-trip
//! decimal form; text and attribute values are XML-escaped by
//! quick-xml. The bulk mesh and slice coordinate elements are formatted
//! straight into the underlying sink (their numeric attributes never
//! need escaping), skipping the per-element event and allocation cost;
//! for large inputs the independent chunks are formatted in parallel and
//! written back in order, which is byte-identical to a sequential emit.

use std::collections::BTreeSet;
use std::io::Write;

use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use quick_xml::writer::Writer;
use rayon::prelude::*;

use crate::error::ModelWriteError;
use crate::names::{CORE_NS, PRODUCTION_NS, SLICE_NS};

use super::slice::{MeshResolution, Slice, SliceStack, SliceStackContent};
use super::types::{
    BaseMaterialsGroup, BuildItem, Matrix3D, MeshData, Metadata, ModelDocument, ObjectPayload,
    ObjectResource, ObjectType, TriangleRef, Vertex3,
};

/// Serialize `document` as the XML payload of a 3D Model part.
pub fn write_model(
    document: &ModelDocument,
    output: &mut impl Write,
) -> Result<(), ModelWriteError> {
    let sliced = validate_slice_content(document)?;
    let has_slice_content = !document.slice_stacks.is_empty()
        || document
            .objects
            .iter()
            .any(|o| o.slice_stack.is_some() || o.mesh_resolution != MeshResolution::Fullres);

    let mut writer = Writer::new(output);
    writer.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;

    let has_production_paths = document.build.iter().any(|i| i.path.is_some())
        || document.objects.iter().any(|o| match &o.payload {
            ObjectPayload::Components(components) => components.iter().any(|c| c.path.is_some()),
            ObjectPayload::Mesh(_) => false,
        });

    let mut model = BytesStart::new("model");
    model.push_attribute(("unit", document.unit.as_str()));
    model.push_attribute(("xmlns", CORE_NS));
    let mut required = Vec::new();
    if has_slice_content {
        model.push_attribute(("xmlns:s", SLICE_NS));
        let any_lowres = document
            .objects
            .iter()
            .any(|o| o.mesh_resolution == MeshResolution::Lowres);
        // With full-resolution meshes the file stays usable for
        // core-only consumers, so the extension is only recommended;
        // a lowres stand-in makes slice support mandatory.
        if any_lowres {
            required.push("s");
        } else {
            model.push_attribute(("recommendedextensions", "s"));
        }
    }
    // Without the production extension the referenced objects are missing.
    if has_production_paths {
        model.push_attribute(("xmlns:p", PRODUCTION_NS));
        required.push("p");
    }
    if !required.is_empty() {
        model.push_attribute(("requiredextensions", required.join(" ").as_str()));
    }
    writer.write_event(Event::Start(model))?;

    for metadata in &document.metadata {
        write_metadata(&mut writer, metadata)?;
    }

    writer.write_event(Event::Start(BytesStart::new("resources")))?;
    for stack in &document.slice_stacks {
        write_slice_stack(&mut writer, stack)?;
    }
    for group in &document.base_materials {
        write_base_materials(&mut writer, group)?;
    }
    for object in &document.objects {
        write_object(&mut writer, object, &sliced)?;
    }
    writer.write_event(Event::End(BytesEnd::new("resources")))?;

    writer.write_event(Event::Start(BytesStart::new("build")))?;
    for item in &document.build {
        write_item(&mut writer, item, &sliced)?;
    }
    writer.write_event(Event::End(BytesEnd::new("build")))?;

    writer.write_event(Event::End(BytesEnd::new("model")))?;
    Ok(())
}

/// Checks every slice-extension invariant the writer enforces and
/// returns the set of transitively sliced object ids: objects with a
/// stack of their own, plus components objects containing one. The
/// propagation runs in definition order, which is also reference
/// order for any document that can parse back.
fn validate_slice_content(document: &ModelDocument) -> Result<BTreeSet<u32>, ModelWriteError> {
    for stack in &document.slice_stacks {
        if let SliceStackContent::Slices(slices) = &stack.content {
            if !stack.z_bottom.is_finite() {
                return Err(ModelWriteError::SliceZBottomNotFinite {
                    stack_id: stack.id.get(),
                });
            }
            let mut previous = stack.z_bottom;
            for (index, slice) in slices.iter().enumerate() {
                if !slice.z_top.is_finite() {
                    return Err(ModelWriteError::SliceZTopNotFinite {
                        stack_id: stack.id.get(),
                        index,
                    });
                }
                if slice.z_top <= previous {
                    return Err(ModelWriteError::SliceZNotIncreasing {
                        stack_id: stack.id.get(),
                        index,
                    });
                }
                previous = slice.z_top;
                for polygon in &slice.polygons {
                    if polygon.segments.is_empty() {
                        return Err(ModelWriteError::PolygonWithoutSegments {
                            stack_id: stack.id.get(),
                        });
                    }
                    let indices = std::iter::once(polygon.start)
                        .chain(polygon.segments.iter().map(|s| s.end));
                    for index in indices {
                        if index.get() as usize >= slice.vertices.len() {
                            return Err(ModelWriteError::SliceIndexOutOfRange {
                                stack_id: stack.id.get(),
                                index: index.get(),
                                vertex_count: slice.vertices.len(),
                            });
                        }
                    }
                }
            }
        }
    }

    for object in &document.objects {
        let Some(stack_id) = object.slice_stack else {
            continue;
        };
        let Some(stack) = document.slice_stack(stack_id) else {
            return Err(ModelWriteError::UnknownSliceStackReference {
                object_id: object.id.get(),
                stack_id: stack_id.get(),
            });
        };
        let closed_required = matches!(
            object.object_type,
            ObjectType::Model | ObjectType::SolidSupport,
        );
        if closed_required && let SliceStackContent::Slices(slices) = &stack.content {
            for slice in slices {
                if slice.polygons.iter().any(|p| !p.is_closed()) {
                    return Err(ModelWriteError::OpenPolygonOnModelObject {
                        object_id: object.id.get(),
                        stack_id: stack_id.get(),
                    });
                }
            }
        }
    }

    let mut sliced: BTreeSet<u32> = BTreeSet::new();
    for object in &document.objects {
        let is_sliced = object.slice_stack.is_some()
            || matches!(
                &object.payload,
                ObjectPayload::Components(components)
                    if components.iter().any(|c| sliced.contains(&c.object_id.get())),
            );
        if is_sliced {
            sliced.insert(object.id.get());
        }
    }

    for object in &document.objects {
        if let ObjectPayload::Components(components) = &object.payload {
            for component in components {
                if sliced.contains(&component.object_id.get())
                    && let Some(transform) = &component.transform
                    && !is_planar(transform)
                {
                    return Err(ModelWriteError::NonPlanarSlicedTransform {
                        object_id: component.object_id.get(),
                    });
                }
            }
        }
    }
    for item in &document.build {
        if sliced.contains(&item.object_id.get())
            && let Some(transform) = &item.transform
            && !is_planar(transform)
        {
            return Err(ModelWriteError::NonPlanarSlicedTransform {
                object_id: item.object_id.get(),
            });
        }
    }

    Ok(sliced)
}

/// The slice extension's planar requirement, checked numerically:
/// the consortium's own sliced sample writes these cells as
/// "0.0000"/"1.0000", so a textual comparison would be stricter than
/// the ecosystem.
fn is_planar(matrix: &Matrix3D) -> bool {
    let m = &matrix.0;
    m[2] == 0.0 && m[5] == 0.0 && m[6] == 0.0 && m[7] == 0.0 && m[8] == 1.0
}

/// Overwrites the 5 planarity cells with exact constants so a -0.0
/// that passed the numeric check cannot serialize as "-0".
fn planar_normalized(matrix: &Matrix3D) -> Matrix3D {
    let mut m = matrix.0;
    m[2] = 0.0;
    m[5] = 0.0;
    m[6] = 0.0;
    m[7] = 0.0;
    m[8] = 1.0;
    Matrix3D(m)
}

/// Attribute form of a transform on a reference to `object_id`:
/// planar-normalized when the target is sliced.
fn transform_attribute(matrix: &Matrix3D, target_sliced: bool) -> String {
    if target_sliced {
        planar_normalized(matrix).to_attribute()
    } else {
        matrix.to_attribute()
    }
}

fn write_slice_stack<W: Write>(
    writer: &mut Writer<W>,
    stack: &SliceStack,
) -> Result<(), ModelWriteError> {
    let mut e = BytesStart::new("s:slicestack");
    e.push_attribute(("id", stack.id.get().to_string().as_str()));
    e.push_attribute(("zbottom", stack.z_bottom.to_string().as_str()));
    writer.write_event(Event::Start(e))?;
    match &stack.content {
        SliceStackContent::Slices(slices) => {
            write_slices(writer.get_mut(), slices)?;
        }
        SliceStackContent::Refs(refs) => {
            for slice_ref in refs {
                let mut r = BytesStart::new("s:sliceref");
                r.push_attribute((
                    "slicestackid",
                    slice_ref.slice_stack.get().to_string().as_str(),
                ));
                r.push_attribute(("slicepath", slice_ref.slice_path.as_str()));
                writer.write_event(Event::Empty(r))?;
            }
        }
    }
    writer.write_event(Event::End(BytesEnd::new("s:slicestack")))?;
    Ok(())
}

/// Emit the elements of `items` to `sink`. Above a threshold the
/// independent chunks are formatted in parallel and written back in
/// order; the output bytes are identical to a sequential emit, so
/// readers and the slice-input fingerprint see no difference. Short
/// lists format inline, avoiding thread-pool overhead on small meshes.
fn write_parts<T, W>(
    sink: &mut W,
    items: &[T],
    format_chunk: fn(&[T]) -> Vec<u8>,
) -> std::io::Result<()>
where
    T: Sync,
    W: Write,
{
    const PARALLEL_THRESHOLD: usize = 8192;
    if items.len() < PARALLEL_THRESHOLD {
        sink.write_all(&format_chunk(items))?;
        return Ok(());
    }
    let chunk = (items.len() / (rayon::current_num_threads() * 4)).max(1);
    let parts: Vec<Vec<u8>> = items.par_chunks(chunk).map(format_chunk).collect();
    for part in &parts {
        sink.write_all(part)?;
    }
    Ok(())
}

/// Format a chunk of mesh vertices into its own buffer.
fn format_vertices(chunk: &[Vertex3]) -> Vec<u8> {
    let mut out = Vec::with_capacity(chunk.len() * 48);
    let mut buf = ryu::Buffer::new();
    for vertex in chunk {
        out.extend_from_slice(b"<vertex x=\"");
        out.extend_from_slice(buf.format(vertex.x).as_bytes());
        out.extend_from_slice(b"\" y=\"");
        out.extend_from_slice(buf.format(vertex.y).as_bytes());
        out.extend_from_slice(b"\" z=\"");
        out.extend_from_slice(buf.format(vertex.z).as_bytes());
        out.extend_from_slice(b"\"/>");
    }
    out
}

/// Format a chunk of triangles into its own buffer.
fn format_triangles(chunk: &[TriangleRef]) -> Vec<u8> {
    let mut out = Vec::with_capacity(chunk.len() * 40);
    let mut buf = itoa::Buffer::new();
    for triangle in chunk {
        out.extend_from_slice(b"<triangle v1=\"");
        out.extend_from_slice(buf.format(triangle.v1.get()).as_bytes());
        out.extend_from_slice(b"\" v2=\"");
        out.extend_from_slice(buf.format(triangle.v2.get()).as_bytes());
        out.extend_from_slice(b"\" v3=\"");
        out.extend_from_slice(buf.format(triangle.v3.get()).as_bytes());
        out.extend_from_slice(b"\"/>");
    }
    out
}

/// Emit `slices` to `sink`. A stack with many layers formats its slice
/// fragments in parallel and writes them in order; the bytes match a
/// sequential emit. Few-layer stacks format inline.
fn write_slices<W: Write>(sink: &mut W, slices: &[Slice]) -> std::io::Result<()> {
    const PARALLEL_THRESHOLD: usize = 64;
    if slices.len() < PARALLEL_THRESHOLD {
        for slice in slices {
            sink.write_all(&format_slice(slice))?;
        }
        return Ok(());
    }
    let parts: Vec<Vec<u8>> = slices.par_iter().map(format_slice).collect();
    for part in &parts {
        sink.write_all(part)?;
    }
    Ok(())
}

/// Format one slice as its complete `<s:slice>` fragment, byte-for-byte
/// as the sequential writer would.
fn format_slice(slice: &Slice) -> Vec<u8> {
    let mut out = Vec::new();
    let mut fbuf = ryu::Buffer::new();
    out.extend_from_slice(b"<s:slice ztop=\"");
    out.extend_from_slice(fbuf.format(slice.z_top).as_bytes());
    if slice.vertices.is_empty() && slice.polygons.is_empty() {
        // A void layer.
        out.extend_from_slice(b"\"/>");
        return out;
    }
    out.extend_from_slice(b"\"><s:vertices>");
    for vertex in &slice.vertices {
        out.extend_from_slice(b"<s:vertex x=\"");
        out.extend_from_slice(fbuf.format(vertex.x).as_bytes());
        out.extend_from_slice(b"\" y=\"");
        out.extend_from_slice(fbuf.format(vertex.y).as_bytes());
        out.extend_from_slice(b"\"/>");
    }
    out.extend_from_slice(b"</s:vertices>");
    let mut ibuf = itoa::Buffer::new();
    for polygon in &slice.polygons {
        out.extend_from_slice(b"<s:polygon startv=\"");
        out.extend_from_slice(ibuf.format(polygon.start.get()).as_bytes());
        out.extend_from_slice(b"\">");
        for segment in &polygon.segments {
            out.extend_from_slice(b"<s:segment v2=\"");
            out.extend_from_slice(ibuf.format(segment.end.get()).as_bytes());
            out.extend_from_slice(b"\"/>");
        }
        out.extend_from_slice(b"</s:polygon>");
    }
    out.extend_from_slice(b"</s:slice>");
    out
}

fn write_metadata<W: Write>(
    writer: &mut Writer<W>,
    metadata: &Metadata,
) -> Result<(), ModelWriteError> {
    let mut e = BytesStart::new("metadata");
    e.push_attribute(("name", metadata.name.as_str()));
    if metadata.preserve {
        e.push_attribute(("preserve", "1"));
    }
    if let Some(value_type) = &metadata.value_type {
        e.push_attribute(("type", value_type.as_str()));
    }
    writer.write_event(Event::Start(e))?;
    writer.write_event(Event::Text(BytesText::new(&metadata.value)))?;
    writer.write_event(Event::End(BytesEnd::new("metadata")))?;
    Ok(())
}

fn write_base_materials<W: Write>(
    writer: &mut Writer<W>,
    group: &BaseMaterialsGroup,
) -> Result<(), ModelWriteError> {
    let mut e = BytesStart::new("basematerials");
    e.push_attribute(("id", group.id.get().to_string().as_str()));
    writer.write_event(Event::Start(e))?;
    for base in &group.bases {
        let mut b = BytesStart::new("base");
        b.push_attribute(("name", base.name.as_str()));
        b.push_attribute(("displaycolor", base.display_color.as_str()));
        writer.write_event(Event::Empty(b))?;
    }
    writer.write_event(Event::End(BytesEnd::new("basematerials")))?;
    Ok(())
}

fn write_object<W: Write>(
    writer: &mut Writer<W>,
    object: &ObjectResource,
    sliced: &BTreeSet<u32>,
) -> Result<(), ModelWriteError> {
    let mut e = BytesStart::new("object");
    e.push_attribute(("id", object.id.get().to_string().as_str()));
    if object.object_type != ObjectType::Model {
        e.push_attribute(("type", object.object_type.as_str()));
    }
    if let Some(name) = &object.name {
        e.push_attribute(("name", name.as_str()));
    }
    if let Some(partnumber) = &object.partnumber {
        e.push_attribute(("partnumber", partnumber.as_str()));
    }
    if let Some(property_ref) = &object.property_ref {
        e.push_attribute(("pid", property_ref.group.get().to_string().as_str()));
        e.push_attribute(("pindex", property_ref.index.get().to_string().as_str()));
    }
    if let Some(stack_id) = &object.slice_stack {
        e.push_attribute(("s:slicestackid", stack_id.get().to_string().as_str()));
    }
    if object.mesh_resolution != MeshResolution::Fullres {
        e.push_attribute(("s:meshresolution", object.mesh_resolution.as_str()));
    }
    writer.write_event(Event::Start(e))?;
    match &object.payload {
        ObjectPayload::Mesh(mesh) => write_mesh(writer, mesh)?,
        ObjectPayload::Components(components) => {
            writer.write_event(Event::Start(BytesStart::new("components")))?;
            for component in components {
                let mut c = BytesStart::new("component");
                c.push_attribute(("objectid", component.object_id.get().to_string().as_str()));
                if let Some(transform) = &component.transform {
                    let target_sliced = sliced.contains(&component.object_id.get());
                    c.push_attribute((
                        "transform",
                        transform_attribute(transform, target_sliced).as_str(),
                    ));
                }
                if let Some(path) = &component.path {
                    c.push_attribute(("p:path", path.as_str()));
                }
                writer.write_event(Event::Empty(c))?;
            }
            writer.write_event(Event::End(BytesEnd::new("components")))?;
        }
    }
    writer.write_event(Event::End(BytesEnd::new("object")))?;
    Ok(())
}

fn write_mesh<W: Write>(writer: &mut Writer<W>, mesh: &MeshData) -> Result<(), ModelWriteError> {
    writer.write_event(Event::Start(BytesStart::new("mesh")))?;

    writer.write_event(Event::Start(BytesStart::new("vertices")))?;
    write_parts(writer.get_mut(), &mesh.vertices, format_vertices)?;
    writer.write_event(Event::End(BytesEnd::new("vertices")))?;

    writer.write_event(Event::Start(BytesStart::new("triangles")))?;
    write_parts(writer.get_mut(), &mesh.triangles, format_triangles)?;
    writer.write_event(Event::End(BytesEnd::new("triangles")))?;

    writer.write_event(Event::End(BytesEnd::new("mesh")))?;
    Ok(())
}

fn write_item<W: Write>(
    writer: &mut Writer<W>,
    item: &BuildItem,
    sliced: &BTreeSet<u32>,
) -> Result<(), ModelWriteError> {
    let mut e = BytesStart::new("item");
    e.push_attribute(("objectid", item.object_id.get().to_string().as_str()));
    if let Some(transform) = &item.transform {
        let target_sliced = sliced.contains(&item.object_id.get());
        e.push_attribute((
            "transform",
            transform_attribute(transform, target_sliced).as_str(),
        ));
    }
    if let Some(partnumber) = &item.partnumber {
        e.push_attribute(("partnumber", partnumber.as_str()));
    }
    if let Some(path) = &item.path {
        e.push_attribute(("p:path", path.as_str()));
    }
    writer.write_event(Event::Empty(e))?;
    Ok(())
}
