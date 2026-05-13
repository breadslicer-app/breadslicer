/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 *
 * sc-mesh-formats:
 * A "SoulCrafted" library to read, write and manipulate 3d mesh data
 */

pub mod error;
pub mod filetypes;
pub mod loader;
pub mod messages;
pub mod obj;
pub mod scene_writer;
pub mod stl;

pub use loader::{LoadedMesh, load_mesh_from_path, load_mesh_from_reader};
