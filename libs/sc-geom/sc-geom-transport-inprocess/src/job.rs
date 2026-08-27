/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 EntoMind Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Handles generic long-running-job process:
//! It is shared by every geometry operation/algorithm, such as support generation.
//!
//! Implementation is based on `sc_sl1cer::api::{CancelToken, SliceProgress}`
//! plus the worker-thread/slot pattern that `apps/slicer-gui` already uses
//! to run the slicing engine off the UI thread: That is, a job runs on its own
//! `std::thread`.
//!
//! It reports coarse phase/count progress and structured
//! diagnostics while it runs, and deposits exactly one terminal result.
//!
//! Currently a [`JobHandle`] is polled or blocked on directly, so it is also usable from a GUI event loop, a blocking CLI command, or any other caller.

//!
//! [`JobHandle`] is generic over two outputs:
//! * `Out`, the one terminal result, and
//! * `Partial` (default `()`).
//!
//! Partial means a low-poly lighter-weight "snapshot" that a long job
//! can publish repeatedly (progressively) while it runs.
//! Example usages: It can publish a low-LOD preview mesh that shows progress,
//! or a rough material estimate "so-far" before the exact one is known.
//!
//! `Partial` versus `JobProgress`:
//! `Partial` is deliberately orthogonal to [`JobProgress`]:
//! progress says how far along the job is, a partial snapshot says what
//! the job's answer looks like *so far*.
//!
//! Todo:
//! An "out-of-process backend": as an implementation of [`JobHandle`]
//! (a native subprocess, in theory it can be, or an executable in another language such as Python,
//! speaking a small message protocol: newline-delimited,
//! `Progress` / `Diagnostic` / `Partial` / `Final` /`Error`
//! over the process's std io)
//!
//! implements [`JobHandle`] by relaying messages into a [`JobWorker`] from a thread that reads the child process's stdout.
//! This "out-of-process backend" is not implemented yet.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Cooperative cancellation flag for a job. Cheap to clone; clones
/// share one underlying flag, so a caller can hand a clone to a
/// worker thread and trip the original to request a stop. Same shape
/// as `sc_sl1cer::CancelToken`.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// A fresh token that has not been cancelled.
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation. Idempotent; visible to every clone.
    #[inline]
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Whether cancellation has been requested.
    #[inline]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Coarse progress of a job: a high-level stage (or phase), with a human-readable name plus a
/// completed/total value for progress within that phase.
/// Deliberately coarser than `sc_sl1cer::SliceProgress`'s per-layer atomics.
/// a mutex-guarded struct is simple enough, hence, geometry-operation phases are few and long, not many fine-grained ticks (like per-layer slicing).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobProgress {
    pub phase: String,
    pub completed: u32,
    pub total: u32,
}

impl JobProgress {
    /// Completed fraction in `[0.0, 1.0]`; `0.0` when there is no work.
    #[inline]
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            self.completed as f32 / self.total as f32
        }
    }
}

impl Default for JobProgress {
    fn default() -> Self {
        Self {
            phase: "starting".to_string(),
            completed: 0,
            total: 0,
        }
    }
}

/// Severity of a [`Diagnostic`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Debug,
}

/// One structured, auditable log entry from a job: a stable code (for
/// tooling), a human message, and free-form key/value context. Pushed
/// to the job's diagnostics as it runs, in addition to whatever it
/// returns in its terminal result. It is readable live via
/// [`JobHandle::diagnostics_snapshot`], or after completion of geometry-processing job's execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
    pub context: Vec<(String, String)>,
}

impl Diagnostic {
    pub fn new(severity: Severity, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity,
            code,
            message: message.into(),
            context: Vec::new(),
        }
    }

    pub fn with_context(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.context.push((key.into(), value.into()));
        self
    }
}

/// Why a job did not produce a result.
#[derive(Debug)]
pub enum JobError {
    /// The job observed `CancelToken::is_cancelled()` and stopped cleanly.
    /// Carries no partial data.
    /// Also [`JobHandle::latest_partial`] has the content/info that the job-execution has produced/published so far, before it stopped.
    Cancelled,
    /// The job's own logic refused to continue, or it panicked (a
    /// panic inside the worker is caught so the handle always resolves
    /// to a result instead of leaving callers joining a dead thread).
    Failed(String),
}

impl fmt::Display for JobError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => write!(f, "job was cancelled"),
            Self::Failed(msg) => write!(f, "job failed: {msg}"),
        }
    }
}

impl std::error::Error for JobError {}

