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
mod names;
mod payload_part;
mod reader;
pub(crate) mod slice_mapping;
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

pub use names::{
    MATERIAL_EXTENSION_PART_PREFIX, MATERIAL_PROFILE_PART_PREFIX, PRINTER_PROFILE_PART_PREFIX,
    PROFILE_PART_CONTENT_TYPE, PROFILES_PART_PREFIX, PROJECT_PART_CONTENT_TYPE, PROJECT_PART_NAME,
    PROJECT_REL_TYPE, SLICE_PART_NAME, SLICE_REL_TYPE,
};
pub use reader::{read_threemf_project, read_threemf_project_with_progress};
pub use slice_mapping::SliceMappingError;
pub use writer::{write_threemf_project, write_threemf_project_with_progress};
