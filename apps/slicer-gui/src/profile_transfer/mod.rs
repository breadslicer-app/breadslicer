/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Import and export of profile packages from the Preferences window, mirroring
//! the CLI's `profiles import` / `profiles export`.
//!
//! This module holds the synchronous, window-independent core: turning a page's
//! export into an [`ExportSelection`], writing a package, reading one back
//! (surfacing collisions rather than failing), and deciding whether the user
//! confirmed an overwrite. The Preferences window drives these from `rfd` file
//! dialogs and a native confirm dialog; both operate on the user library,
//! exactly like the CLI.

use std::fs::File;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use rfd::MessageDialogResult;
use sc_print_profiles::{
    Collision, CollisionReport, ExportManifest, ExportSelection, ImportError, export_package,
    import_package,
};

use crate::i18n::ctr;

/// Which kind of profile a page's "Export ..." action packages. The page
/// implies the scope, so no separate selector is needed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExportScope {
    Printers,
    Materials,
}

impl ExportScope {
    pub(crate) fn selection(self) -> ExportSelection {
        match self {
            ExportScope::Printers => ExportSelection::Printers,
            ExportScope::Materials => ExportSelection::Materials,
        }
    }

    /// Default file name to seed the save dialog with.
    pub(crate) fn default_file_name(self) -> &'static str {
        match self {
            ExportScope::Printers => "printers.zip",
            ExportScope::Materials => "materials.zip",
        }
    }
}

/// Outcome of an import attempt that did not hard-fail: either the package
/// installed, or it would overwrite existing profiles and `force` was not set.
pub(crate) enum ImportOutcome {
    Installed,
    Collisions(CollisionReport),
}

/// Package the selected profiles from `library_dir` into `dest`.
pub(crate) fn run_export(
    library_dir: &Path,
    selection: &ExportSelection,
    dest: &Path,
) -> Result<()> {
    let file = File::create(dest)
        .with_context(|| format!("could not create the output archive {}", dest.display()))?;
    let manifest = ExportManifest::stamped_now(env!("CARGO_PKG_VERSION"));
    export_package(library_dir, selection, &manifest, file)
        .map_err(|e| anyhow!("could not export the profile package: {e}"))
}

/// Install the package at `src` into `library_dir`. A collision without `force`
/// is returned as [`ImportOutcome::Collisions`] rather than an error, so the
/// caller can offer to overwrite.
pub(crate) fn run_import(library_dir: &Path, src: &Path, force: bool) -> Result<ImportOutcome> {
    let file = File::open(src)
        .with_context(|| format!("could not open the package archive {}", src.display()))?;
    match import_package(file, library_dir, force) {
        Ok(()) => Ok(ImportOutcome::Installed),
        Err(ImportError::Collisions(report)) => Ok(ImportOutcome::Collisions(report)),
        Err(other) => Err(anyhow!("could not import the profile package: {other}")),
    }
}

/// Render a collision report for the confirm dialog: a headline plus a per-kind
/// list of the profiles that would be overwritten, in the user's language. The
/// profile ids stay verbatim.
pub(crate) fn format_collision_report(report: &CollisionReport) -> String {
    let mut out = ctr!(
        "PreferencesPage",
        "Importing this package would overwrite existing profiles:"
    )
    .to_string();
    append_conflict_section(
        &mut out,
        &ctr!("PreferencesPage", "Printers"),
        &report.printers,
    );
    append_conflict_section(
        &mut out,
        &ctr!("PreferencesPage", "Materials"),
        &report.materials,
    );
    append_conflict_section(
        &mut out,
        &ctr!("PreferencesPage", "Material extensions"),
        &report.material_extensions,
    );
    out
}

fn append_conflict_section(out: &mut String, header: &str, collisions: &[Collision]) {
    if collisions.is_empty() {
        return;
    }
    out.push_str("\n\n");
    out.push_str(header);
    out.push(':');
    for collision in collisions {
        out.push_str(&format!("\n  - {}", collision.id));
    }
}

/// Whether the user picked the overwrite button in the confirm dialog. `rfd`'s
/// `OkCancelCustom(ok, cancel)` returns `Custom(ok)` for the affirmative
/// button; anything else (the cancel button, a closed dialog) declines, so the
/// safe default is to not overwrite.
pub(crate) fn overwrite_confirmed(result: &MessageDialogResult, overwrite_label: &str) -> bool {
    matches!(result, MessageDialogResult::Custom(label) if label == overwrite_label)
}

#[cfg(test)]
mod tests;
