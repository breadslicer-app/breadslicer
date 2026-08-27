/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 EntoMind Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Dispatches geometry-operation jobs other than support generation:
//! CSG subtract/intersect, mesh fixing, etc.
//! to whichever concrete algorithm the caller/app/cli picks,
//! behind the same "submit, poll, get one result" protocol shape that
//! `support-gen-handler` uses. See [`handler`] for the full picture.
//!
//!
//! This is a placeholder, no geometry algorithm/operation is implemented or registered here yet.


pub mod handler;

pub use handler::{
    GeomOpAlgorithm, GeomOpError, GeomOpJobContext, GeomOpResult, reproduce_geom_op, run_geom_op,
};

#[cfg(test)]
mod tests_handler;
