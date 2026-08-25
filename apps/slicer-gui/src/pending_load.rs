/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Per-window slot for a load request that can't start yet because the
//! GPU device hasn't been published (no `RenderingSetup` has fired
//! since the window opened, or a teardown cleared the slot). External
//! entry points (argv at startup, drag-and-drop before first paint,
//! the future macOS `openURLs` event) write here via
//! [`PendingLoad::queue`]; the controller drains it from
//! `handle_rendering_setup` once the device is available.
//!
//! The slot is one-shot (`take` empties it) and latest-wins (a second
//! `queue` before drain replaces the first). Latest-wins matters when
//! several requests can stack up before first paint: the user should
//! see the most recent file they asked for, not the first.
//!
//! One instance per `MainWindow`, shared via `Rc` between
//! `AppState::WiredMainWindow` (the writer) and `ViewportController`
//! (the reader / drainer).

use std::cell::Cell;
use std::path::PathBuf;

#[derive(Default)]
pub(crate) struct PendingLoad {
    slot: Cell<Option<PathBuf>>,
}

impl PendingLoad {
    pub(crate) fn new() -> Self {
        Self {
            slot: Cell::new(None),
        }
    }

    /// Park `path` in the slot. Latest-wins: silently overwrites any
    /// path that was already queued. Used by external entry points
    /// before the window's GPU device is available.
    pub(crate) fn queue(&self, path: PathBuf) {
        self.slot.set(Some(path));
    }

    /// Drain the slot, returning the queued path if any. The drainer
    /// (the controller's `handle_rendering_setup`) calls this once the
    /// device is published; `None` is the steady-state.
    pub(crate) fn take(&self) -> Option<PathBuf> {
        self.slot.take()
    }

    /// Discard any queued path without dispatching it. Used by paths
    /// where the request becomes moot (e.g. the window is closing) so
    /// the slot doesn't fire on a dead window.
    #[allow(dead_code)]
    pub(crate) fn clear(&self) {
        self.slot.set(None);
    }
}

#[cfg(test)]
mod pending_load_tests {
    use super::*;

    #[test]
    fn fresh_slot_is_empty() {
        let p = PendingLoad::new();
        assert!(p.take().is_none());
    }

    #[test]
    fn queue_then_take_returns_the_path() {
        let p = PendingLoad::new();
        p.queue(PathBuf::from("/tmp/a.stl"));
        assert_eq!(p.take(), Some(PathBuf::from("/tmp/a.stl")));
    }

    #[test]
    fn take_empties_the_slot() {
        let p = PendingLoad::new();
        p.queue(PathBuf::from("/tmp/a.stl"));
        let _ = p.take();
        assert!(
            p.take().is_none(),
            "the slot must be empty after a take; double-take would deliver a stale path"
        );
    }

    #[test]
    fn second_queue_overwrites_the_first() {
        // Latest-wins is the documented invariant: when several open
        // requests stack up before first paint (multiple drops, a
        // burst of macOS openURLs events), the user expects to see
        // the last file they asked for.
        let p = PendingLoad::new();
        p.queue(PathBuf::from("/tmp/first.stl"));
        p.queue(PathBuf::from("/tmp/second.stl"));
        assert_eq!(p.take(), Some(PathBuf::from("/tmp/second.stl")));
    }

    #[test]
    fn clear_empties_the_slot() {
        let p = PendingLoad::new();
        p.queue(PathBuf::from("/tmp/a.stl"));
        p.clear();
        assert!(p.take().is_none());
    }
}
