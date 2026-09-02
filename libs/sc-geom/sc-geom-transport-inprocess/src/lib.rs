/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 EntoMind Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! In-process transport for sc-geom's handlers: the async job model, a
//! constraint-to-actual resolution trail, and a reproducibility recipe.
//! Domain-agnostic: no dependency on `sc-mesh-core` or any
//! other mesh type. The `support-gen-handler`, `geom-ops-handler`,
//! and any future handler all build their trait/dispatch skeleton on
//! the same primitives instead of each reinventing a job handle.
//!
//! A future `sc-geom-flow-outofprocess` sibling crate is meant to
//! implement the same shape (a geometry-processing job the app/cli submits, poll progress on, and
//! get one terminal result from) over a subprocess/socket boundary
//! instead of a thread; a handler written against [`JobHandle`] should
//! not need to change to pick either up.
//!
//! # How the pieces fit together
//!
//! - [`job`] is the generic long-running-job machinery: [`job::JobHandle`]
//!   for progress/diagnostics/partial snapshots/cancellation/the
//!   terminal result, all without assuming any particular async
//!   runtime (this workspace has none).
//!
//! - [`staged`] wraps a single result field as [`staged::Staged::Provisional`]
//!   or [`staged::Staged::Final`]. It is orthogonal to the concept of job progress:
//!       e.g., how much to trust the progressive result / partial value returned/ent back so far.
//!       It is not about how far along the job is (in terms of time, iterations, etc).
//!
//! - [`resolution`] is the constraint-to-actual trail, mirroring
//!   `sc_print_profiles::resolve()`'s warnings: one [`resolution::ResolutionRecord`]
//!   per specs/settings field, naming where its final value came from.
//! - [`recipe`] is what makes a finished job reproducible: a
//!   [`recipe::JobRecipe`] captures the fully resolved specs/settings (not
//!   the caller's original, possibly-underspecified ones), so a replay
//!   never re-runs auto-detection.
//!
//! # Not included (v0)
//!
//! - The out-of-process backend itself (see above) is to be implemented.
//! - `serde`/on-disk persistence of a [`recipe::JobRecipe`] or
//!   [`job::Diagnostic`]. Both are plain, non-serialised structs for
//!   now.

pub mod job;
pub mod recipe;
pub mod resolution;
pub mod staged;

pub use job::{CancelToken, Diagnostic, JobError, JobHandle, JobProgress, JobWorker, Severity};
pub use recipe::{AlgorithmId, JobRecipe};
pub use resolution::{FieldSource, ResolutionRecord};
pub use staged::Staged;

#[cfg(test)]
mod tests_job;
#[cfg(test)]
mod tests_recipe;
#[cfg(test)]
mod tests_resolution;
#[cfg(test)]
mod tests_staged;
