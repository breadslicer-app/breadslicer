/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! 3MF read support: adapts `sc-3mf` documents to [`Scene`] graphs.
//!
//! The `sc-3mf` crate owns the container (OPC/ZIP) and the model XML;
//! this module owns the domain conversion: objects become mesh
//! resources, build items become leaves (or groups, for component
//! objects) under one identity-Group scene root, document lengths are
//! scaled to millimetres, and 3MF's row-vector transforms become
//! [`sc_mesh_core::transform::Transform3D`] values.

mod reader;

use std::io::{Read, Seek};

use sc_mesh_core::Scene;

use crate::error::LoadThreeMfError;
use crate::messages::LoadThreeMfWarning;
use crate::ports::{MeshReader, sealed};

pub use reader::{MAX_COMPONENT_DEPTH, MAX_EXPANDED_LEAF_COUNT, read_threemf};

/// Format marker for 3MF packages. Implements [`MeshReader`] via
/// static dispatch; the dispatcher in [`crate::loader`] invokes
/// `ThreeMf::read(reader)`.
pub struct ThreeMf;

impl sealed::Sealed for ThreeMf {}

impl MeshReader for ThreeMf {
    type Error = LoadThreeMfError;
    type Warning = LoadThreeMfWarning;

    fn read<R: Read + Seek>(
        reader: &mut R,
    ) -> Result<(Scene, Vec<LoadThreeMfWarning>), LoadThreeMfError> {
        read_threemf(reader)
    }
}