struct JobState<Out, Partial> {
    progress: Mutex<JobProgress>,
    diagnostics: Mutex<Vec<Diagnostic>>,
    partial: Mutex<Option<Partial>>,
    slot: Mutex<Option<Result<Out, JobError>>>,
}

/// The worker-side handle a job's body uses to report progress,
/// publish partial snapshots, log diagnostics, and check for
/// cancellation. Never constructed directly by algorithm code --
/// [`JobHandle::spawn`] hands one to the closure it runs.
pub struct JobWorker<Out, Partial = ()> {
    cancel: CancelToken,
    state: Arc<JobState<Out, Partial>>,
}

impl<Out, Partial> JobWorker<Out, Partial> {
    #[inline]
    pub fn cancel_token(&self) -> &CancelToken {
        &self.cancel
    }

    #[inline]
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    pub fn report_progress(&self, progress: JobProgress) {
        *lock(&self.state.progress) = progress;
    }

    pub fn push_diagnostic(&self, diagnostic: Diagnostic) {
        lock(&self.state.diagnostics).push(diagnostic);
    }

    /// Publish (replacing) the latest partial snapshot. Readable live
    /// via [`JobHandle::latest_partial`] while the job keeps running.
    pub fn publish_partial(&self, partial: Partial) {
        *lock(&self.state.partial) = Some(partial);
    }
}

/// Handle to a running (or finished) job. `Out` is the one terminal
/// result; `Partial` (default `()`) is an optional, repeatedly
/// updated lighter-weight snapshot. Also see module's docs.
pub struct JobHandle<Out, Partial = ()> {
    cancel: CancelToken,
    state: Arc<JobState<Out, Partial>>,
    join: Option<JoinHandle<()>>,
}

impl<Out, Partial> fmt::Debug for JobHandle<Out, Partial> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JobHandle")
            .field("progress", &*lock(&self.state.progress))
            .field("finished", &lock(&self.state.slot).is_some())
            .finish()
    }
}

impl<Out, Partial> JobHandle<Out, Partial>
where
    Out: Send + 'static,
    Partial: Send + 'static,
{
    /// Runs `work` on its own thread, handing it a [`JobWorker`] to
    /// report through. A panic inside `work` is caught so the handle
    /// still resolves (as `JobError::Failed`) rather than leaving a
    /// caller blocked forever on [`Self::join`].
    pub fn spawn<F>(work: F) -> Self
    where
        F: FnOnce(&JobWorker<Out, Partial>) -> Result<Out, JobError> + Send + 'static,
    {
        let cancel = CancelToken::new();
        let state = Arc::new(JobState {
            progress: Mutex::new(JobProgress::default()),
            diagnostics: Mutex::new(Vec::new()),
            partial: Mutex::new(None),
            slot: Mutex::new(None),
        });

        let worker = JobWorker {
            cancel: cancel.clone(),
            state: state.clone(),
        };
        let slot_state = state.clone();
        let join = std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&worker)))
                .unwrap_or_else(|panic| {
                    let message = panic
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| panic.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "worker panicked".to_string());
                    Err(JobError::Failed(message))
                });
            *lock(&slot_state.slot) = Some(result);
        });

        Self {
            cancel,
            state,
            join: Some(join),
        }
    }

    /// A clone of this job's cancellation token; `cancel()` on either
    /// this or the clone stops the job.
    pub fn cancel_token(&self) -> CancelToken {
        self.cancel.clone()
    }

    /// Requests cancellation. The job observes it cooperatively; it
    /// does not stop instantly.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub fn progress(&self) -> JobProgress {
        lock(&self.state.progress).clone()
    }

    pub fn diagnostics_snapshot(&self) -> Vec<Diagnostic> {
        lock(&self.state.diagnostics).clone()
    }

    /// The most recently published "partial snapshot", if any.
    pub fn latest_partial(&self) -> Option<Partial>
    where
        Partial: Clone,
    {
        lock(&self.state.partial).clone()
    }

    /// Non-blocking check for the final result. Consumes it once
    /// seen: a second call after a `Some` returns `None`, even though
    /// the job is (still) finished. Callers that only need the final
    /// answer should prefer [`Self::join`].
    pub fn try_poll(&self) -> Option<Result<Out, JobError>> {
        lock(&self.state.slot).take()
    }

    /// Blocks until the job's thread finishes and returns its result.
    pub fn join(mut self) -> Result<Out, JobError> {
        if let Some(handle) = self.join.take() {
            let _ = handle.join();
        }
        lock(&self.state.slot)
            .take()
            .unwrap_or_else(|| Err(JobError::Failed("worker exited without a result".into())))
    }
}
