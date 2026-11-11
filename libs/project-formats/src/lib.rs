/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Read & write support for slicer project files.
//!
//! A project file carries a scene together with project-level data the plain
//! mesh formats cannot hold: which printer and resin profiles the project uses,
//! and (optionally) embedded copies of those profile documents so the file
//! stays portable across machines.
//!
//! [`ProjectPayload`] and the warning enums say nothing about how a given file
//! format stores them. The container specifics live in 1 submodule per format;
//! [`threemf`] stores the payload in custom OPC parts of a 3MF package, and
//! [`bread`] in the settings chunk of Breadslicer's `.bread` file.
//! [`read_project`] / [`write_project`] pick the submodule from a
//! [`ProjectFileType`].
//!
//! Applications load any supported scene file (plain mesh or project) through
//! the function [`load_scene`], which picks the reader from the path's
//! extension, enforces the file-size cap, and reports byte progress on both
//! routes.

pub mod bread;
mod bread_file;
pub mod error;
pub mod filetypes;
pub mod loader;
pub mod messages;
pub mod payload;
mod progress_reader;
pub mod read;
pub mod slices;
pub mod threemf;
pub mod write;

#[cfg(test)]
mod tests_bread;
#[cfg(test)]
mod tests_bread_file;
#[cfg(test)]
mod tests_error;
#[cfg(test)]
mod tests_filetypes;
#[cfg(test)]
mod tests_loader;
#[cfg(test)]
mod tests_payload;

pub use sc_3mf::package::DecompressionProgress;

pub use bread::{BREAD_SETTINGS_VERSION, read_bread_project, write_bread_project_with_progress};
pub use bread_file::{
    HollowDocument, SURFACE_DOCUMENT_VERSION, SupportDocument, SurfaceNote, read_bread_thumbnail,
};
pub use error::{ReadProjectError, WriteProjectError};
pub use filetypes::{
    InferProjectFileTypeError, ProjectFileType, infer_project_file_type_from_extension,
};
pub use loader::{
    LoadProgress, LoadSceneError, LoadedContent, LoadedScene, PersistedSlices, ProjectContent,
    SUPPORTED_SCENE_FILE_EXTENSIONS, is_supported_scene_file_extension, load_scene,
};
pub use messages::{ForeignDialect, ProfileRefWarning, ReadProjectWarning};
pub use payload::{EmbedProfiles, EmbeddedProfile, ProjectPayload, ReviewDecision};
pub use read::{LoadedProject, read_project, read_project_with_progress};
pub use slices::PlateSlices;
pub use write::progress::WriteProgress;
pub use write::{write_project, write_project_with_progress};
