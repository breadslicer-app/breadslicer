/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 EntoMind Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Support-structure generation, the first geometry-operation family
//! built on `geom-ops-handler`'s generic dispatch (see that crate for
//! the shared job/result/error machinery this family builds on, and for
//! why support generation is not treated as special).
//!
//!
//! How these pieces fit together:
//!
//! - [`handler`] holds what's specific to this family:
//!   [`handler::SupportStructureSpecs`] (this family's common
//!   family_specs), [`handler::SupportAlgError`] (this family's error,
//!   wrapped generically via
//!   `geom_ops_handler::GeomAlgorithmError::FamilySpecific`), and
//!   [`handler::suggest_support_regions`], a lightweight, synchronous,
//!   separate query for "what needs support, how badly."
//! - [`boundary`] and [`print_plate`] are the shared geometric context
//!   (the region a job's output must stay within, and the physical
//!   machine) every support algorithm needs.
//!
//! A concrete algorithm (`support-gen-trivial`'s `TrivialSupport`,
//! `support-gen-chalyplex`'s future one) implements
//! `geom_ops_handler::GeomAlgorithm` directly, with
//! `GeomAlgCommonSpecs = SupportStructureSpecs`; drivers submit/replay a
//! job through `geom_ops_handler::run_geom_algorithm`/
//! `reproduce_geom_algorithm`, same as any other family -- this crate
//! adds no submission entry point of its own.

pub mod boundary;
pub mod handler;
pub mod print_plate;

pub use boundary::{BoundaryIntersection, BoundaryShape};
pub use geom_ops_handler::{
    AlgorithmFamilyCommonSpecs, GeomAlgorithm, GeomAlgorithmError, GeomAlgorithmJobContext,
    GeomAlgorithmResult, reproduce_geom_algorithm, run_geom_algorithm,
};
pub use handler::{SupportAlgError, SupportStructureSpecs, normalize, suggest_support_regions};
pub use print_plate::PrintPlate;

#[cfg(test)]
mod tests_boundary;
#[cfg(test)]
mod tests_handler;
