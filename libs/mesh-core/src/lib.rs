/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

//! Core mesh data structures and validation routines for the slicer.
//!
//! # Conventions
//!
//! - **Coordinate frame:** right-handed, with **Z up**. The XY plane is the
//!   build platform; +Z points away from it. The slicer engine, format
//!   readers/writers, and any new geometry code must follow this
//!   convention.
//! - **Units:** every coordinate, length, and translation in this crate is
//!   in **millimetres**. Mesh storage is `f32` for vertex positions and
//!   `f64` for transform matrices; both share the same millimetre scale.
//!   Format readers (STL, OBJ, …) are unit-less by spec, so the convention
//!   is "the file's numbers are millimetres"; converting other units is
//!   the caller's job.

pub mod aabb;
pub mod geometry;
pub mod indexed_mesh;
pub mod presupported;
pub mod primitives;
pub mod shells;
pub mod transform;
pub mod triangle_iterator;

// Re-exporting "core" symbols for ease of use
pub use crate::aabb::Aabb;
pub use crate::indexed_mesh::{IndexedMesh, MeshMetadata, ValidatedEdgeManifoldMesh};
pub use crate::primitives::{IndexedTriangle, Normal, Triangle, Vector3D, Vertex};
pub use crate::triangle_iterator::{
    IndexedMeshBuildError, TriangleIterator, indexed_mesh_from_soup,
};

#[cfg(test)]
mod tests_shells;
#[cfg(test)]
mod tests_triangle_iterator;
