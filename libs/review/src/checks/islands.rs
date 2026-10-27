/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Islands (#84): solid regions that start in mid-air.
//!
//! Reports [`CheckKind::Island`](crate::finding::CheckKind::Island)
//! findings. The shared data it builds on: every solid
//! [`Region`](crate::regions::Region) of layer `k > 0` knows how many of its
//! pixels rest on solid in layer `k - 1` (`below_px`) and how many are model
//! or support; [`PlateRegions::group_solid`](crate::regions::PlateRegions::group_solid)
//! joins an island with the layers that grow from it.
//!
//! Not implemented yet: returns no finding.

use crate::finding::Finding;
use crate::runner::ReviewContext;

/// Settings of the island check.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IslandOptions {}

/// Finds the islands of the plate.
pub fn run(ctx: &ReviewContext<'_>, options: &IslandOptions) -> Vec<Finding> {
    let _ = (ctx, options);
    Vec::new()
}

#[cfg(test)]
#[path = "tests_islands.rs"]
mod tests;
