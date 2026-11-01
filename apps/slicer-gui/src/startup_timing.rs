/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! How long the launch took, step by step (#359).
//!
//! [`mark`] notes the end of each step of the startup path, from `main` to the
//! first frame on screen and the first turn of the event loop after it, when
//! the window takes input. Times count from the creation of the process, so
//! the loader's work before `main` shows too (on Windows; elsewhere they count
//! from the first mark).
//!
//! Once the window is interactive, the steps go into the event trail as 1 line
//! (`startup: …`), which Help > Export Diagnostics carries. With
//! `BREADSLICER_STARTUP_TRACE` naming a file, they are also written there, 1
//! step per line, for the startup timing harness.

use std::fmt::Write as _;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// The file the steps are written to, when set.
const TRACE_ENV: &str = "BREADSLICER_STARTUP_TRACE";

struct Timeline {
    /// The first mark, and how old the process was then.
    origin: Instant,
    process_age: Option<Duration>,
    steps: Vec<(&'static str, Duration)>,
}

static TIMELINE: Mutex<Option<Timeline>> = Mutex::new(None);
static FRAMES: AtomicU32 = AtomicU32::new(0);
static REPORTED: AtomicBool = AtomicBool::new(false);

/// Notes that step `step` just ended.
pub(crate) fn mark(step: &'static str) {
    if REPORTED.load(Ordering::Relaxed) {
        return;
    }
    let now = Instant::now();
    let Ok(mut timeline) = TIMELINE.lock() else {
        return;
    };
    let timeline = timeline.get_or_insert_with(|| Timeline {
        origin: now,
        process_age: platform::process_age(),
        steps: Vec::new(),
    });
    let at = timeline.process_age.unwrap_or_default() + (now - timeline.origin);
    timeline.steps.push((step, at));
}

/// Called before every frame a window draws; marks the first two.
pub(crate) fn frame_starting() {
    match FRAMES.load(Ordering::Relaxed) {
        0 => mark("first frame started"),
        1 => mark("second frame started"),
        _ => {}
    }
}

/// Called after every frame a window presents. The first one marks the frame
/// and schedules the interactive mark for the next turn of the event loop,
/// the first one free to handle input.
pub(crate) fn frame_presented() {
    match FRAMES.fetch_add(1, Ordering::Relaxed) {
        0 => {
            mark("first frame");
            slint::Timer::single_shot(Duration::ZERO, || {
                mark("interactive");
                report();
            });
        }
        1 => mark("second frame"),
        2 => mark("third frame"),
        _ => {}
    }
}

/// The steps so far as `step<TAB>ms since the process started` lines.
fn trace() -> String {
    let Ok(timeline) = TIMELINE.lock() else {
        return String::new();
    };
    let mut out = String::new();
    if let Some(timeline) = timeline.as_ref() {
        for (step, at) in &timeline.steps {
            let _ = writeln!(out, "{step}\t{:.1}", at.as_secs_f64() * 1000.0);
        }
    }
    out
}

/// Puts the steps in the event trail and, when asked, in the trace file.
fn report() {
    let trace = trace();
    REPORTED.store(true, Ordering::Relaxed);
    let line = trace
        .lines()
        .map(|line| line.replace('\t', " "))
        .collect::<Vec<_>>()
        .join(", ");
    crate::breadcrumbs::note(&format!("startup (ms since the process started): {line}"));
    if let Some(path) = std::env::var_os(TRACE_ENV) {
        let _ = std::fs::write(path, trace);
    }
}

#[cfg(windows)]
mod platform {
    use std::time::Duration;

    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    fn ticks(time: FILETIME) -> u64 {
        (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
    }

    /// How long ago the process was created.
    #[allow(unsafe_code)]
    pub(super) fn process_age() -> Option<Duration> {
        // SAFETY: both calls only write the structs they are handed, which
        // live on this stack frame for the whole call; the pseudo handle of
        // the current process needs no closing.
        let (created, now) = unsafe {
            let mut created: FILETIME = std::mem::zeroed();
            let mut unused: [FILETIME; 3] = std::mem::zeroed();
            let [exited, kernel, user] = &mut unused;
            if GetProcessTimes(GetCurrentProcess(), &mut created, exited, kernel, user) == 0 {
                return None;
            }
            let mut now: FILETIME = std::mem::zeroed();
            GetSystemTimePreciseAsFileTime(&mut now);
            (created, now)
        };
        // FILETIME counts 100 ns ticks.
        let age = ticks(now).checked_sub(ticks(created))?;
        Some(Duration::from_nanos(age.saturating_mul(100)))
    }
}

#[cfg(not(windows))]
mod platform {
    pub(super) fn process_age() -> Option<std::time::Duration> {
        None
    }
}
