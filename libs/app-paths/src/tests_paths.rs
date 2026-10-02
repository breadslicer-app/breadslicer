/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::fs;
use std::path::Path;

#[cfg(target_os = "macos")]
use std::path::{Component, PathBuf};

use tempfile::tempdir;

use super::{APP_ID, all_app_dirs, bundled_data_dirs, home_dir, project_dirs};

#[cfg(not(target_os = "linux"))]
use super::exe_relative_bases;

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
fn app_id_names_the_shipped_desktop_entry() {
    // A window's `app_id` is paired with the launcher by this file's
    // basename, so a rename on either side breaks the pairing without
    // any build or run time complaint.
    let entry = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/shared")
        .join(format!("{APP_ID}.desktop"));
    assert!(entry.is_file(), "no desktop entry at {entry:?}");
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

#[cfg(target_os = "macos")]
#[test]
fn exe_relative_bases_put_the_bundle_resources_first() {
    let macos_dir = Path::new("/Applications/Slicer.app/Contents/MacOS");
    assert_eq!(
        exe_relative_bases(macos_dir),
        vec![
            PathBuf::from("/Applications/Slicer.app/Contents/Resources"),
            PathBuf::from("/Applications/Slicer.app/Contents/MacOS"),
            PathBuf::from("/Applications/Slicer.app/Contents/MacOS/share"),
        ],
    );
}

#[cfg(target_os = "macos")]
#[test]
fn exe_relative_bases_hold_no_parent_components() {
    // `Resources` is reached through the executable directory's parent rather
    // than by appending `..`. A base carrying `..` resolves just the same, but
    // it then travels into every profile path the loader reports back to the
    // user.
    for base in exe_relative_bases(Path::new("/Applications/Slicer.app/Contents/MacOS")) {
        assert!(
            !base.components().any(|c| matches!(c, Component::ParentDir)),
            "base should carry no `..`: {base:?}",
        );
    }
}

#[cfg(target_os = "macos")]
#[test]
fn exe_relative_bases_tolerate_an_exe_dir_without_a_parent() {
    // A binary at the filesystem root has no `Contents` above it, so only the
    // executable-relative bases apply.
    assert_eq!(
        exe_relative_bases(Path::new("/")),
        vec![PathBuf::from("/"), PathBuf::from("/share")],
    );
}

#[cfg(target_os = "macos")]
#[test]
fn bundle_resources_outrank_a_data_dir_beside_the_binary() {
    let root = tempdir().unwrap();
    let contents = root.path().join("Slicer.app/Contents");
    let macos_dir = contents.join("MacOS");
    fs::create_dir_all(macos_dir.join("slicer")).unwrap();
    fs::create_dir_all(contents.join("Resources/slicer")).unwrap();

    let found = all_app_dirs(&exe_relative_bases(&macos_dir), "slicer");
    assert_eq!(
        found,
        vec![contents.join("Resources/slicer"), macos_dir.join("slicer")],
    );
}

#[cfg(all(not(target_os = "linux"), not(target_os = "macos")))]
#[test]
fn exe_relative_bases_are_the_binary_dir_and_its_share_sibling() {
    let exe_dir = Path::new(r"C:\Program Files\SoulCrafted Slicer");
    assert_eq!(
        exe_relative_bases(exe_dir),
        vec![exe_dir.to_path_buf(), exe_dir.join("share")],
    );
}
