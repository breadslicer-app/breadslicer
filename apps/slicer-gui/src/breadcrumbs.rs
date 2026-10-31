/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Breadcrumbs: a short trail of what the app was doing, with timestamps.
//!
//! The trail is always kept in memory (the last [`KEPT`] events). It reaches
//! the disk only in 3 ways, none of them sent anywhere:
//! - the diagnostic log (Preferences > Privacy, off at every launch) appends
//!   every event to `logs/session.log` in the app's data folder as it
//!   happens, until the app closes; the previous one, or the first part of a
//!   long one, is kept as `session.prev.log`. A log is never emptied before
//!   it is kept aside: when another program holds it open so it can't be,
//!   the new one goes in a file of its own, `session-<stamp>.log`;
//! - a panic writes the last events into `crash.log` with the backtrace, so a
//!   crash says which step it died in even with the log off;
//! - Help > Export Diagnostics puts them in the file the person saves.
//!
//! Events only: no geometry, file names or anything the person typed. Each
//! event goes through [`Scrubber`] and [`hide_paths`], so a path in an error
//! comes out as the kind of file it was (`<file>.stl`), without the person's
//! home folder, user name or file names.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::diagnostics::scrub::{Scrubber, hide_paths};

/// How many events the in-memory trail keeps.
const KEPT: usize = 200;

/// The most a `session.log` holds: past it the file starts over, and the
/// full one is kept as `session.prev.log`.
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

/// The diagnostic log on disk.
struct LogFile {
    dir: PathBuf,
    file: File,
    written: u64,
    limit: u64,
}

impl LogFile {
    /// Starts a new `session.log` in `dir`, keeping the last one as
    /// `session.prev.log`: moved there, or copied when another program holds
    /// it open. When the last one can't be kept aside, or `session.log` can't
    /// be written, the new log goes in a file of its own instead
    /// (`session-<stamp>.log`), so no log is ever emptied. Fails only when
    /// `dir` can't be written.
    fn start(dir: &Path, limit: u64) -> std::io::Result<Self> {
        let path = dir.join("session.log");
        let file = if !path.exists() {
            OpenOptions::new().write(true).create_new(true).open(&path)
        } else if set_aside(&path, &dir.join("session.prev.log")) {
            // Moved away, or copied: either way it is safe to start over.
            OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&path)
        } else {
            Err(std::io::ErrorKind::AlreadyExists.into())
        };
        Ok(Self {
            dir: dir.to_owned(),
            file: match file {
                Ok(file) => file,
                Err(_) => own_file(dir)?,
            },
            written: 0,
            limit,
        })
    }

    /// Appends `line`, flushed at once so it survives a crash right after.
    /// A line that would take the file past its limit starts it over first.
    fn append(&mut self, line: &str) {
        let size = line.len() as u64 + 1;
        if self.written > 0
            && self.written + size > self.limit
            && let Ok(fresh) = Self::start(&self.dir, self.limit)
        {
            *self = fresh;
        }
        if writeln!(self.file, "{line}").is_ok() {
            self.written += size;
        }
        let _ = self.file.flush();
    }
}

/// Keeps the log at `path` as `prev`: moves it, or copies it when another
/// program holds it open. Whether it is kept.
fn set_aside(path: &Path, prev: &Path) -> bool {
    std::fs::rename(path, prev).is_ok() || std::fs::copy(path, prev).is_ok()
}

/// A new log file in `dir` with a name of its own, `session-<stamp>.log`.
fn own_file(dir: &Path) -> std::io::Result<File> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let mut n = 0;
    loop {
        let name = match n {
            0 => format!("session-{stamp}.log"),
            n => format!("session-{stamp}-{n}.log"),
        };
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join(name))
        {
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists && n < 1000 => n += 1,
            opened => return opened,
        }
    }
}

