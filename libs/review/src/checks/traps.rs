/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Resin traps and suction cups (#83): cavities of air the resin cannot
//! leave.
//!
//! Reports [`CheckKind::ResinTrap`](crate::finding::CheckKind::ResinTrap) and
//! [`CheckKind::SuctionCup`](crate::finding::CheckKind::SuctionCup)
//! findings. The shared data it builds on: every layer's air
//! [`Region`](crate::regions::Region)s (4-connected; `touches_border` marks
//! the open air), the air overlaps between consecutive layers
//! ([`PlateRegions::air_links`](crate::regions::PlateRegions::air_links)),
//! and [`PlateRegions::group_air`](crate::regions::PlateRegions::group_air)
//! to follow a cavity through the layers.
//!
//! Not implemented yet: returns no finding.

use crate::finding::Finding;
use crate::runner::ReviewContext;

/// Settings of the resin-trap and suction-cup check.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrapOptions {}

/// Finds the resin traps and suction cups of the plate.
pub fn run(ctx: &ReviewContext<'_>, options: &TrapOptions) -> Vec<Finding> {
    let _ = (ctx, options);
    Vec::new()
}

#[cfg(test)]
#[path = "tests_traps.rs"]
mod tests;
