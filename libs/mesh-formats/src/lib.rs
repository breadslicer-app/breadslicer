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
pub mod ports;
pub mod scene_writer;
pub mod stl;

#[cfg(test)]
mod tests_infer_file_type_error;
#[cfg(test)]
mod tests_load_mesh_error;
#[cfg(test)]
mod tests_load_obj_error;
#[cfg(test)]
mod tests_load_stl_error;
#[cfg(test)]
mod tests_write_obj_error;
#[cfg(test)]
mod tests_write_scene_error;
#[cfg(test)]
mod tests_write_stl_error;

pub use loader::{LoadedMesh, load_mesh_from_path, load_mesh_from_reader};
