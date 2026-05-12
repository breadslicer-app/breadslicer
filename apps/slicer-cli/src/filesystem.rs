/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

use std::path::PathBuf;

use anyhow::{Result, anyhow};

/// Hard upper bound on the on-disk size of input meshes the CLI will
/// load. Matches the GUI's `MAX_MESH_FILE_SIZE_MIB` so both surfaces
/// agree on what counts as "too large". Passed straight through to
/// `sc_mesh_formats::load_mesh_from_path` as the `max_file_size`
/// argument.
pub(crate) const MAX_INPUT_FILE_SIZE_MIB: u64 = 2048;
pub(crate) const MAX_INPUT_FILE_SIZE_BYTES: u64 = MAX_INPUT_FILE_SIZE_MIB * 1024 * 1024;

pub(crate) fn validate_sourcefile_path(path: PathBuf) -> Result<PathBuf> {
    let resolved = path.canonicalize().map_err(|e| {
        anyhow!(
            "Unable to resolve source file path \"{}\": {}",
            path.display(),
            e
        )
    })?;

    if !resolved.is_file() {
        return Err(anyhow!(
            "Specified path for source file \"{}\" is not a regular file.",
            path.display()
        ));
    }

    Ok(resolved)
}

pub(crate) fn validate_targetfile_path(path: PathBuf) -> Result<PathBuf> {
    if path.is_dir() || path.is_symlink() {
        return Err(anyhow!(
            "Specified path for target file \"{}\" already points to a directory or symlink",
            path.display()
        ));
    }

    Ok(path)
}
