/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The recent files (#381): the files last opened or saved, newest first, at
//! most [`RECENT_FILES_CAP`]. They live in the settings file, which the
//! diagnostics file leaves them out of (#74), and never go anywhere else.
//!
//! A load that finishes (`ViewportController::on_scene_load_finished`) and a
//! project write that succeeds (`wiring::save_to_target`) call [`note`]; the
//! app's preferences bus, set with [`record_to`], keeps the list. A scene read
//! from an autosave or an earlier version is not noted: the person did not
//! open that file.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::autosave_store::unix_seconds;
use crate::i18n::ctr;
use crate::mesh_loader::is_supported_mesh_extension;
use crate::preferences_bus::{PreferencesBus, PreferencesChange};

/// How many files the list keeps.
pub(crate) const RECENT_FILES_CAP: usize = 10;

/// 1 file of the list: where it is, and when it was last opened or saved, in
/// seconds since the Unix epoch.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct RecentFile {
    pub(crate) path: PathBuf,
    pub(crate) opened: u64,
}

/// Whether `a` and `b` name the same file as the list compares them: on
/// Windows, letter case and the kind of slash make no difference.
pub(crate) fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        let fold = |p: &Path| p.to_string_lossy().replace('/', "\\").to_lowercase();
        fold(a) == fold(b)
    } else {
        a == b
    }
}

/// Puts `path` first in `list`, opened at `now`, taking out its older entry
/// and the files past the cap. The first file never reads older than the one
/// after it, even with a clock set back.
pub(crate) fn remember(list: &mut Vec<RecentFile>, path: PathBuf, now: u64) {
    list.retain(|file| !same_path(&file.path, &path));
    let opened = list.first().map_or(now, |newest| now.max(newest.opened));
    list.insert(0, RecentFile { path, opened });
    list.truncate(RECENT_FILES_CAP);
}

/// Takes `path` off `list`; returns whether it was there.
pub(crate) fn forget(list: &mut Vec<RecentFile>, path: &Path) -> bool {
    let before = list.len();
    list.retain(|file| !same_path(&file.path, path));
    list.len() != before
}

/// The list as `theirs`, the settings file as another process left it, with
/// the changes that this process made from `base` to `ours`: the files it
/// opened or saved come in with their time, and the files it took off stay
/// off unless `theirs` opened them again since. Newest first, at most
/// [`RECENT_FILES_CAP`].
pub(crate) fn merge(
    base: &[RecentFile],
    ours: &[RecentFile],
    theirs: Vec<RecentFile>,
) -> Vec<RecentFile> {
    if base == ours {
        return theirs;
    }
    let taken_off: Vec<&RecentFile> = base
        .iter()
        .filter(|old| !ours.iter().any(|file| same_path(&file.path, &old.path)))
        .collect();
    let mut merged = ours.to_vec();
    for their in theirs {
        if let Some(file) = merged
            .iter_mut()
            .find(|file| same_path(&file.path, &their.path))
        {
            file.opened = file.opened.max(their.opened);
            continue;
        }
        let gone = taken_off
            .iter()
            .any(|old| same_path(&old.path, &their.path) && their.opened <= old.opened);
        if !gone {
            merged.push(their);
        }
    }
    // Stable: files opened in the same second keep their order.
    merged.sort_by_key(|file| std::cmp::Reverse(file.opened));
    merged.truncate(RECENT_FILES_CAP);
    merged
}

thread_local! {
    // The bus that keeps the list, set by `record_to`. Unset in tests that
    // build no app, so their loads and saves note nothing.
    static RECORDER: RefCell<Weak<PreferencesBus>> = const { RefCell::new(Weak::new()) };
}

/// Makes [`note`] keep the list in `bus` from now on, on this thread.
pub(crate) fn record_to(bus: &Rc<PreferencesBus>) {
    RECORDER.with(|recorder| *recorder.borrow_mut() = Rc::downgrade(bus));
}

/// Puts `path`, just opened or saved, first in the list, when it is a file the
/// app opens. A relative path is made absolute first.
pub(crate) fn note(path: &Path) {
    if !is_supported_mesh_extension(path) {
        return;
    }
    let Some(bus) = RECORDER.with(|recorder| recorder.borrow().upgrade()) else {
        return;
    };
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let now = unix_seconds(SystemTime::now());
    bus.update(move |snap| {
        remember(&mut snap.recent_files, path, now);
        PreferencesChange::RecentFiles
    });
}

