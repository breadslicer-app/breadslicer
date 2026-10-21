/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 EntoMind Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Support-structure generation: the first geometry-operation family
//! built on `generic-geom-alg-family-dispatch`'s generic
//! [`generic_geom_alg_family_dispatch::GeomAlgorithm`] dispatch.
//! Everything here is specific to support generation alone;
//! the dispatching mechanism itself
//! (job context, result, error wrapping, submit/replay)
//! lives one layer down and is shared with every future family.
//!
//! [`SupportStructureSpecs`] is this family's
//! [`AlgorithmFamilyCommonSpecs`]: the inputs every support algorithm
//! shares (gravity, the region it must stay within, the printer's own
//! plate, explicit or auto-detected anchor locations). Anything specific
//! to one algorithm's numerics lives in that algorithm's own `GeomAlgSpecificSpecs`
//! associated type instead.
//!
//! See libs/sc-geom/support-gen-family-dispatch/src/dispatch.rs
//!  for types: `SupportStructureSpecs`, `SupportAlgError`.
//! How these pieces fit together:
//!
//! - [`dispatch`] holds what's specific to this family:
//!   [`dispatch::SupportStructureSpecs`] (this family's common
//!   family_specs), [`dispatch::SupportAlgError`] (this family's error,
//!   wrapped generically via
//!   `generic_geom_alg_family_dispatch::GeomAlgorithmError::FamilySpecific`), and
//!   [`dispatch::suggest_support_regions`], a lightweight, synchronous,
//!   separate query for "what needs support, how badly."
//! - [`print_plate`] is the physical-machine context every support
//!   algorithm needs. `BoundaryShape` (the region a job's output must
//!   stay within) is no longer defined here -- it moved to
//!   `generic-geom-alg-family-dispatch`, since it's not support-specific,
//!   and is re-exported below so nothing downstream has to change how it
//!   imports it.
//!
//! A concrete algorithm (`support-gen-trivial`'s `TrivialSupport`,
//! `support-gen-chalyplex`'s future one) implements
//! `generic_geom_alg_family_dispatch::GeomAlgorithm` directly, with
//! `GeomAlgCommonSpecs = SupportStructureSpecs`; drivers submit/replay a
//! job through `generic_geom_alg_family_dispatch::run_geom_algorithm`/
//! `reproduce_geom_algorithm`, same as any other family -- this crate
//! adds no submission entry point of its own.

pub mod dispatch;
pub mod print_plate;

pub use dispatch::{SupportAlgError, SupportStructureSpecs, normalize, suggest_support_regions};
pub use generic_geom_alg_family_dispatch::{
    AlgorithmFamilyCommonSpecs, BoundaryIntersection, BoundaryShape, GeomAlgorithm,
    GeomAlgorithmError, GeomAlgorithmJobContext, GeomAlgorithmResult, reproduce_geom_algorithm,
    run_geom_algorithm,
};
pub use print_plate::PrintPlate;

#[cfg(test)]
mod tests_dispatch;
