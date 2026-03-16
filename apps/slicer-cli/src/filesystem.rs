/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

use std::path::PathBuf;

use anyhow::{Result, anyhow};

pub(crate) fn validate_sourcefile_path(path: PathBuf) -> Result<PathBuf> {
    if !path.exists() {
        return Err(anyhow!(
            "The specified source file \"{:?}\" does not exist.",
            path
        ));
    }

    let original_path = path.clone();
    let mut path = path;
    let mut jumps_count = 0;
    while path.is_symlink() {
        if jumps_count >= 8 {
            return Err(anyhow!(
                "Too many steps to resolve symbolic link for source file \"{:?}\"",
                original_path
            ));
        }

        path = match path.read_link() {
            Ok(p) => p,
            Err(_) => {
                return Err(anyhow!(
                    "Unable to follow symlink for source file \"{:?}\"",
                    original_path
                ));
            }
        };
        jumps_count += 1;
    }

    if !path.is_file() {
        return Err(anyhow!(
            "Specified path for source file \"{:?}\" points to a directory.",
            path
        ));
    }

    Ok(path)
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
