/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Progress and cancellation of a repair run (#253). A run tells its
//! [`RepairWatch`] each step as it starts, with how far the run is, and asks
//! it there whether to stop. A run that stops returns [`RepairCancelled`] and
//! no mesh: it works on a copy, so the mesh it was given stays as it was.
//! Watching changes nothing a run does.

/// A step of a repair run, as its progress names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepairStep {
    /// Checks what the mesh needs.
    Checking,
    /// Removes faces that name a vertex twice or one that does not exist.
    RemovingBrokenFaces,
    /// Welds cracks.
    Welding,
    /// Removes duplicate faces and double walls.
    RemovingDuplicates,
    /// Separates parts that touch at an edge, and orients each piece.
    SeparatingParts,
    /// Fills holes.
    FillingHoles,
    /// Turns inside-out shells outward.
    TurningShellsOutward,
    /// Checks that a round of the watertight pass left the mesh better and
    /// lost no material.
    CheckingRound,
    /// Orients each piece of a mesh that passes the edge check.
    Orienting,
    /// Removes zero-area slivers.
    RemovingSlivers,
    /// Measures the result and finds what is left.
    CheckingResult,
}

/// Watches a repair run.
pub trait RepairWatch {
    /// `step` starts; `done` is the share of the run done, from 0 to 1. It
    /// never goes back within a run.
    fn step(&self, step: RepairStep, done: f32);
    /// Whether the run should stop. Asked as each step starts.
    fn cancelled(&self) -> bool;
}

/// A watch that wants nothing: no progress, never cancels.
#[derive(Clone, Copy, Debug, Default)]
pub struct Unwatched;

impl RepairWatch for Unwatched {
    fn step(&self, _step: RepairStep, _done: f32) {}

    fn cancelled(&self) -> bool {
        false
    }
}

/// A repair run stopped because its watch asked it to. Nothing it did is
/// kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RepairCancelled;

impl std::fmt::Display for RepairCancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the repair was cancelled")
    }
}

impl std::error::Error for RepairCancelled {}

/// A [`RepairWatch`] seen through a share of a run: a part of the run reports
/// its own progress from 0 to 1, and this maps it into `[from, to]` of the
/// whole.
#[derive(Clone, Copy)]
pub(crate) struct Progress<'a> {
    watch: &'a dyn RepairWatch,
    from: f32,
    to: f32,
}

impl<'a> Progress<'a> {
    /// The whole run of `watch`.
    pub(crate) fn of(watch: &'a dyn RepairWatch) -> Self {
        Self {
            watch,
            from: 0.0,
            to: 1.0,
        }
    }

    /// The part of this run from `from` to `to`, as shares of it.
    pub(crate) fn part(self, from: f32, to: f32) -> Self {
        let span = self.to - self.from;
        Self {
            watch: self.watch,
            from: self.from + span * from,
            to: self.from + span * to,
        }
    }

    /// `step` starts at `done` of this part: stops the run when the watch
    /// asks to, else tells it.
    pub(crate) fn step(self, step: RepairStep, done: f32) -> Result<(), RepairCancelled> {
        if self.watch.cancelled() {
            return Err(RepairCancelled);
        }
        self.watch
            .step(step, self.from + (self.to - self.from) * done);
        Ok(())
    }
}
