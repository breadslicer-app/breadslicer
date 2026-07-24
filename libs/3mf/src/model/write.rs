/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Serializer for the 3D Model part. Assumes a well-formed
//! [`ModelDocument`] (adapters construct them); only the output
//! channel can fail. Numbers use Rust's shortest-round-trip decimal
//! form; text and attribute values are XML-escaped by quick-xml.

use std::io::Write;

use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use quick_xml::writer::Writer;

use crate::error::ModelWriteError;
use crate::names::CORE_NS;

use super::types::{
    BaseMaterialsGroup, BuildItem, MeshData, Metadata, ModelDocument, ObjectPayload,
    ObjectResource, ObjectType,
};

/// Serialize `document` as the XML payload of a 3D Model part.
pub fn write_model(
    document: &ModelDocument,
    output: &mut impl Write,
) -> Result<(), ModelWriteError> {
    let mut writer = Writer::new(output);
    writer.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;

    let mut model = BytesStart::new("model");
    model.push_attribute(("unit", document.unit.as_str()));
    model.push_attribute(("xmlns", CORE_NS));
    writer.write_event(Event::Start(model))?;

    for metadata in &document.metadata {
        write_metadata(&mut writer, metadata)?;
    }

    writer.write_event(Event::Start(BytesStart::new("resources")))?;
    for group in &document.base_materials {
        write_base_materials(&mut writer, group)?;
    }
    for object in &document.objects {
        write_object(&mut writer, object)?;
    }
    writer.write_event(Event::End(BytesEnd::new("resources")))?;

    writer.write_event(Event::Start(BytesStart::new("build")))?;
    for item in &document.build {
        write_item(&mut writer, item)?;
    }
    writer.write_event(Event::End(BytesEnd::new("build")))?;

    writer.write_event(Event::End(BytesEnd::new("model")))?;
    Ok(())
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
    writer.write_event(Event::Start(e))?;
    match &object.payload {
        ObjectPayload::Mesh(mesh) => write_mesh(writer, mesh)?,
        ObjectPayload::Components(components) => {
            writer.write_event(Event::Start(BytesStart::new("components")))?;
            for component in components {
                let mut c = BytesStart::new("component");
                c.push_attribute(("objectid", component.object_id.get().to_string().as_str()));
                if let Some(transform) = &component.transform {
                    c.push_attribute(("transform", transform.to_attribute().as_str()));
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
    for vertex in &mesh.vertices {
        let mut v = BytesStart::new("vertex");
        v.push_attribute(("x", vertex.x.to_string().as_str()));
        v.push_attribute(("y", vertex.y.to_string().as_str()));
        v.push_attribute(("z", vertex.z.to_string().as_str()));
        writer.write_event(Event::Empty(v))?;
    }
    writer.write_event(Event::End(BytesEnd::new("vertices")))?;

    writer.write_event(Event::Start(BytesStart::new("triangles")))?;
    for triangle in &mesh.triangles {
        let mut t = BytesStart::new("triangle");
        t.push_attribute(("v1", triangle.v1.get().to_string().as_str()));
        t.push_attribute(("v2", triangle.v2.get().to_string().as_str()));
        t.push_attribute(("v3", triangle.v3.get().to_string().as_str()));
        writer.write_event(Event::Empty(t))?;
    }
    writer.write_event(Event::End(BytesEnd::new("triangles")))?;

    writer.write_event(Event::End(BytesEnd::new("mesh")))?;
    Ok(())
}

fn write_item<W: Write>(writer: &mut Writer<W>, item: &BuildItem) -> Result<(), ModelWriteError> {
    let mut e = BytesStart::new("item");
    e.push_attribute(("objectid", item.object_id.get().to_string().as_str()));
    if let Some(transform) = &item.transform {
        e.push_attribute(("transform", transform.to_attribute().as_str()));
    }
    if let Some(partnumber) = &item.partnumber {
        e.push_attribute(("partnumber", partnumber.as_str()));
    }
    writer.write_event(Event::Empty(e))?;
    Ok(())
}
