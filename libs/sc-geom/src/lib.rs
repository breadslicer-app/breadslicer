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
//! - `generic-geom-alg-family-dispatch`: the generic dispatch skeleton
//!   (`GeomAlgorithm`, `GeomAlgorithmJobContext`, `GeomAlgorithmResult`,
//!   `GeomAlgorithmError`, `AlgorithmFamilyCommonSpecs`, `BoundaryShape`,
//!   `run_geom_algorithm`/`reproduce_geom_algorithm`) every geometry-
//!   operation family builds on. Domain-agnostic; not itself a family.
//! - `support-gen-family-dispatch`: the support-generation family's own
//!   pieces on top of that skeleton -- its common specs
//!   (`SupportStructureSpecs`), its own error (`SupportAlgError`),
//!   and `suggest_support_regions`.
//! - `support-gen-trivial`: the deliberately trivial reference
//!   algorithm that proves the interface end to end. Not a real
//!   support-generation feature.
//!
//! `support-gen-chalyplex` (the real, in-development support-generation
//! algorithm) is deliberately **not** re-exported here yet: it is
//! still empty. Once it implements `GeomAlgorithm`, add it alongside
//! `support-gen-trivial` above.

pub use generic_geom_alg_family_dispatch::{
    AlgorithmFamilyCommonSpecs, BoundaryIntersection, BoundaryShape, GeomAlgorithm,
    GeomAlgorithmError, GeomAlgorithmJobContext, GeomAlgorithmResult, reproduce_geom_algorithm,
    run_geom_algorithm,
};
pub use sc_geom_transport_inprocess::{
    AlgorithmId, CancelToken, Diagnostic, FieldSource, JobError, JobHandle, JobProgress, JobRecipe,
    JobWorker, ResolutionRecord, Severity, Staged,
};
pub use support_gen_family_dispatch::{
    PrintPlate, SupportAlgError, SupportStructureSpecs, normalize, suggest_support_regions,
};
pub use support_gen_trivial::{
    SupportPartial, TRIVIAL_SUPPORT_ALGORITHM_ID, TRIVIAL_SUPPORT_VERSION, TrivialSupport,
    TrivialSupportInferredSpecs, TrivialSupportSpecs,
};
