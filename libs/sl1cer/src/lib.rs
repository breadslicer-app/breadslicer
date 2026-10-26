/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Slicing engine for resin 3D printers.
//!
//! `sc-sl1cer` turns validated 3D meshes from [`sc_mesh_core`] into
//! per-layer geometry suitable for a resin printer. The crate is
//! intentionally narrow:
//!
//! - No file I/O. Mesh formats live in `sc-mesh-formats`; printer
//!   file formats will live in the planned `sc-sla-formats` crate.
//! - No support-structure generation (planned `sc-supports` crate).
//! - No rendering or GPU types.
//!
//! # Conventions
//!
//! Same as [`sc_mesh_core`]: right-handed, Z-up, millimetres. The
//! build platform sits at Z = 0; layers are slabs of constant
//! thickness along +Z.

pub mod api;
pub mod contour;
pub mod error;
pub mod geom2d;
pub mod grouping;
pub mod mesh_slice;
pub mod plane_intersect;
pub mod profile;
pub mod raster;
mod shells;
pub mod stitch;
mod z_bins;

pub use api::{
    CancelToken, LeafSlices, SliceOutcome, SliceProgress, slice_leaf, slice_scene,
    slice_scene_with_progress, sliceable_leaves,
};

/// This crate's package version. Surfaced so downstream tooling can
/// stamp the slicer's identity into its output (e.g. SLA file
/// metadata, debug-format `<metadata>` blocks).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests_mesh_slice;
#[cfg(test)]
mod tests_slice_scene;
