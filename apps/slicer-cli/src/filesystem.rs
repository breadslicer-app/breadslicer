/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

use std::path::{Path, PathBuf};

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
            "Specified path for target file \"{}\" already points to a directory or symlink",
            path.display()
        ));
    }

    // The target file itself need not exist (we are about to create it),
    // so we cannot canonicalise the full path. We canonicalise the
    // parent directory and re-join the final name: that resolves any
    // intermediate symlinked directory and produces a single absolute
    // spelling, so the downstream `ensure_distinct_files` check sees
    // both source and target in the same form.
    let file_name = path.file_name().ok_or_else(|| {
        anyhow!(
            "Specified path for target file \"{}\" has no file name component.",
            path.display()
        )
    })?;
    let parent_canon = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.canonicalize().map_err(|e| {
            anyhow!(
                "Unable to resolve parent directory \"{}\" of target file \"{}\": {}",
                parent.display(),
                path.display(),
                e
            )
        })?,
        _ => std::env::current_dir().map_err(|e| {
            anyhow!(
                "Unable to resolve current working directory while validating target \"{}\": {}",
                path.display(),
                e
            )
        })?,
    };

    Ok(parent_canon.join(file_name))
}

/// Refuses to proceed when `source` and `target` resolve to the same
/// underlying file. Both paths are expected to come from
/// [`validate_sourcefile_path`] / [`validate_targetfile_path`] (i.e.
/// canonicalised, or parent-canonicalised, respectively).
///
/// Three checks, in order:
/// - canonical path equality, which catches every spelling variant of
///   the same path (relative-vs-absolute, embedded `.`/`..`, symlinked
///   intermediate directory),
/// - on Unix, `symlink_metadata` on the target: a symlink planted at
///   the target between [`validate_targetfile_path`] and now is
///   rejected here even though the atomic-rename writer would replace
///   it harmlessly, because surprising the user is bad UX,
/// - on Unix, `(dev, ino)` equality of source and target metadata,
///   which catches the hard-link-aliasing case that no amount of path
///   canonicalisation can see.
pub(crate) fn ensure_distinct_files(source: &Path, target: &Path) -> Result<()> {
    if source == target {
        return Err(anyhow!(
            "Source file path and target file path are the same"
        ));
    }
    #[cfg(unix)]
    {
        use std::io::ErrorKind;
        use std::os::unix::fs::MetadataExt;

        let target_meta = match std::fs::symlink_metadata(target) {
            Ok(m) => m,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                return Err(anyhow!(
                    "Unable to stat target file \"{}\": {}",
                    target.display(),
                    e
                ));
            }
        };
        if target_meta.file_type().is_symlink() {
            return Err(anyhow!(
                "Target file path \"{}\" is a symbolic link; refusing to follow it.",
                target.display()
            ));
        }
        let source_meta = source
            .metadata()
            .map_err(|e| anyhow!("Unable to stat source file \"{}\": {}", source.display(), e))?;
        if target_meta.dev() == source_meta.dev() && target_meta.ino() == source_meta.ino() {
            return Err(anyhow!(
                "Source and target paths refer to the same underlying file"
            ));
        }
    }
    Ok(())
}
