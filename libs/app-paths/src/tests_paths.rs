/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::fs;

use tempfile::tempdir;

use super::{bundled_data_dir, find_app_dir, project_dirs};

#[test]
fn find_app_dir_returns_the_first_existing_base() {
    let first = tempdir().unwrap();
    let second = tempdir().unwrap();
    fs::create_dir(first.path().join("soulcrafted-slicer")).unwrap();
    fs::create_dir(second.path().join("soulcrafted-slicer")).unwrap();

    let bases = vec![first.path().to_path_buf(), second.path().to_path_buf()];
    let found = find_app_dir(&bases, "soulcrafted-slicer").unwrap();
    assert_eq!(found, first.path().join("soulcrafted-slicer"));
}

#[test]
fn find_app_dir_skips_bases_without_the_app_dir() {
    let empty = tempdir().unwrap();
    let populated = tempdir().unwrap();
    fs::create_dir(populated.path().join("soulcrafted-slicer")).unwrap();

    let bases = vec![empty.path().to_path_buf(), populated.path().to_path_buf()];
    let found = find_app_dir(&bases, "soulcrafted-slicer").unwrap();
    assert_eq!(found, populated.path().join("soulcrafted-slicer"));
}

#[test]
fn find_app_dir_returns_none_when_no_base_has_the_app_dir() {
    let only = tempdir().unwrap();
    let bases = vec![only.path().to_path_buf()];
    assert!(find_app_dir(&bases, "soulcrafted-slicer").is_none());
}

#[test]
fn find_app_dir_ignores_a_file_with_the_app_name() {
    // A regular file with the app name is not a directory match.
    let base = tempdir().unwrap();
    fs::write(base.path().join("soulcrafted-slicer"), b"not a dir").unwrap();
    let bases = vec![base.path().to_path_buf()];
    assert!(find_app_dir(&bases, "soulcrafted-slicer").is_none());
}

#[test]
fn bundled_data_dir_is_absolute() {
    assert!(bundled_data_dir().is_absolute());
}

#[test]
fn project_dirs_paths_embed_the_app_identity() {
    // When a home directory is available the resolved paths carry the
    // app or org identifier.
    if let Some(dirs) = project_dirs() {
        let data = dirs.data_dir().to_string_lossy().to_lowercase();
        assert!(
            data.contains("slicer") || data.contains("soulcrafted"),
            "data dir should embed the app or org identifier: {data}",
        );
    }
}
