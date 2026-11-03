/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Every integration test of this crate, in one test binary: each file in
//! `tests/` is a module here, so the crate and its dependencies are linked once
//! for all of them instead of once per file. `autotests = false` in
//! `Cargo.toml` keeps Cargo from also building each file on its own, and
//! `every_test_file_is_a_module` fails when a new file is not listed below.

mod df_analytics;

mod bench_real_model;
mod builders5_df_tests;
mod df_parity_autobrace;
mod df_parity_autosupport_a;
mod df_parity_autosupport_b;
mod df_parity_autosupport_c;
mod df_parity_builders;
mod df_parity_builders5;
mod df_parity_clipper;
mod df_parity_extrude;
mod df_parity_islands_b;
mod df_parity_islands_c;
mod df_parity_native_islands;
mod df_parity_primitives;
mod df_parity_rafts;
mod df_parity_sdf;
mod df_parity_support_triangles;
mod df_parity_tip_offset;
mod df_parity_v3;
mod js_math;
mod pipeline;
mod smooth_shafts;
mod support_primitives;
mod support_type_registry;
mod wolverine_sdf_scale;

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
