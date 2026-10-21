/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 EntoMind Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! [`Staged`] marks a single result field as provisional or final.
//!
//! This is a different axis from [`crate::job::JobProgress`]: progress
//! says how far along a job is, `Staged` says how much to trust one
//! particular value in its result right now. A rough material-weight
//! estimate available immediately and an exact one available only once
//! the job finishes are both legitimate answers to "how much material
//! will this take". They carry different confidence, which is
//! why this is a wrapper on the "value" rather than a separate optional
//! field that the caller has to remember to check against job completion.

/// A value that may still be provisional, or is now final. `Provisional`
/// values can still change on a later publish (see
/// `JobWorker::publish_partial`); `Final` values will not change again
/// for this job run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Staged<T> {
    Provisional(T),
    Final(T),
}

impl<T> Staged<T> {
    #[inline]
    pub fn value(&self) -> &T {
        match self {
            Self::Provisional(v) | Self::Final(v) => v,
        }
    }

    #[inline]
    pub fn into_inner(self) -> T {
        match self {
            Self::Provisional(v) | Self::Final(v) => v,
        }
    }

    #[inline]
    pub fn is_final(&self) -> bool {
        matches!(self, Self::Final(_))
    }
}
