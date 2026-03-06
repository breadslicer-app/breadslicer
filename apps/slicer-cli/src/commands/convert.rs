use std::ffi::OsString;
use std::fs::copy;
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

    if source_path == target_path {
        return Err(anyhow!("Source file path and target file path are the same"));
    }

    let source_format = match source_format {
        Some(f) => f,
        None => infer_mesh_file_format_from_extension(&source_path)?,
    };
    let target_format = match target_format {
        Some(f) => f,
        None => infer_mesh_file_format_from_extension(&target_path)?,
    };

    if source_format == target_format {
        eprintln!("Warning: copying file without performing any format conversion");

        return match copy(&source_path, &target_path) {
            Ok(_) => Ok(()),
            Err(err) => Err(err.into())
        }
    }

    // TODO: Do the actual work

    Err(anyhow!("Unable to convert file"))
}
