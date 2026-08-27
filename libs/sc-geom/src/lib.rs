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
//! - `support-gen-handler`: dispatches a support-generation job to
//!   whichever concrete algorithm the caller picks.
//! - `support-gen-trivial`: the deliberately trivial reference
//!   algorithm that proves the interface end to end. Not a real
//!   support-generation feature.
//! - `geom-ops-handler`: the same shape as `support-gen-handler`,
//!   for geometry operations that aren't support generation (CSG
//!   subtract/intersect, mesh fixing, ...). No concrete operation is
//!   registered yet.
//!
//! `support-gen-chalyplex` (the real, in-development support-generation
//! algorithm) is deliberately **not** re-exported here yet: it is
//! still empty. Once it implements `support-gen-handler`'s
//! `SupportAlgorithm`, add it alongside `support-gen-trivial` above.

pub use geom_ops_handler::{
    GeomOpAlgorithm, GeomOpError, GeomOpJobContext, GeomOpResult, reproduce_geom_op, run_geom_op,
};
pub use sc_geom_transport_inprocess::{
    AlgorithmId, CancelToken, Diagnostic, FieldSource, JobError, JobHandle, JobProgress, JobRecipe,
    JobWorker, ResolutionRecord, Severity, Staged,
};
pub use support_gen_handler::{
    BoundaryIntersection, BoundaryShape, PrintPlate, SupportAlgorithm, SupportError,
    SupportJobContext, SupportStructureResult, SupportStructureSettings, gen_support, normalize,
    reproduce_support, suggest_support_regions,
};
pub use support_gen_trivial::{
    SupportPartial, TRIVIAL_SUPPORT_ALGORITHM_ID, TRIVIAL_SUPPORT_VERSION, TrivialSupport,
    TrivialSupportInferredSpecs, TrivialSupportSettings,
};
