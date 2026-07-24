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
pub mod threemf;

#[cfg(test)]
mod tests_infer_file_type_error;
#[cfg(test)]
mod tests_load_mesh_error;
#[cfg(test)]
mod tests_load_obj_error;
#[cfg(test)]
mod tests_load_stl_error;
#[cfg(test)]
mod tests_threemf;
#[cfg(test)]
mod tests_write_obj_error;
#[cfg(test)]
mod tests_write_scene_error;
#[cfg(test)]
mod tests_write_stl_error;

pub use loader::{LoadedMesh, load_mesh_from_path, load_mesh_from_reader};

/// Default hard upper bound on the on-disk size of a mesh file the
/// loader will accept, in bytes. 2 GiB sits well beyond anything a
/// resin-printer slice job needs and protects callers (CLI argv, GUI
/// drag-and-drop) from spending arbitrary time on an accidental huge
/// pick. Callers may pass a smaller cap to
/// [`load_mesh_from_path`](crate::loader::load_mesh_from_path) if
/// their use case warrants it.
pub const DEFAULT_MAX_MESH_FILE_SIZE_BYTES: u64 = 2048 * 1024 * 1024;

/// Hard ceiling on the count of a single mesh-element kind a format
/// reader will allow before refusing the input: declared-triangle
/// count for binary STL, vertex count for OBJ, etc. Shared across the
/// readers so every format imposes the same memory-exhaustion
/// backstop. 200M elements is well above any legitimate engineering
/// or scan mesh; the cap is a defence against malicious or corrupt
/// inputs whose upstream file-size gate is absent or set higher.
pub const MAX_MESH_ELEMENT_COUNT: usize = 200_000_000;
