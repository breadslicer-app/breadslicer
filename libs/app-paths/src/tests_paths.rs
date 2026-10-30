/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::fs;
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
use std::path::Component;

use tempfile::tempdir;

use super::{
    APP_ID, all_app_dirs, bundled_data_dirs, bundled_data_dirs_from, dev_assets_dir, home_dir,
    migrate_dir, project_dirs,
};

#[cfg(target_os = "linux")]
use super::data_search_bases;

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
fn bundled_data_dirs_end_with_the_source_tree_assets() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the crate sits 2 levels below the workspace root");
    let last = bundled_data_dirs()
        .pop()
        .expect("the source tree has assets/");
    assert_eq!(
        fs::canonicalize(last).unwrap(),
        fs::canonicalize(workspace.join("assets")).unwrap()
    );
}

#[test]
fn bundled_data_dirs_from_put_the_source_tree_root_last() {
    let first = tempdir().unwrap();
    let second = tempdir().unwrap();
    let dev = tempdir().unwrap();
    let roots = vec![first.path().to_path_buf(), second.path().to_path_buf()];
    assert_eq!(
        bundled_data_dirs_from(roots, Some(dev.path().to_path_buf())),
        [first.path(), second.path(), dev.path()],
    );
}

#[test]
fn bundled_data_dirs_from_leave_out_a_missing_source_tree_root() {
    let installed = tempdir().unwrap();
    let roots = vec![installed.path().to_path_buf()];
    assert_eq!(bundled_data_dirs_from(roots, None), [installed.path()]);
}

#[test]
fn dev_assets_dir_walks_up_to_the_tree_with_profiles() {
    let tree = tempdir().unwrap();
    fs::create_dir_all(tree.path().join("assets/profiles")).unwrap();
    let nested = tree.path().join("libs/sc-geom/support-gen-df");
    fs::create_dir_all(&nested).unwrap();
    assert_eq!(
        dev_assets_dir(Some(nested)),
        Some(tree.path().join("assets"))
    );
}

#[test]
fn dev_assets_dir_skips_an_assets_folder_without_profiles() {
    let tree = tempdir().unwrap();
    fs::create_dir_all(tree.path().join("assets/profiles")).unwrap();
    let app = tree.path().join("apps/icon-gen");
    fs::create_dir_all(app.join("assets")).unwrap();
    assert_eq!(dev_assets_dir(Some(app)), Some(tree.path().join("assets")));
}

#[test]
fn dev_assets_dir_is_none_outside_cargo() {
    assert_eq!(dev_assets_dir(None), None);
}

#[test]
fn dev_assets_dir_is_none_for_a_deleted_tree() {
    let tree = tempdir().unwrap();
    let gone = tree.path().join("worktrees/deleted/apps/slicer-gui");
    assert_eq!(dev_assets_dir(Some(gone)), None);
}

#[test]
fn dev_assets_dir_ignores_a_relative_manifest_dir() {
    assert_eq!(dev_assets_dir(Some(PathBuf::from("libs/app-paths"))), None);
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
fn project_dirs_resolve_exactly_when_the_home_dir_does() {
    assert_eq!(project_dirs().is_some(), home_dir().is_some());
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
            data.contains("breadslicer"),
            "data dir should embed the app or org identifier: {data}",
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn data_search_bases_keep_the_order_of_xdg_data_dirs() {
    assert_eq!(
        data_search_bases(Some("/opt/share:/usr/share".into())),
        [Path::new("/opt/share"), Path::new("/usr/share")],
    );
}

#[cfg(target_os = "linux")]
#[test]
fn data_search_bases_default_to_the_spec_dirs_when_xdg_data_dirs_is_unset() {
    assert_eq!(
        data_search_bases(None),
        [Path::new("/usr/local/share"), Path::new("/usr/share")],
    );
}

#[cfg(target_os = "linux")]
#[test]
fn data_search_bases_default_to_the_spec_dirs_when_xdg_data_dirs_is_empty() {
    assert_eq!(
        data_search_bases(Some("".into())),
        [Path::new("/usr/local/share"), Path::new("/usr/share")],
    );
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

#[test]
fn a_legacy_dir_is_copied_once_and_kept() {
    let root = tempdir().unwrap();
    let old = root.path().join("old");
    fs::create_dir_all(old.join("logs")).unwrap();
    fs::write(old.join("settings.toml"), "aa_level = \"x4\"").unwrap();
    fs::write(old.join("logs").join("session.log"), "hello").unwrap();
    let new = root.path().join("new").join("config");

    assert!(migrate_dir(&old, &new));
    assert_eq!(
        fs::read_to_string(new.join("settings.toml")).unwrap(),
        "aa_level = \"x4\""
    );
    assert_eq!(
        fs::read_to_string(new.join("logs").join("session.log")).unwrap(),
        "hello"
    );
    // The old folder stays as a backup.
    assert!(old.join("settings.toml").is_file());
    // A second start finds the new folder and leaves it alone.
    fs::write(new.join("settings.toml"), "newer").unwrap();
    assert!(!migrate_dir(&old, &new));
    assert_eq!(
        fs::read_to_string(new.join("settings.toml")).unwrap(),
        "newer"
    );
}

#[test]
fn nothing_to_migrate_without_a_legacy_dir() {
    let root = tempdir().unwrap();
    assert!(!migrate_dir(
        &root.path().join("missing"),
        &root.path().join("new")
    ));
    assert!(!root.path().join("new").exists());
}
