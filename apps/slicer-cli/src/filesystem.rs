/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

use std::path::PathBuf;

use anyhow::{Result, anyhow};

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
            "Specified path for target file \"{:?}\" already points to a directory",
            path
        ));
    }

    Ok(path)
}
