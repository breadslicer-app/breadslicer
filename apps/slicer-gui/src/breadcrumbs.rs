/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Breadcrumbs: a short trail of what the app was doing, with timestamps.
//!
//! The trail is always kept in memory (the last [`KEPT`] events). It reaches
//! the disk only in 2 ways:
//! - the diagnostic log (Preferences, off by default) appends every event to
//!   `logs/session.log` in the app's data folder as it happens, for a person
//!   reproducing a problem to send; the previous one is kept as
//!   `session.prev.log`;
//! - a panic writes the last events into `crash.log` with the backtrace, so a
//!   crash says which step it died in even with the log off.
//!
//! Events only: no geometry, paths or anything the person typed.

use std::collections::VecDeque;
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// How many events the in-memory trail keeps.
const KEPT: usize = 200;

struct Trail {
    started: Instant,
    recent: VecDeque<String>,
    file: Option<std::fs::File>,
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
            file: None,
        });
    }
    note(&format!("start v{}", env!("CARGO_PKG_VERSION")));
}

/// Whether the diagnostic log is writing to disk.
pub(crate) fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Turns the diagnostic log on or off. Turning it on starts a new
/// `session.log` (keeping the last one aside) holding the events already in
/// memory, so the session so far is not lost.
pub(crate) fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
    let Ok(mut trail) = TRAIL.lock() else {
        return;
    };
    let Some(trail) = trail.as_mut() else {
        return;
    };
    if !on {
        trail.file = None;
        return;
    }
    if trail.file.is_some() || cfg!(test) {
        return;
    }
    let Some(dir) = logs_dir() else {
        return;
    };
    let path = dir.join("session.log");
    let _ = std::fs::rename(&path, dir.join("session.prev.log"));
    if let Ok(mut file) = std::fs::File::create(&path) {
        for line in &trail.recent {
            let _ = writeln!(file, "{line}");
        }
        let _ = file.flush();
        trail.file = Some(file);
    }
}

/// Records one event; with the diagnostic log on, also appends it to disk,
/// flushed at once so it survives a crash right after.
pub(crate) fn note(event: &str) {
    let Ok(mut trail) = TRAIL.lock() else {
        return;
    };
    let Some(trail) = trail.as_mut() else {
        return;
    };
    let line = format!("{:>9.3}s {event}", trail.started.elapsed().as_secs_f64());
    if let Some(file) = trail.file.as_mut() {
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
    }
    if trail.recent.len() == KEPT {
        trail.recent.pop_front();
    }
    trail.recent.push_back(line);
}

/// The events kept in memory, oldest first, for a crash report. Never blocks:
/// a panic while the trail is locked gets none.
pub(crate) fn recent() -> Vec<String> {
    TRAIL
        .try_lock()
        .ok()
        .and_then(|trail| trail.as_ref().map(|t| t.recent.iter().cloned().collect()))
        .unwrap_or_default()
}

/// `<local data dir>/logs`, created if missing.
pub(crate) fn logs_dir() -> Option<std::path::PathBuf> {
    let dir = sc_app_paths::project_dirs()?.data_local_dir().join("logs");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_trail_keeps_only_the_last_events_and_nothing_reaches_disk_by_default() {
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
}
