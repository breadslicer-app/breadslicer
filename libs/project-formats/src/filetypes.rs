/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Project file formats and extension-based inference.

use std::fmt;
use std::path::{Path, PathBuf};

/// File formats [`crate::read_project`] / [`crate::write_project`]
/// dispatch on. One variant per supported project container; other
/// formats (DragonFruit's VOXL, for instance) become new variants
/// with their own submodules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectFileType {
    ThreeMf,
}

/// Returned by [`infer_project_file_type_from_extension`] when the
/// path's extension names no supported project format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferProjectFileTypeError {
    UnknownExtension { path: PathBuf },
}

impl fmt::Display for InferProjectFileTypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownExtension { path } => write!(
                f,
                "cannot infer a project file format from the extension of {}",
                path.display(),
            ),
        }
    }
}

impl std::error::Error for InferProjectFileTypeError {}

/// Picks a [`ProjectFileType`] from a path's extension
/// (case-insensitive).
pub fn infer_project_file_type_from_extension(
    path: &Path,
) -> Result<ProjectFileType, InferProjectFileTypeError> {
    let extension = match path.extension() {
        Some(ext) => ext.to_str().unwrap_or(""),
        None => "",
    }
    .to_lowercase();

    match extension.as_str() {
        "3mf" => Ok(ProjectFileType::ThreeMf),
        _ => Err(InferProjectFileTypeError::UnknownExtension {
            path: path.to_path_buf(),
        }),
    }
}
