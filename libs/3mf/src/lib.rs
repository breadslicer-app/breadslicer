/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! 3MF (3D Manufacturing Format) vocabulary crate.
//!
//! `sc-3mf` owns the two format-side layers of 3MF support:
//!
//! - [`package`]: the OPC (Open Packaging Conventions) container, a
//!   ZIP archive with a content-type map and relationship parts.
//! - [`model`]: the typed document model for the 3D Model part and
//!   its XML reading / writing.
//!
//! The crate deliberately knows NOTHING about this workspace's mesh
//! or slicing types: conversions between domain types (`Scene`,
//! contour stacks) and 3MF documents live in the format adapter
//! crates that depend on this one. Coordinates in documents are
//! plain `f64` in the unit declared by the model element; unit
//! conversion is the adapter's job.

pub mod error;
pub mod model;
pub mod names;
pub mod package;

#[cfg(test)]
mod tests_model;
#[cfg(test)]
mod tests_package;
#[cfg(test)]
mod tests_slice;
