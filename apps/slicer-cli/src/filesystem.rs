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

// Properties of the identity read that hold on every platform with an
// identity impl. They are gated on `any(unix, windows)` (not `windows`)
// because the asserted behaviour is not Windows-specific; the reason to
// run them at all is that the Windows path computes the id by hand
// (`GetFileInformationByHandle` plus a manual high/low reconstruction),
// so this is the only guard on that arithmetic. On Unix they also guard
// the `dev`/`ino` read.
#[cfg(all(test, any(unix, windows)))]
mod identity_tests {
    use super::*;

    // Two different byte-spellings of the same file must resolve to the
    // same identity: the read identifies the file, not the path string.
    #[test]
    fn same_file_via_two_spellings_has_equal_identity() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        // `sub/../a.txt` reaches the same file through a distinct spelling.
        let alt = dir.path().join("sub").join("..").join("a.txt");

        let direct = read_file_identity(&file).unwrap();
        let detour = read_file_identity(&alt).unwrap();
        assert!(direct == detour, "same file must have equal identity");
    }

    // Two separate files on the same volume must differ, and must differ
    // in `file_id` specifically (same volume serial). Guards against a
    // collapsed high/low reconstruction that the equality test alone
    // would not catch.
    #[test]
    fn distinct_files_have_distinct_file_ids() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(&b, b"b").unwrap();

        let id_a = read_file_identity(&a).unwrap();
        let id_b = read_file_identity(&b).unwrap();
        assert!(id_a != id_b, "distinct files must have distinct identity");
        assert_eq!(
            id_a.volume, id_b.volume,
            "two files in one tempdir share a volume"
        );
        assert_ne!(id_a.file_id, id_b.file_id);
    }
}

// Behaviour of the identity read that genuinely diverges on Windows and
// would not hold on Unix: the error text comes from the Windows-only
// `File::open` arm, and opening a directory as a file fails on Windows
// where it succeeds on Unix.
#[cfg(all(test, windows))]
mod windows_identity_tests {
    use super::*;

    // A nonexistent path fails at `File::open`, surfacing the identity
    // check's own error message (the Unix impl reads metadata and says
    // "Unable to stat" instead).
    #[test]
    fn nonexistent_path_errors_at_open() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.txt");
        let err = read_file_identity(&missing)
            .err()
            .expect("opening a missing file must fail");
        assert!(
            err.to_string().contains("for identity check"),
            "unexpected error: {err}"
        );
    }

    // On Windows a directory cannot be opened as a `File` (Rust does not
    // set FILE_FLAG_BACKUP_SEMANTICS), so identity of a directory path is
    // an error. On Unix the same call would succeed; the CLI never feeds
    // a directory here because `validate_sourcefile_path` rejects it via
    // `is_file()` first. This is a contract test for the divergence.
    #[test]
    fn directory_path_errors_on_windows() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_file_identity(dir.path()).is_err());
    }
}
