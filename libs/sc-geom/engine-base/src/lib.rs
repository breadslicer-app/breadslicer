/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! What every stage of the geometry engines shares (#682): one cancel token,
//! the error a cancelled stage returns, and a [`Control`] that a stage polls
//! for cancellation and reports its progress to.
//!
//! The job that runs an engine hands its own token down, so a cancel reaches
//! the stage at its next poll, not at its next progress report.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A shared cancellation flag; its clones observe the same flag.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// A token nobody has cancelled yet.
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Cancels every holder of this token or of one of its clones.
    #[inline]
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Whether the token was cancelled.
    #[inline]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// What a stage returns when it stopped because it was cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

impl fmt::Display for Cancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// What a stage reports to: an optional cancel check and an optional
/// progress sink, both shared with the stage's worker threads. `S` is the
/// engine's own list of stages.
pub struct Control<'a, S> {
    cancelled: Option<&'a (dyn Fn() -> bool + Sync)>,
    token: Option<&'a CancelToken>,
    progress: Option<&'a (dyn Fn(S) + Sync)>,
}

impl<S> Clone for Control<'_, S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S> Copy for Control<'_, S> {}

impl<S> Default for Control<'_, S> {
    fn default() -> Self {
        Self {
            cancelled: None,
            token: None,
            progress: None,
        }
    }
}

impl<'a, S> Control<'a, S> {
    /// Never cancelled, reports nowhere.
    pub fn none() -> Self {
        Self::default()
    }

    /// Cancelled by `token`, reporting to `progress`.
    pub fn new(token: Option<&'a CancelToken>, progress: Option<&'a (dyn Fn(S) + Sync)>) -> Self {
        Self {
            cancelled: None,
            token,
            progress,
        }
    }

    /// Cancelled when `cancelled` says so, reporting to `progress`.
    pub fn with_check(
        cancelled: &'a (dyn Fn() -> bool + Sync),
        progress: Option<&'a (dyn Fn(S) + Sync)>,
    ) -> Self {
        Self {
            cancelled: Some(cancelled),
            token: None,
            progress,
        }
    }

    /// Whether the stage should stop.
    #[inline]
    pub fn is_cancelled(&self) -> bool {
        self.token.is_some_and(CancelToken::is_cancelled) || self.cancelled.is_some_and(|f| f())
    }

    /// `Err(Cancelled)` when the stage should stop.
    #[inline]
    pub fn check(&self) -> Result<(), Cancelled> {
        if self.is_cancelled() {
            Err(Cancelled)
        } else {
            Ok(())
        }
    }

    /// Checks for a cancel, then reports that `stage` starts.
    pub fn stage(&self, stage: S) -> Result<(), Cancelled> {
        self.check()?;
        if let Some(progress) = self.progress {
            progress(stage);
        }
        Ok(())
    }
}

impl<S> fmt::Debug for Control<'_, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Control")
            .field("cancelled", &self.cancelled.is_some())
            .field("token", &self.token)
            .field("progress", &self.progress.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn a_clone_of_a_token_sees_its_cancel() {
        let token = CancelToken::new();
        let held = token.clone();
        assert!(!held.is_cancelled());
        token.cancel();
        assert!(held.is_cancelled());
    }

    #[test]
    fn a_stage_reports_until_its_token_is_cancelled() {
        let token = CancelToken::new();
        let seen = Mutex::new(Vec::new());
        let report = |s: u8| seen.lock().unwrap().push(s);
        let control = Control::new(Some(&token), Some(&report));
        assert_eq!(control.stage(1), Ok(()));
        token.cancel();
        assert_eq!(control.stage(2), Err(Cancelled));
        assert_eq!(*seen.lock().unwrap(), [1]);
    }

    #[test]
    fn a_check_function_cancels_too() {
        let control: Control<'_, ()> = Control::with_check(&|| true, None);
        assert_eq!(control.check(), Err(Cancelled));
        assert_eq!(Control::<()>::none().check(), Ok(()));
    }
}
