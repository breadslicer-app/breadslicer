/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 EntoMind Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The generic base every geometry-operation family (support generation,
//! CSG subtract/intersection, mesh-fixing, ...) builds on:
//! one dispatch trait ([`dispatch::GeomAlgorithm`]),
//! one job context, one result shape, one error wrapper,
//! all generic over the family's own common-specs type (general inputs)
//! and each algorithm's own specs/output types. [`boundary`] (the region
//! a job's output must stay within) lives here too: not support-specific,
//! any family may need it.
//! A crate for each algorithm-family (currently: `support-gen-family-dispatch`)
//! adds only what's genuinely specific to this family, on top of this;
//! it does not reimplement any of this (D.R.Y.).
//!
//! Support generation is not special. It's simply the first algorithm-family that is built.
//! The patterns and complexity that emerges from `support-gen-family-dispatch` needed
//! (e.g., a pair of specs: a shared/common specs (input settings) type, a family-specific error, validation)
//! is expected to appear in every later family, in the same way.
pub mod boundary;
pub mod dispatch;

pub use boundary::{BoundaryIntersection, BoundaryShape};
pub use dispatch::{
    AlgorithmFamilyCommonSpecs, GeomAlgorithm, GeomAlgorithmError, GeomAlgorithmJobContext,
    GeomAlgorithmResult, reproduce_geom_algorithm, run_geom_algorithm,
};

#[cfg(test)]
mod tests_boundary;
#[cfg(test)]
mod tests_dispatch;
