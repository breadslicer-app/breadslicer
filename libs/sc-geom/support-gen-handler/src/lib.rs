/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 EntoMind Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! "Dispatch"es a support-generation job to whichever concrete algorithm that the caller chooses.
//! Currently: `support-gen-trivial`, `support-gen-chalyplex`.
//! Later: It can be another one, or even a Python-wrapping variant, alongside it.
//! All are behind one shared interface.
//! It should never run a job itself.
//! [`handler::SupportAlgorithm`] "seam"s various algorithm crates as a plugin.
//! This crate owns only what is shared between all algorithms
//! (the common settings shape, the "job context", the result shape,
//! the resolution entry points and reproduction entry point).
//!
//! Uses `sc-geom-transport-inprocess` for
//! the job/progress/diagnostics/recipe machinery.
//! This crate adds no transport mechanism of its own.
//! The support-generation-specific types are layered on top of the shared infrastructure.
//!
//! How these pieces fit together:
//!
//! - [`handler`] is the main module: [`handler::SupportAlgorithm`],
//!   [`handler::gen_support`] / [`handler::reproduce_support`], and
//!   [`handler::suggest_support_regions`], a lightweight, synchronous,
//!   separate query for "what needs support, how badly."
//! - [`boundary`] and [`print_plate`] are the shared geometric context
//!   (the region a job's output must stay within, and the physical
//!   machine) every support algorithm needs.

pub mod boundary;
pub mod handler;
pub mod print_plate;

pub use boundary::{BoundaryIntersection, BoundaryShape};
pub use handler::{
    SupportAlgorithm, SupportError, SupportJobContext, SupportStructureResult,
    SupportStructureSettings, gen_support, normalize, reproduce_support, suggest_support_regions,
};
pub use print_plate::PrintPlate;

#[cfg(test)]
mod tests_boundary;
#[cfg(test)]
mod tests_handler;
