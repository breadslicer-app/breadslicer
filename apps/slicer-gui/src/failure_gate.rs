/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Reports the first of a run of consecutive failures, so an operation
//! that fails on every attempt is reported once, not on every attempt.

use std::cell::Cell;

#[derive(Debug, Default)]
pub(crate) struct FailureGate {
    consecutive: Cell<u32>,
}

impl FailureGate {
    /// Records a failure; `true` only for the first one since the last
    /// success or reset.
    pub(crate) fn note_failure(&self) -> bool {
        let prior = self.consecutive.get();
        self.consecutive.set(prior.saturating_add(1));
        prior == 0
    }

    /// Ends the current run, so the next failure is reported again.
    pub(crate) fn reset(&self) {
        self.consecutive.set(0);
    }

    #[cfg(test)]
    pub(crate) fn consecutive_failures(&self) -> u32 {
        self.consecutive.get()
    }
}

#[cfg(test)]
mod failure_gate_tests {
    use super::*;

    #[test]
    fn only_the_first_failure_of_a_run_is_reported() {
        let gate = FailureGate::default();
        assert!(gate.note_failure());
        assert!(!gate.note_failure());
        assert!(!gate.note_failure());
        assert_eq!(gate.consecutive_failures(), 3);
    }

    #[test]
    fn a_reset_ends_the_run() {
        let gate = FailureGate::default();
        assert!(gate.note_failure());
        gate.reset();
        assert_eq!(gate.consecutive_failures(), 0);
        assert!(gate.note_failure());
    }
}
