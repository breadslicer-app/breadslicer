use std::ffi::OsString;
use std::fs::{File, copy};
use std::io::Read;
use std::path::PathBuf;

use anyhow::{Result, anyhow};

use sc_mesh_formats::{
    SourceMeshFileType, infer_mesh_file_format_from_extension, read_stl, read_stla, read_stlb,
};

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
        return Err(anyhow!(
            "Source file path and target file path are the same"
        ));
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
            Err(err) => Err(err.into()),
        };
    }

    let mut source_file = File::open(&source_path)?;

    let source_mesh = match source_format {
        SourceMeshFileType::Stl => read_stl(&mut source_file)?,
        SourceMeshFileType::StlAscii => read_stla(&mut source_file)?,
        SourceMeshFileType::StlBin => read_stlb(&mut source_file)?,
        _ => todo!("Not implemented yet!"),
    };

    // TODO: Do the actual work

    Err(anyhow!("Unable to convert file"))
}
