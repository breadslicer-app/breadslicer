/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Show in folder (#732): the system's file browser opens on a file's
//! folder with the file selected, from a model in the scene panel or a
//! recent file.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;

use crate::error_dialog::{DialogManager, ErrorTexts};
use crate::i18n::ctr;

/// What the file browser is asked to show for `path`: the file, selected,
/// while it is there; its folder when only the folder is. `None` when
/// neither is.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Shown {
    File(PathBuf),
    Folder(PathBuf),
}

/// What there is to show of `path`.
pub(crate) fn what_to_show(path: &Path) -> Option<Shown> {
    if path.is_file() {
        return Some(Shown::File(path.to_path_buf()));
    }
    path.parent()
        .filter(|folder| folder.is_dir())
        .map(|folder| Shown::Folder(folder.to_path_buf()))
}

/// The command that shows `shown` in this system's file browser.
fn command_for(shown: &Shown) -> Command {
    match shown {
        #[cfg(windows)]
        Shown::File(file) => {
            use std::os::windows::process::CommandExt;
            let mut command = Command::new("explorer");
            // Explorer wants the quotes around the path alone, after the
            // comma, which the usual quoting of an argument does not give.
            command.raw_arg(format!("/select,\"{}\"", file.display()));
            command
        }
        #[cfg(target_os = "macos")]
        Shown::File(file) => {
            let mut command = Command::new("open");
            command.arg("-R").arg(file);
            command
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        Shown::File(file) => {
            // The freedesktop file manager interface selects the file; a
            // desktop without it opens the folder.
            let mut command = Command::new("dbus-send");
            command.args([
                "--session",
                "--dest=org.freedesktop.FileManager1",
                "--type=method_call",
                "/org/freedesktop/FileManager1",
                "org.freedesktop.FileManager1.ShowItems",
            ]);
            command.arg(format!("array:string:{}", file_uri(file)));
            command.arg("string:");
            command
        }
        Shown::Folder(folder) => {
            let opener = if cfg!(windows) {
                "explorer"
            } else if cfg!(target_os = "macos") {
                "open"
            } else {
                "xdg-open"
            };
            let mut command = Command::new(opener);
            command.arg(folder);
            command
        }
    }
}

/// `path` as a `file://` address, each byte that is not plain text escaped.
#[cfg_attr(any(windows, target_os = "macos"), allow(dead_code))]
fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        if byte.is_ascii_alphanumeric() || b"/-_.~".contains(&byte) {
            uri.push(byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

/// Opens the system's file browser on `path`, selected; on its folder when
/// only the folder is there. `NotFound` when neither is.
pub(crate) fn reveal(path: &Path) -> std::io::Result<()> {
    crate::breadcrumbs::note("show in folder");
    let Some(shown) = what_to_show(path) else {
        return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
    };
    let spawned = command_for(&shown).spawn().map(drop);
    // On Linux the file manager may not take the request: the folder opens
    // instead.
    if spawned.is_err()
        && !cfg!(windows)
        && !cfg!(target_os = "macos")
        && let Shown::File(file) = &shown
        && let Some(folder) = file.parent()
    {
        return command_for(&Shown::Folder(folder.to_path_buf()))
            .spawn()
            .map(drop);
    }
    spawned
}

/// [`reveal`], saying in a dialog why it could not.
pub(crate) fn show_in_folder(dialogs: &Rc<DialogManager>, path: &Path) {
    let Err(err) = reveal(path) else {
        return;
    };
    let path = path.to_path_buf();
    let missing = err.kind() == std::io::ErrorKind::NotFound && what_to_show(&path).is_none();
    dialogs.show_error(move || ErrorTexts {
        heading: ctr!("ErrorDialog", "Cannot show the file"),
        context: Some(ctr!("ErrorDialog", "File: {}", path.display())),
        message: if missing {
            ctr!(
                "ErrorDialog",
                "Neither the file nor its folder is there any more."
            )
        } else {
            ctr!("ErrorDialog", "The file browser did not open: {}", err)
        },
        details: Default::default(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_shows_selected_and_a_gone_one_its_folder() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("dragon.stl");
        std::fs::write(&file, b"solid").unwrap();
        assert_eq!(what_to_show(&file), Some(Shown::File(file.clone())));
        let gone = dir.path().join("gone.stl");
        assert_eq!(
            what_to_show(&gone),
            Some(Shown::Folder(dir.path().to_path_buf()))
        );
        assert_eq!(what_to_show(&dir.path().join("no/such.stl")), None);

        let command = command_for(&Shown::File(file.clone()));
        let args: Vec<String> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        if cfg!(windows) {
            assert_eq!(command.get_program(), "explorer");
        }
        assert!(args.iter().any(|a| a.contains("dragon.stl")), "{args:?}");
    }

    #[test]
    fn a_path_becomes_a_file_address() {
        assert_eq!(
            file_uri(Path::new("/home/dana/minis/Dragón 1.stl")),
            "file:///home/dana/minis/Drag%C3%B3n%201.stl"
        );
    }
}
