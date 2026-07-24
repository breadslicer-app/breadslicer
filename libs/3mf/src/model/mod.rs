/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Typed document model for the 3D Model part, with XML parsing and
//! serialization. Coordinates stay in the document's declared unit;
//! converting to millimetres is the caller's job via
//! [`Unit::scale_to_mm`].

mod read;
mod types;
mod write;

pub use read::parse_model;
pub use types::{
    BaseMaterial, BaseMaterialsGroup, BuildItem, Component, Matrix3D, MeshData, Metadata,
    ModelDocument, ObjectPayload, ObjectResource, ObjectType, PropertyRef, ResourceId,
    ResourceIndex, TriangleRef, Unit, Vertex3,
};
pub use write::write_model;
