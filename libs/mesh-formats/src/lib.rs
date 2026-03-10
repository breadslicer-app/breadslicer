/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

mod core;
mod filetypes;
mod stl;

pub use core::IndexedMesh;

pub use filetypes::{
    InvalidSourceMeshFileType, SourceMeshFileType, infer_mesh_file_format_from_extension,
};
