use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::{Result, anyhow};

use sc_mesh_formats::{SourceMeshFileType, infer_mesh_file_format_from_extension};

use crate::filesystem::{validate_sourcefile_path, validate_targetfile_path};

pub fn convert(
    source: OsString,
    source_format: Option<SourceMeshFileType>,
    target: OsString,
    target_format: Option<SourceMeshFileType>,
) -> Result<()> {
    let source_path = validate_sourcefile_path(PathBuf::from(source))?;
    let target_path = validate_targetfile_path(PathBuf::from(target))?;

    let source_format = match source_format {
        Some(f) => f,
        None => infer_mesh_file_format_from_extension(&source_path)?,
    };
    let target_format = match target_format {
        Some(f) => f,
        None => infer_mesh_file_format_from_extension(&target_path)?,
    };

    Err(anyhow!("Unable to convert file"))
}
