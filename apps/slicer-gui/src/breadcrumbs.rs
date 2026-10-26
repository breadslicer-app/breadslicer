/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Breadcrumbs: a short trail of what the app was doing, appended to
//! `logs/session.log` in the app's data folder as it happens. A panic leaves
//! `crash.log`; a crash that is no panic (a native fault, a stack overflow,
//! the process killed while frozen) leaves nothing else, and then the last
//! line here says which step it died in. Events only: no geometry, paths or
//! anything the person typed. The previous session's trail is kept as
//! `session.prev.log`.

use std::io::Write;
use std::sync::Mutex;
use std::time::Instant;

static TRAIL: Mutex<Option<(std::fs::File, Instant)>> = Mutex::new(None);

/// Starts this session's trail, keeping the last one aside.
pub(crate) fn start() {
    if cfg!(test) {
        return;
    }
    let Some(dir) = sc_app_paths::project_dirs().map(|d| d.data_local_dir().join("logs")) else {
        return;
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join("session.log");
    let _ = std::fs::rename(&path, dir.join("session.prev.log"));
    if let Ok(file) = std::fs::File::create(&path)
        && let Ok(mut trail) = TRAIL.lock()
    {
        *trail = Some((file, Instant::now()));
    }
    note(&format!("start v{}", env!("CARGO_PKG_VERSION")));
}

/// Appends one event, flushed at once so it survives a crash right after.
pub(crate) fn note(event: &str) {
    if let Ok(mut trail) = TRAIL.lock()
        && let Some((file, started)) = trail.as_mut()
    {
        let _ = writeln!(file, "{:>9.3}s {event}", started.elapsed().as_secs_f64());
        let _ = file.flush();
    }
}