struct Trail {
    started: Instant,
    recent: VecDeque<String>,
    /// The tasks under way, for the time [`finished`] notes.
    running: Vec<(&'static str, Instant)>,
    file: Option<LogFile>,
}

static TRAIL: Mutex<Option<Trail>> = Mutex::new(None);
static ENABLED: AtomicBool = AtomicBool::new(false);

/// Starts this session's trail. Writes to disk only once the diagnostic log
/// is on (see [`set_enabled`]).
pub(crate) fn start() {
    if let Ok(mut trail) = TRAIL.lock() {
        *trail = Some(Trail {
            started: Instant::now(),
            recent: VecDeque::with_capacity(KEPT),
            running: Vec::new(),
            file: None,
        });
    }
    note(&format!(
        "start v{} ({}, {} {})",
        env!("CARGO_PKG_VERSION"),
        crate::diagnostics::commit(),
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
}

/// Whether the diagnostic log is writing to disk.
pub(crate) fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Turns the diagnostic log on or off. Turning it on starts a new
/// `session.log` (keeping the last one aside) holding the events already in
/// memory, so the session so far is not lost. Fails, with the log left off,
/// when that file can't start.
pub(crate) fn set_enabled(on: bool) -> std::io::Result<()> {
    let mut trail = TRAIL
        .lock()
        .map_err(|_| std::io::Error::other("the event trail is unusable"))?;
    let trail = trail
        .as_mut()
        .ok_or_else(|| std::io::Error::other("the event trail has not started"))?;
    if !on {
        trail.file = None;
    } else if trail.file.is_none() {
        let dir = log_dir().ok_or_else(|| std::io::Error::other("there is no logs folder"))?;
        let mut file = LogFile::start(&dir, MAX_LOG_BYTES)?;
        for line in &trail.recent {
            file.append(line);
        }
        trail.file = Some(file);
    }
    ENABLED.store(on, Ordering::Relaxed);
    Ok(())
}

/// Records one event; with the diagnostic log on, also appends it to disk.
pub(crate) fn note(event: &str) {
    let Ok(mut trail) = TRAIL.lock() else {
        return;
    };
    let Some(trail) = trail.as_mut() else {
        return;
    };
    // One event, one line: an error's chain of causes goes on the same line.
    let event = hide_paths(&Scrubber::of_this_user().scrub(event))
        .lines()
        .map(str::trim_end)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" | ");
    let line = format!("{:>9.3}s {event}", trail.started.elapsed().as_secs_f64());
    if let Some(file) = trail.file.as_mut() {
        file.append(&line);
    }
    if trail.recent.len() == KEPT {
        trail.recent.pop_front();
    }
    trail.recent.push_back(line);
}

/// Notes that `task` (`"slice"`, `"save"`…) started, and keeps when, for
/// [`finished`].
pub(crate) fn started(task: &'static str) {
    if let Ok(mut trail) = TRAIL.lock()
        && let Some(trail) = trail.as_mut()
    {
        trail.running.push((task, Instant::now()));
    }
    note(&format!("{task}: started"));
}

/// Notes how `task` ended (`ok`, `cancelled`, `failed: …`) and, when it was
/// [`started`], how long it took.
pub(crate) fn finished(task: &'static str, outcome: &str) {
    let took = TRAIL.lock().ok().and_then(|mut trail| {
        let running = &mut trail.as_mut()?.running;
        let at = running.iter().rposition(|(t, _)| *t == task)?;
        Some(running.remove(at).1.elapsed())
    });
    match took {
        Some(took) => note(&format!("{task}: {outcome} in {:.2} s", took.as_secs_f64())),
        None => note(&format!("{task}: {outcome}")),
    }
}

/// `ok`, or `failed: <the error>`, for [`finished`].
pub(crate) fn outcome<T, E: std::fmt::Display>(result: &Result<T, E>) -> String {
    match result {
        Ok(_) => "ok".to_owned(),
        Err(err) => format!("failed: {err}"),
    }
}

/// The events kept in memory, oldest first, for a crash report or a
/// diagnostics file. Never blocks: a panic while the trail is locked gets
/// none.
pub(crate) fn recent() -> Vec<String> {
    TRAIL
        .try_lock()
        .ok()
        .and_then(|trail| trail.as_ref().map(|t| t.recent.iter().cloned().collect()))
        .unwrap_or_default()
}

/// `<local data dir>/logs`, created if missing.
pub(crate) fn logs_dir() -> Option<PathBuf> {
    let dir = sc_app_paths::project_dirs()?.data_local_dir().join("logs");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Where [`set_enabled`] starts the log: [`logs_dir`].
#[cfg(not(test))]
fn log_dir() -> Option<PathBuf> {
    logs_dir()
}

/// In a test, the folder it gave to [`write_log_into`], never the person's
/// logs folder; none is a system without one.
#[cfg(test)]
fn log_dir() -> Option<PathBuf> {
    TEST_LOG_DIR.with_borrow(Clone::clone)
}

#[cfg(test)]
thread_local! {
    static TEST_LOG_DIR: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Makes the diagnostic log that this thread turns on write into `dir`.
#[cfg(test)]
pub(crate) fn write_log_into(dir: &Path) {
    TEST_LOG_DIR.with_borrow_mut(|to| *to = Some(dir.to_owned()));
}

/// Held by the tests that restart the shared trail, so they run one at a
/// time.
#[cfg(test)]
pub(crate) fn serial() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_trail_keeps_only_the_last_events_and_nothing_reaches_disk_by_default() {
        let _serial = serial();
        start();
        for i in 0..(KEPT + 5) {
            note(&format!("event {i}"));
        }
        let kept = recent();
        assert_eq!(kept.len(), KEPT);
        assert!(
            kept.last()
                .unwrap()
                .ends_with(&format!("event {}", KEPT + 4))
        );
        assert!(!enabled());
        let file_open = TRAIL.lock().unwrap().as_ref().unwrap().file.is_some();
        assert!(!file_open);
    }

    #[test]
    fn a_full_log_starts_over_and_keeps_the_full_one_aside() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("session.log"), "last session\n").unwrap();
        let mut log = LogFile::start(dir.path(), 25).unwrap();
        let read = |name: &str| std::fs::read_to_string(dir.path().join(name)).unwrap();
        assert_eq!(read("session.prev.log"), "last session\n");

        log.append("0123456789");
        log.append("0123456789");
        assert_eq!(read("session.log"), "0123456789\n0123456789\n");
        // A 3rd line would pass 25 bytes.
        log.append("abc");
        assert_eq!(read("session.log"), "abc\n");
        assert_eq!(read("session.prev.log"), "0123456789\n0123456789\n");
    }

    /// The files of `dir` other than `session.log` and `session.prev.log`.
    fn own_files(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                let name = path.file_name().unwrap();
                name != "session.log" && name != "session.prev.log"
            })
            .collect()
    }

    /// #223: another program holds the last `session.log` open, sharing
    /// `share` (Windows share flags), when the log starts.
    #[cfg(windows)]
    fn start_while_held(share: u32) -> (tempfile::TempDir, Vec<PathBuf>) {
        use std::os::windows::fs::OpenOptionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.log");
        std::fs::write(
            dir.path().join("session.prev.log"),
            "older session
",
        )
        .unwrap();
        std::fs::write(
            &path,
            "last session
",
        )
        .unwrap();
        let held = OpenOptions::new()
            .read(true)
            .share_mode(share)
            .open(&path)
            .unwrap();
        let mut log = LogFile::start(dir.path(), MAX_LOG_BYTES).expect("the log starts");
        log.append("new session");
        drop(log);
        drop(held);
        let own = own_files(dir.path());
        (dir, own)
    }

    #[cfg(windows)]
    const SHARE_READ: u32 = 1;
    #[cfg(windows)]
    const SHARE_WRITE: u32 = 2;
    #[cfg(windows)]
    const SHARE_DELETE: u32 = 4;

    #[cfg(windows)]
    fn read(dir: &tempfile::TempDir, name: &str) -> String {
        std::fs::read_to_string(dir.path().join(name)).unwrap()
    }

    #[cfg(windows)]
    #[test]
    fn a_log_held_open_with_delete_shared_is_moved_aside() {
        let (dir, own) = start_while_held(SHARE_READ | SHARE_WRITE | SHARE_DELETE);
        assert_eq!(
            read(&dir, "session.prev.log"),
            "last session
"
        );
        assert_eq!(
            read(&dir, "session.log"),
            "new session
"
        );
        assert!(own.is_empty(), "{own:?}");
    }

    #[cfg(windows)]
    #[test]
    fn a_log_held_open_for_writing_is_copied_aside_before_it_is_emptied() {
        let (dir, own) = start_while_held(SHARE_READ | SHARE_WRITE);
        assert_eq!(
            read(&dir, "session.prev.log"),
            "last session
"
        );
        assert_eq!(
            read(&dir, "session.log"),
            "new session
"
        );
        assert!(own.is_empty(), "{own:?}");
    }

    #[cfg(windows)]
    #[test]
    fn a_log_held_open_for_reading_only_is_kept_and_the_new_one_has_its_own_file() {
        let (dir, own) = start_while_held(SHARE_READ);
        assert_eq!(
            read(&dir, "session.prev.log"),
            "last session
"
        );
        assert_eq!(
            read(&dir, "session.log"),
            "last session
"
        );
        let [own] = own.as_slice() else {
            panic!("one file of its own: {own:?}");
        };
        assert_eq!(
            std::fs::read_to_string(own).unwrap(),
            "new session
"
        );
        let name = own.file_name().unwrap().to_string_lossy();
        assert!(
            name.starts_with("session-") && name.ends_with(".log"),
            "{name}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_log_that_cannot_even_be_read_is_left_alone() {
        let (dir, own) = start_while_held(0);
        assert_eq!(
            read(&dir, "session.log"),
            "last session
"
        );
        assert_eq!(
            read(&dir, "session.prev.log"),
            "older session
"
        );
        let [own] = own.as_slice() else {
            panic!("one file of its own: {own:?}");
        };
        assert_eq!(
            std::fs::read_to_string(own).unwrap(),
            "new session
"
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_log_whose_aside_file_is_held_is_not_emptied() {
        use std::os::windows::fs::OpenOptionsExt;

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("session.prev.log"),
            "older session
",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("session.log"),
            "last session
",
        )
        .unwrap();
        // session.prev.log can be neither replaced nor written.
        let held = OpenOptions::new()
            .read(true)
            .share_mode(SHARE_READ)
            .open(dir.path().join("session.prev.log"))
            .unwrap();
        let mut log = LogFile::start(dir.path(), MAX_LOG_BYTES).expect("the log starts");
        log.append("new session");
        drop((log, held));
        assert_eq!(
            read(&dir, "session.log"),
            "last session
"
        );
        assert_eq!(
            read(&dir, "session.prev.log"),
            "older session
"
        );
        let own = own_files(dir.path());
        let [own] = own.as_slice() else {
            panic!("one file of its own: {own:?}");
        };
        assert_eq!(
            std::fs::read_to_string(own).unwrap(),
            "new session
"
        );
    }

    #[test]
    fn two_own_files_in_the_same_second_get_different_names() {
        let dir = tempfile::tempdir().unwrap();
        let first = own_file(dir.path()).unwrap();
        let second = own_file(dir.path()).unwrap();
        drop((first, second));
        assert_eq!(own_files(dir.path()).len(), 2);
    }

    #[test]
    fn a_line_longer_than_the_limit_is_still_written() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = LogFile::start(dir.path(), 4).unwrap();
        log.append("too long for the limit");
        let written = std::fs::read_to_string(dir.path().join("session.log")).unwrap();
        assert_eq!(written, "too long for the limit\n");
    }

    #[test]
    fn a_finished_task_says_how_it_ended_and_how_long_it_took() {
        let _serial = serial();
        start();
        started("unit-test-task");
        finished("unit-test-task", "ok");
        finished("unit-test-other", "cancelled");
        let kept = recent();
        let line = |what: &str| kept.iter().find(|l| l.contains(what)).cloned();
        assert!(line("unit-test-task: started").is_some());
        let done = line("unit-test-task: ok in ").expect("the end, with its time");
        assert!(done.ends_with(" s"));
        let other = line("unit-test-other:").expect("an end with no start");
        assert!(other.ends_with("unit-test-other: cancelled"));
    }

    #[test]
    fn an_outcome_is_ok_or_the_error() {
        assert_eq!(outcome(&Ok::<(), &str>(())), "ok");
        assert_eq!(outcome(&Err::<(), _>("no facets")), "failed: no facets");
    }
}
