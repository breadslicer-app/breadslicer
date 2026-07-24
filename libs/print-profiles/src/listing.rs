/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Enumeration of the profile ids present in a library directory.
//!
//! Given a library base directory, these helpers list the printer
//! and material ids whose `.toml` files live under the `printers/`
//! and `materials/` subdirectories. A missing subdirectory is not an
//! error: it yields an empty list. Files without a `.toml` extension
//! and files whose stem is not a valid profile id are skipped, since
//! they are not profiles the by-id loaders could ever resolve. The
//! returned ids are sorted.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sc_print_primitives::{MaterialId, PrinterId};

use crate::loader::{MATERIALS_SUBDIR, PRINTERS_SUBDIR, material_extensions_dir};

/// Returned when a library subdirectory cannot be read (other than
/// it simply not existing, which yields an empty list).
#[derive(Debug)]
pub struct ListLibraryError {
    pub path: PathBuf,
    pub kind: io::Error,
}

impl fmt::Display for ListLibraryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "failed to list the profile library at {}: {}",
            self.path.display(),
            self.kind,
        )
    }
}

impl std::error::Error for ListLibraryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.kind)
    }
}

/// List the printer ids present in `<library_dir>/printers/`, sorted.
pub fn list_printer_ids(library_dir: &Path) -> Result<Vec<PrinterId>, ListLibraryError> {
    list_ids(&library_dir.join(PRINTERS_SUBDIR), |stem| {
        PrinterId::new(stem).ok()
    })
}

/// List the material ids present in `<library_dir>/materials/`,
/// sorted. Extension profiles under `materials/extensions/` are not
/// included; see [`list_material_extension_ids`].
pub fn list_material_ids(library_dir: &Path) -> Result<Vec<MaterialId>, ListLibraryError> {
    list_ids(&library_dir.join(MATERIALS_SUBDIR), |stem| {
        MaterialId::new(stem).ok()
    })
}

/// List the material ids that have an extension profile under
/// `<library_dir>/materials/extensions/`, sorted.
pub fn list_material_extension_ids(
    library_dir: &Path,
) -> Result<Vec<MaterialId>, ListLibraryError> {
    list_ids(&material_extensions_dir(library_dir), |stem| {
        MaterialId::new(stem).ok()
    })
}

fn list_ids<Id, F>(dir: &Path, parse: F) -> Result<Vec<Id>, ListLibraryError>
where
    Id: Ord,
    F: Fn(&str) -> Option<Id>,
{
    let read_dir = match fs::read_dir(dir) {
        Ok(read_dir) => read_dir,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(ListLibraryError {
                path: dir.to_owned(),
                kind: err,
            });
        }
    };

    let mut ids = Vec::new();
    for entry in read_dir {
        let entry = entry.map_err(|err| ListLibraryError {
            path: dir.to_owned(),
            kind: err,
        })?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str())
            && let Some(id) = parse(stem)
        {
            ids.push(id);
        }
    }
    ids.sort();
    Ok(ids)
}
