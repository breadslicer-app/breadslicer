/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::fs;

use tempfile::tempdir;

use super::{all_app_dirs, bundled_data_dir, bundled_data_dirs, home_dir, project_dirs};

#[test]
fn all_app_dirs_returns_every_existing_base_in_order() {
    let first = tempdir().unwrap();
    let second = tempdir().unwrap();
    fs::create_dir(first.path().join("soulcrafted-slicer")).unwrap();
    fs::create_dir(second.path().join("soulcrafted-slicer")).unwrap();

    let bases = vec![first.path().to_path_buf(), second.path().to_path_buf()];
    let found = all_app_dirs(&bases, "soulcrafted-slicer");
    assert_eq!(
        found,
        vec![
            first.path().join("soulcrafted-slicer"),
            second.path().join("soulcrafted-slicer"),
        ],
    );
}

#[test]
fn all_app_dirs_skips_bases_without_the_app_dir() {
    let empty = tempdir().unwrap();
    let populated = tempdir().unwrap();
    fs::create_dir(populated.path().join("soulcrafted-slicer")).unwrap();

    let bases = vec![empty.path().to_path_buf(), populated.path().to_path_buf()];
    let found = all_app_dirs(&bases, "soulcrafted-slicer");
    assert_eq!(found, vec![populated.path().join("soulcrafted-slicer")]);
}

#[test]
fn all_app_dirs_is_empty_when_no_base_has_the_app_dir() {
    let only = tempdir().unwrap();
    let bases = vec![only.path().to_path_buf()];
    assert!(all_app_dirs(&bases, "soulcrafted-slicer").is_empty());
}

#[test]
fn all_app_dirs_ignores_a_file_with_the_app_name() {
    // A regular file with the app name is not a directory match.
    let base = tempdir().unwrap();
    fs::write(base.path().join("soulcrafted-slicer"), b"not a dir").unwrap();
    let bases = vec![base.path().to_path_buf()];
    assert!(all_app_dirs(&bases, "soulcrafted-slicer").is_empty());
}

#[test]
fn bundled_data_dir_is_absolute() {
    assert!(bundled_data_dir().is_absolute());
}

#[test]
fn bundled_data_dirs_are_all_absolute() {
    assert!(bundled_data_dirs().iter().all(|p| p.is_absolute()));
}

#[test]
fn home_dir_is_absolute_when_resolved() {
    // Environments without a home directory resolve to `None`; when one
    // is available it must be an absolute path.
    if let Some(home) = home_dir() {
        assert!(home.is_absolute(), "home dir should be absolute: {home:?}");
    }
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
