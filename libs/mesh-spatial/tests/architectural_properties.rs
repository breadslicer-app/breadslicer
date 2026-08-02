/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Architectural property tests: enforce the layering rule of this
//! crate at CI time so a regression fails the build, not the review.
//!
//! - `mesh-core` and `mesh-formats` must not contain `OnceLock`
//!   references. Cache state belongs in `mesh-spatial`; a stray
//!   `OnceLock` in a data crate signals that derived state is being
//!   smuggled back into the data layer.
//! - `mesh-formats` and `slicer-cli` must not depend on
//!   `sc-mesh-spatial`. Their job is to stream mesh data through;
//!   they should never need a spatial cache.

use std::fs;
use std::path::{Path, PathBuf};

/// Workspace root, derived from this crate's manifest dir
/// (`libs/mesh-spatial`) by climbing two levels.
fn workspace_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .ancestors()
        .nth(2)
        .expect("workspace root sits two levels above the crate manifest")
        .to_path_buf()
}

/// Recursively walks `dir`, calling `f(path, contents)` for every
/// `.rs` file it finds. Symlinks and non-UTF-8 names are skipped.
fn walk_rust_sources(dir: &Path, f: &mut impl FnMut(&Path, &str)) {
    for entry in fs::read_dir(dir).expect("dir readable") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        let ft = entry.file_type().expect("file type");
        if ft.is_dir() {
            walk_rust_sources(&path, f);
        } else if ft.is_file()
            && path.extension().and_then(|s| s.to_str()) == Some("rs")
            && let Ok(contents) = fs::read_to_string(&path)
        {
            f(&path, &contents);
        }
    }
}

#[test]
fn mesh_core_source_contains_no_once_lock() {
    let root = workspace_root();
    let src = root.join("libs/mesh-core/src");
    let mut offenders = Vec::new();
    walk_rust_sources(&src, &mut |path, contents| {
        if contents.contains("OnceLock") {
            offenders.push(path.strip_prefix(&root).unwrap_or(path).to_path_buf());
        }
    });
    assert!(
        offenders.is_empty(),
        "`OnceLock` must not appear in `mesh-core` source files. \
         Cache state belongs in `mesh-spatial`. Offenders:\n  - {}",
        offenders
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("\n  - "),
    );
}

#[test]
fn mesh_formats_source_contains_no_once_lock() {
    let root = workspace_root();
    let src = root.join("libs/mesh-formats/src");
    let mut offenders = Vec::new();
    walk_rust_sources(&src, &mut |path, contents| {
        if contents.contains("OnceLock") {
            offenders.push(path.strip_prefix(&root).unwrap_or(path).to_path_buf());
        }
    });
    assert!(
        offenders.is_empty(),
        "`OnceLock` must not appear in `mesh-formats` source files. \
         Format readers/writers should not be initialising cache \
         state. Offenders:\n  - {}",
        offenders
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("\n  - "),
    );
}

/// `mesh-formats` is a pure data-crate dependency on `mesh-core`. It
/// must not transitively pull in spatial-cache types.
#[test]
fn mesh_formats_does_not_depend_on_mesh_spatial() {
    let root = workspace_root();
    let manifest = root.join("libs/mesh-formats/Cargo.toml");
    let contents = fs::read_to_string(&manifest).expect("read mesh-formats Cargo.toml");
    assert!(
        !contents.contains("sc-mesh-spatial"),
        "`mesh-formats` must not depend on `sc-mesh-spatial`. The \
         dependency direction is mesh-spatial → mesh-core, never \
         the other way."
    );
}

/// `slicer-cli` runs headless format conversions. It has no GUI, no
/// picker, no gizmo, and therefore no reason to pull spatial-cache
/// types into its dependency graph.
#[test]
fn slicer_cli_does_not_depend_on_mesh_spatial() {
    let root = workspace_root();
    let manifest = root.join("apps/slicer-cli/Cargo.toml");
    let contents = fs::read_to_string(&manifest).expect("read slicer-cli Cargo.toml");
    assert!(
        !contents.contains("sc-mesh-spatial"),
        "`slicer-cli` must not depend on `sc-mesh-spatial`. The CLI \
         streams mesh data through readers and writers; it does not \
         need spatial-query acceleration."
    );
}
