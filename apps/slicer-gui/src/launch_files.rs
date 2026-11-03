/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

//! The files a launch names on its command line (#418), and how they open.
//!
//! Every model (STL, OBJ, 3MF) opens in one window, together, as Import
//! model places them (#384): `breadslicer a.stl b.stl c.stl` is one scene.
//! A project (`.bread`) is a whole scene, so each opens in a window of its
//! own, as before. A file of another type goes with the models, and the
//! window says once that it cannot open it; a file that is missing or
//! cannot be read is reported by the import, which adds the others.
//!
//! The paths are made absolute against the launch's own folder: a launch
//! that hands them to the running app ([`crate::single_instance`]) runs in
//! a folder the app does not know.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::mesh_loader::is_project;

/// The paths of `args` (the command line without the program), in order.
/// Options (`-x`, `--x`) are skipped, as the app takes none; so is an empty
/// argument.
pub(crate) fn paths_of(args: impl IntoIterator<Item = OsString>) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for raw in args {
        if raw.is_empty() {
            continue;
        }
        // A leading dash is far more likely a mistake than a real file name
        // beginning with `-`.
        if raw.to_str().is_some_and(|s| s.starts_with('-') && s != "-") {
            eprintln!("Ignoring unrecognised option: {}", raw.display());
            continue;
        }
        let path = PathBuf::from(raw);
        paths.push(std::path::absolute(&path).unwrap_or(path));
    }
    paths
}

/// The paths on this process's command line; see [`paths_of`].
pub(crate) fn from_env() -> Vec<PathBuf> {
    paths_of(std::env::args_os().skip(1))
}

/// Splits `paths` into what opens together: every model in one group, each
/// project in a group of its own. The groups keep the order in which they
/// first appear; the models keep theirs.
pub(crate) fn groups(paths: Vec<PathBuf>) -> Vec<Vec<PathBuf>> {
    let mut groups: Vec<Vec<PathBuf>> = Vec::new();
    let mut models = None;
    for path in paths {
        if is_project(&path) {
            groups.push(vec![path]);
            continue;
        }
        let slot = *models.get_or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[slot].push(path);
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn options_and_empty_arguments_are_skipped() {
        let paths = paths_of(args(&["--verbose", "", "-x", "a.stl"]));
        assert_eq!(paths.len(), 1);
        assert!(paths[0].ends_with("a.stl"));
    }

    #[test]
    fn relative_paths_become_absolute_against_the_launch_folder() {
        let paths = paths_of(args(&["models/a.stl"]));
        assert!(paths[0].is_absolute());
        assert_eq!(
            paths[0],
            std::env::current_dir().unwrap().join("models/a.stl")
        );
    }

    #[test]
    fn spaces_and_accents_stay_as_they_came() {
        let raw = if cfg!(windows) {
            r"D:\MZ\Pirate Male ñandú 海賊.stl"
        } else {
            "/MZ/Pirate Male ñandú 海賊.stl"
        };
        assert_eq!(paths_of(args(&[raw])), vec![PathBuf::from(raw)]);
    }

    #[test]
    fn every_model_opens_in_one_group() {
        let paths = ["/m/a.stl", "/m/b.OBJ", "/m/c.3mf", "/m/notes.txt"].map(PathBuf::from);
        assert_eq!(groups(paths.to_vec()), vec![paths.to_vec()]);
    }

    #[test]
    fn each_project_opens_alone_and_the_models_together() {
        let p = |s: &str| PathBuf::from(s);
        let grouped = groups(vec![p("/a.bread"), p("/x.stl"), p("/b.BREAD"), p("/y.stl")]);
        assert_eq!(
            grouped,
            vec![
                vec![p("/a.bread")],
                vec![p("/x.stl"), p("/y.stl")],
                vec![p("/b.BREAD")],
            ]
        );
    }

    #[test]
    fn nothing_named_is_no_group() {
        assert!(groups(Vec::new()).is_empty());
        assert!(paths_of(Vec::<OsString>::new()).is_empty());
    }
}
