/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! 3MF project packages: a standard 3MF model plus custom OPC parts carrying
//! the project payload.
//!
//! Package shape on top of what `sc-mesh-formats` writes for a plain model:
//!
//! - `/Metadata/soulcrafted-project.toml`: the project part (TOML, versioned,
//!   profile references). Linked from the package root with our custom
//!   relationship type and with the OPC MustPreserve relationship, so
//!   spec-conformant editors keep it when rewriting the package.
//! - `/Metadata/profiles/printers/<id>.toml`,
//!   `/Metadata/profiles/materials/<id>.toml`,
//!   `/Metadata/profiles/materials/extensions/<id>.toml`: embedded profile
//!   documents (raw TOML bytes), mirroring the profile library layout, each
//!   also marked MustPreserve.
//!
//! A package without the project part reads as a plain model. Parts written by
//! other slicers are never parsed; known dialects are detected by their marker
//! part names and reported as warnings.

mod foreign;
mod payload_part;
mod reader;
mod slice_mapping;
mod writer;

#[cfg(test)]
mod test_fixtures;
#[cfg(test)]
mod tests_foreign;
#[cfg(test)]
mod tests_payload_part;
#[cfg(test)]
mod tests_reader;
#[cfg(test)]
mod tests_slice_mapping;
#[cfg(test)]
mod tests_writer;

pub use reader::{read_threemf_project, read_threemf_project_with_progress};
pub use slice_mapping::SliceMappingError;
pub use writer::{write_threemf_project, write_threemf_project_with_progress};

/// Name of the project part inside the package.
pub const PROJECT_PART_NAME: &str = "/Metadata/soulcrafted-project.toml";

/// Name of the slice model part inside the package. A second 3D-model part
/// carrying only the slice stack, standalone from the scene.
pub const SLICE_PART_NAME: &str = "/3D/slices.model";

/// Relationship type linking the package root to the slice part. An opaque OPC
/// identifier following the schemas.<domain> shape of the core 3MF relationship
/// types; it is not meant to resolve.
pub const SLICE_REL_TYPE: &str = "http://schemas.soulcrafted.it/slicer/2026/07/slices";

/// Content type of the project part.
pub const PROJECT_PART_CONTENT_TYPE: &str = "application/vnd.soulcrafted.project+toml";

/// Relationship type linking the package root to the project part. An opaque
/// OPC identifier following the schemas.<domain> shape of the core 3MF
/// relationship types; it is not meant to resolve.
pub const PROJECT_REL_TYPE: &str = "http://schemas.soulcrafted.it/slicer/2026/07/project";

/// Content type of embedded profile parts.
pub const PROFILE_PART_CONTENT_TYPE: &str = "application/toml";

/// Directory prefix of every embedded profile part.
pub const PROFILES_PART_PREFIX: &str = "/Metadata/profiles/";

/// Directory prefix of embedded printer profile parts.
pub const PRINTER_PROFILE_PART_PREFIX: &str = "/Metadata/profiles/printers/";

/// Directory prefix of embedded material profile parts.
pub const MATERIAL_PROFILE_PART_PREFIX: &str = "/Metadata/profiles/materials/";

/// Directory prefix of embedded material extension parts.
pub const MATERIAL_EXTENSION_PART_PREFIX: &str = "/Metadata/profiles/materials/extensions/";