/// Takes `path` off the list kept in `bus`.
pub(crate) fn remove(bus: &PreferencesBus, path: PathBuf) {
    bus.update(move |snap| {
        forget(&mut snap.recent_files, &path);
        PreferencesChange::RecentFiles
    });
}

/// The file's name, as the list shows it.
pub(crate) fn name_label(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// The folder `path` is in, with the home folder `home` written `~`.
pub(crate) fn folder_label(path: &Path, home: Option<&Path>) -> String {
    let Some(folder) = path.parent() else {
        return String::new();
    };
    if let Some(home) = home
        && let Ok(rest) = folder.strip_prefix(home)
    {
        let separator = std::path::MAIN_SEPARATOR;
        return if rest.as_os_str().is_empty() {
            "~".to_owned()
        } else {
            format!("~{separator}{}", rest.display())
        };
    }
    folder.display().to_string()
}

/// The extension of `path` in capitals: `BREAD`, `STL`, `3MF`...
pub(crate) fn kind_label(path: &Path) -> String {
    path.extension()
        .map(|ext| ext.to_string_lossy().to_uppercase())
        .unwrap_or_default()
}

/// Whether `path` is a Breadslicer project.
pub(crate) fn is_project(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("bread"))
}

/// When a file was last opened, as the list says it at `now`: the time today,
/// "Yesterday", the days for the last week, the date before that.
pub(crate) fn when_label(opened: u64, now: SystemTime) -> String {
    use chrono::{DateTime, Local};

    let opened: DateTime<Local> = (SystemTime::UNIX_EPOCH + Duration::from_secs(opened)).into();
    let now: DateTime<Local> = now.into();
    let days = (now.date_naive() - opened.date_naive()).num_days();
    match days {
        ..=0 => ctr!("StartDialog", "Today, {}", opened.format("%H:%M")).to_string(),
        1 => ctr!("StartDialog", "Yesterday").to_string(),
        2..=6 => ctr!("StartDialog", "{n} day ago", "{n} days ago", days).to_string(),
        _ => opened.format("%Y-%m-%d").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, opened: u64) -> RecentFile {
        RecentFile {
            path: PathBuf::from(path),
            opened,
        }
    }

    fn paths(list: &[RecentFile]) -> Vec<String> {
        list.iter()
            .map(|file| file.path.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn a_file_opened_goes_first() {
        let mut list = vec![file("/m/a.stl", 10), file("/m/b.bread", 5)];
        remember(&mut list, PathBuf::from("/m/c.obj"), 20);
        assert_eq!(paths(&list), ["/m/c.obj", "/m/a.stl", "/m/b.bread"]);
        assert_eq!(list[0].opened, 20);
    }

    #[test]
    fn a_file_opened_again_moves_up_and_is_listed_once() {
        let mut list = vec![file("/m/a.stl", 10), file("/m/b.bread", 5)];
        remember(&mut list, PathBuf::from("/m/b.bread"), 20);
        assert_eq!(paths(&list), ["/m/b.bread", "/m/a.stl"]);
    }

    #[test]
    fn the_list_keeps_the_ten_newest() {
        let mut list = Vec::new();
        for n in 0..14 {
            remember(&mut list, PathBuf::from(format!("/m/{n}.stl")), n);
        }
        assert_eq!(list.len(), RECENT_FILES_CAP);
        assert_eq!(list[0].path, PathBuf::from("/m/13.stl"));
        assert_eq!(list[9].path, PathBuf::from("/m/4.stl"));
    }

    #[test]
    fn a_clock_set_back_still_puts_the_file_first() {
        let mut list = vec![file("/m/a.stl", 100)];
        remember(&mut list, PathBuf::from("/m/b.stl"), 50);
        assert_eq!(paths(&list), ["/m/b.stl", "/m/a.stl"]);
        assert!(list[0].opened >= list[1].opened);
    }

    #[test]
    fn a_file_taken_off_leaves_the_others_in_order() {
        let mut list = vec![
            file("/m/a.stl", 3),
            file("/m/b.stl", 2),
            file("/m/c.stl", 1),
        ];
        assert!(forget(&mut list, Path::new("/m/b.stl")));
        assert_eq!(paths(&list), ["/m/a.stl", "/m/c.stl"]);
        assert!(!forget(&mut list, Path::new("/m/b.stl")));
    }

    #[cfg(windows)]
    #[test]
    fn on_windows_case_and_slashes_name_the_same_file() {
        let mut list = vec![file(r"C:\Models\Hueco.bread", 1)];
        remember(&mut list, PathBuf::from("c:/models/hueco.bread"), 2);
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn a_merge_keeps_the_files_another_process_opened() {
        let base = vec![file("/m/a.stl", 1)];
        let ours = vec![file("/m/ours.stl", 5), file("/m/a.stl", 1)];
        let theirs = vec![file("/m/theirs.obj", 3), file("/m/a.stl", 1)];
        let merged = merge(&base, &ours, theirs);
        assert_eq!(paths(&merged), ["/m/ours.stl", "/m/theirs.obj", "/m/a.stl"]);
    }

    #[test]
    fn a_merge_keeps_a_file_taken_off_out_unless_opened_again_since() {
        let base = vec![file("/m/a.stl", 2), file("/m/b.stl", 1)];
        let ours = vec![file("/m/b.stl", 1)];
        let untouched = merge(&base, &ours, base.clone());
        assert_eq!(paths(&untouched), ["/m/b.stl"]);
        let reopened = merge(&base, &ours, vec![file("/m/a.stl", 9), file("/m/b.stl", 1)]);
        assert_eq!(paths(&reopened), ["/m/a.stl", "/m/b.stl"]);
    }

    #[test]
    fn a_merge_with_no_change_of_ours_is_theirs() {
        let base = vec![file("/m/a.stl", 1)];
        let theirs = vec![file("/m/z.stl", 4)];
        assert_eq!(merge(&base, &base, theirs.clone()), theirs);
    }

    #[test]
    fn a_merge_keeps_the_cap() {
        let base = Vec::new();
        let ours: Vec<_> = (0..10)
            .map(|n| file(&format!("/o/{n}.stl"), 100 + n))
            .collect();
        let theirs: Vec<_> = (0..10).map(|n| file(&format!("/t/{n}.stl"), n)).collect();
        let merged = merge(&base, &ours, theirs);
        assert_eq!(merged.len(), RECENT_FILES_CAP);
        assert!(merged.iter().all(|f| f.path.starts_with("/o")));
    }

    #[test]
    fn the_home_folder_is_written_as_a_tilde() {
        let home = Path::new("/home/dana");
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(
            folder_label(Path::new("/home/dana/minis/hueco.bread"), Some(home)),
            format!("~{sep}minis")
        );
        assert_eq!(
            folder_label(Path::new("/home/dana/hueco.bread"), Some(home)),
            "~"
        );
        assert_eq!(
            folder_label(Path::new("/srv/hueco.bread"), Some(home)),
            Path::new("/srv").display().to_string()
        );
    }

    #[test]
    fn the_kind_is_the_extension_in_capitals() {
        assert_eq!(kind_label(Path::new("/m/a.bread")), "BREAD");
        assert_eq!(kind_label(Path::new("/m/a.3mf")), "3MF");
        assert!(is_project(Path::new("/m/a.BREAD")));
        assert!(!is_project(Path::new("/m/a.stl")));
    }

    #[test]
    fn the_date_says_today_yesterday_days_or_the_date() {
        use chrono::{Local, TimeZone};

        let at = |y, m, d, h| {
            let local = Local.with_ymd_and_hms(y, m, d, h, 30, 0).unwrap();
            SystemTime::from(local)
        };
        let now = at(2026, 11, 1, 18);
        let secs = |time| unix_seconds(time);
        assert_eq!(when_label(secs(at(2026, 11, 1, 9)), now), "Today, 09:30");
        assert_eq!(when_label(secs(at(2026, 10, 31, 23)), now), "Yesterday");
        assert_eq!(when_label(secs(at(2026, 10, 28, 9)), now), "4 days ago");
        assert_eq!(when_label(secs(at(2026, 10, 2, 9)), now), "2026-10-02");
    }
}
