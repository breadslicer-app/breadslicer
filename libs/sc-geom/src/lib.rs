/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 EntoMind Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! This is a facade crate: the one thing the main app (SoulCrafted Slicer, a
//! sibling checkout) depends on. Has no code of its own: purely re-exports from this workspace's real modules,
//! so that the main app's `sc-geom = { path = "..." }` "dependency" never has to change if
//! the internal module boundaries move. The internal modules are:
//!
//! - `sc-geom-transport-inprocess`: the in-process job/progress/
//!   diagnostics/recipe machinery every handler builds on. Domain-
//!   agnostic; a future `sc-geom-flow-outofprocess` sibling is meant
//!   to implement the same shape over a subprocess/socket boundary.

pub use sc_geom_transport_inprocess::{
    AlgorithmId, CancelToken, Diagnostic, FieldSource, JobError, JobHandle, JobProgress, JobRecipe,
    JobWorker, ResolutionRecord, Severity, Staged,
};
/// The DragonFruit support port, run on a scene (`bread-support-session`).
pub use bread_support_session as supports;

/// The DragonFruit hollowing port, run on a scene leaf (`bread-hollow-session`; the
/// engine itself is `hollow::bread_hollow_engine`).
pub use bread_hollow_session as hollow;
