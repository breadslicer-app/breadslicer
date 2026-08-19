/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! This module holds the process-wide record of a lost graphics
//! device: a flag meant to be set from wgpu's device-lost callback,
//! with the reason, and read before the device is touched. That
//! callback is wgpu's only signal of a loss; error scopes and the
//! uncaptured-error handler never see it, resources created afterwards
//! are silently invalid, and the next buffer map panics.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// Whether the shared device is lost, to be checked before touching
/// the device. Cloned handles see the same state, from any thread.
#[derive(Clone, Debug, Default)]
pub struct DeviceLoss(Arc<Inner>);

#[derive(Debug, Default)]
struct Inner {
    lost: AtomicBool,
    reason: Mutex<Option<String>>,
    reported: AtomicBool,
}

impl DeviceLoss {
    /// Records the loss; the first reason wins.
    pub fn mark(&self, reason: String) {
        self.0
            .reason
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert(reason);
        self.0.lost.store(true, Ordering::Release);
    }

    pub fn is_lost(&self) -> bool {
        self.0.lost.load(Ordering::Acquire)
    }

    pub fn reason(&self) -> Option<String> {
        self.0
            .reason
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// `true` for exactly one caller per process: the one that tells
    /// the user.
    pub fn take_report(&self) -> bool {
        !self.0.reported.swap(true, Ordering::AcqRel)
    }
}

#[cfg(test)]
mod device_loss_tests {
    use super::*;

    #[test]
    fn a_fresh_flag_is_clear() {
        let loss = DeviceLoss::default();
        assert!(!loss.is_lost());
        assert_eq!(loss.reason(), None);
    }

    #[test]
    fn marking_sets_the_flag_and_keeps_the_first_reason() {
        let loss = DeviceLoss::default();
        let other = loss.clone();
        other.mark("Destroyed: by test".to_string());
        other.mark("Unknown: later".to_string());
        assert!(loss.is_lost());
        assert_eq!(loss.reason().as_deref(), Some("Destroyed: by test"));
    }

    #[test]
    fn the_report_is_taken_once() {
        let loss = DeviceLoss::default();
        assert!(loss.take_report());
        assert!(!loss.clone().take_report());
    }

    #[test]
    fn clones_share_the_state_across_threads() {
        let loss = DeviceLoss::default();
        let worker = loss.clone();
        std::thread::spawn(move || worker.mark("Destroyed: worker".to_string()))
            .join()
            .expect("worker thread");
        assert!(loss.is_lost());
    }
}
