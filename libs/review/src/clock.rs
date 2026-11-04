/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! What each procedure of a review costs (#427), for measuring: with the
//! `stage-clock` feature every [`scope`] adds its time to its name, summed
//! over the threads that ran it, with the number of calls and the first
//! start and last end; [`count`] adds to a tally. Without the feature, both
//! compile to nothing.

/// One procedure's totals since the last [`reset`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stage {
    /// Its name.
    pub name: &'static str,
    /// How many times it ran, or what [`count`] added up.
    pub calls: u64,
    /// Its time, summed over the threads, in nanoseconds.
    pub nanos: u64,
    /// From its first start to its last end, in nanoseconds.
    pub span_nanos: u64,
}

#[cfg(feature = "stage-clock")]
mod on {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock, PoisonError};
    use std::time::Instant;

    use super::Stage;

    #[derive(Clone, Copy)]
    struct Entry {
        calls: u64,
        nanos: u64,
        first: Instant,
        last: Instant,
    }

    fn table() -> &'static Mutex<HashMap<&'static str, Entry>> {
        static TABLE: OnceLock<Mutex<HashMap<&'static str, Entry>>> = OnceLock::new();
        TABLE.get_or_init(Default::default)
    }

    /// Times from here to its drop.
    #[derive(Debug)]
    pub struct Scope {
        name: &'static str,
        started: Instant,
    }

    impl Drop for Scope {
        fn drop(&mut self) {
            let now = Instant::now();
            let nanos = (now - self.started).as_nanos() as u64;
            let mut table = table().lock().unwrap_or_else(PoisonError::into_inner);
            let entry = table.entry(self.name).or_insert(Entry {
                calls: 0,
                nanos: 0,
                first: self.started,
                last: now,
            });
            entry.calls += 1;
            entry.nanos += nanos;
            entry.first = entry.first.min(self.started);
            entry.last = entry.last.max(now);
        }
    }

    /// Starts timing `name`.
    #[must_use]
    pub fn scope(name: &'static str) -> Scope {
        Scope {
            name,
            started: Instant::now(),
        }
    }

    /// Adds `amount` to the tally `name`.
    pub fn count(name: &'static str, amount: u64) {
        let now = Instant::now();
        let mut table = table().lock().unwrap_or_else(PoisonError::into_inner);
        table
            .entry(name)
            .or_insert(Entry {
                calls: 0,
                nanos: 0,
                first: now,
                last: now,
            })
            .calls += amount;
    }

    /// Forgets every total.
    pub fn reset() {
        table()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    /// Every total, by name.
    pub fn snapshot() -> Vec<Stage> {
        let table = table().lock().unwrap_or_else(PoisonError::into_inner);
        let mut stages: Vec<Stage> = table
            .iter()
            .map(|(name, entry)| Stage {
                name,
                calls: entry.calls,
                nanos: entry.nanos,
                span_nanos: (entry.last - entry.first).as_nanos() as u64,
            })
            .collect();
        stages.sort_by_key(|stage| stage.name);
        stages
    }
}

#[cfg(feature = "stage-clock")]
pub use on::{Scope, count, reset, scope, snapshot};

/// Nothing to time.
#[cfg(not(feature = "stage-clock"))]
#[derive(Debug)]
pub struct Scope;

/// Starts timing `name`: nothing, without the feature.
#[cfg(not(feature = "stage-clock"))]
#[inline(always)]
#[must_use]
pub fn scope(_name: &'static str) -> Scope {
    Scope
}

/// Adds to a tally: nothing, without the feature.
#[cfg(not(feature = "stage-clock"))]
#[inline(always)]
pub fn count(_name: &'static str, _amount: u64) {}

/// Forgets every total: nothing, without the feature.
#[cfg(not(feature = "stage-clock"))]
#[inline(always)]
pub fn reset() {}

/// Every total: none, without the feature.
#[cfg(not(feature = "stage-clock"))]
#[inline(always)]
pub fn snapshot() -> Vec<Stage> {
    Vec::new()
}
