/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 *
 * sc-mesh-formats:
 * A "SoulCrafted" library to read, write and manipulate 3d mesh data
 */

mod core;
mod filetypes;
mod stl;

pub use core::IndexedMesh;

pub use filetypes::{
    InvalidSourceMeshFileType, SourceMeshFileType, infer_mesh_file_format_from_extension,
};

pub use stl::{read_stl, read_stla, read_stlb, write_stla, write_stlb};
