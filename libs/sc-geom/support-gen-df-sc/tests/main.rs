/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Every integration test of this crate, in one test binary: each file in
//! `tests/` is a module here, so the crate and its dependencies are linked once
//! for all of them instead of once per file. `autotests = false` in
//! `Cargo.toml` keeps Cargo from also building each file on its own, and
//! `every_test_file_is_a_module` fails when a new file is not listed below.

mod bench_support_job;
mod df_support_document;
mod df_support_follow;
mod df_supports_scene;

/// A file in `tests/` that is not declared above would never be compiled or
/// run, because `autotests = false`.
#[test]
fn every_test_file_is_a_module() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let main = std::fs::read_to_string(dir.join("main.rs")).expect("read tests/main.rs");
    let mut missing = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("read tests/") {
        let path = entry.expect("read tests/ entry").path();
        if path.extension().is_none_or(|ext| ext != "rs") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if stem != "main" && !main.contains(&format!("\nmod {stem};")) {
            missing.push(format!("tests/{stem}.rs"));
        }
    }
    assert!(
        missing.is_empty(),
        "{missing:?} would not run: declare each one with `mod <name>;` in tests/main.rs"
    );
}
