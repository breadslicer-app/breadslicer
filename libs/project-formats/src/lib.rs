/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Read & write support for slicer project files.
//!
//! A project file carries a scene together with project-level data
//! the plain mesh formats cannot hold: which printer and resin
//! profiles the project uses, and (optionally) embedded copies of
//! those profile documents so the file stays portable across
//! machines.
//!
//! The crate-root types are container-neutral: [`ProjectPayload`]
//! and the warning enums say nothing about how a given file format
//! stores them. Each supported format owns a submodule with the
//! container specifics; [`threemf`] is the first one, storing the
//! payload in custom OPC parts of a 3MF package. Dispatch happens
//! through [`ProjectFileType`] in [`read_project`] /
//! [`write_project`].
//!
//! Reading is backward and sideways compatible by construction: a
//! 3MF package without our project part loads as a plain model
//! (payload `None`), and packages written by other slicers have
//! their project data detected and reported, never parsed.

pub mod error;
pub mod filetypes;
pub mod messages;
pub mod payload;
pub mod read;
pub mod slices;
pub mod threemf;
pub mod write;

#[cfg(test)]
mod tests_error;
#[cfg(test)]
mod tests_filetypes;
#[cfg(test)]
mod tests_payload;

pub use sc_3mf::package::DecompressionProgress;

pub use error::{ReadProjectError, WriteProjectError};
pub use filetypes::{
    InferProjectFileTypeError, ProjectFileType, infer_project_file_type_from_extension,
};
pub use messages::{ForeignDialect, ProfileRefWarning, ReadProjectWarning};
pub use payload::{EmbedProfiles, EmbeddedProfile, ProjectPayload};
pub use read::{LoadedProject, read_project, read_project_with_progress};
pub use slices::PlateSlices;
pub use write::{WriteProgress, write_project, write_project_with_progress};
