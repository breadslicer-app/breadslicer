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
/// - canonical path equality, catching every spelling variant of the
///   same path (relative-vs-absolute, embedded `.`/`..`, symlinked
///   intermediate directory);
/// - `symlink_metadata` on the target, rejecting a symlink planted at
///   the target between [`validate_targetfile_path`] and now;
/// - file-identity equality of source and target, catching hard-link
///   aliasing that path canonicalisation cannot see. The identity is
///   `(dev, ino)` on Unix and `(volume serial, file id)` on Windows;
///   on other targets this guard is absent.
pub(crate) fn ensure_distinct_files(source: &Path, target: &Path) -> Result<()> {
    if source == target {
        return Err(anyhow!(
            "Source file path and target file path are the same"
        ));
    }

    let target_meta = match std::fs::symlink_metadata(target) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
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

    #[cfg(any(unix, windows))]
    {
        let source_id = read_file_identity(source)?;
        let target_id = read_file_identity(target)?;
        if source_id == target_id {
            return Err(anyhow!(
                "Source and target paths refer to the same underlying file"
            ));
        }
    }

    Ok(())
}

#[cfg(any(unix, windows))]
#[derive(Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    volume: u64,
    file_id: u64,
}

#[cfg(unix)]
fn read_file_identity(path: &Path) -> Result<FileIdentity> {
    use std::os::unix::fs::MetadataExt;
    let m = path
        .metadata()
        .map_err(|e| anyhow!("Unable to stat \"{}\": {}", path.display(), e))?;
    Ok(FileIdentity {
        volume: m.dev(),
        file_id: m.ino(),
    })
}

#[cfg(windows)]
fn read_file_identity(path: &Path) -> Result<FileIdentity> {
    use std::fs::File;
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    let file = File::open(path).map_err(|e| {
        anyhow!(
            "Unable to open \"{}\" for identity check: {}",
            path.display(),
            e
        )
    })?;
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) };
    if ok == 0 {
        return Err(anyhow!(
            "GetFileInformationByHandle failed for \"{}\": {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    let file_id = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    Ok(FileIdentity {
        volume: u64::from(info.dwVolumeSerialNumber),
        file_id,
    })
}
