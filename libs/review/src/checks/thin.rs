/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Features thinner than a pixel (#92): walls, pins, blades and tips
//! narrower than the printer can resolve.
//!
//! Reports [`CheckKind::ThinFeature`](crate::finding::CheckKind::ThinFeature)
//! findings. The shared data it builds on: the vector contours of each layer
//! with their roles
//! ([`ReviewInput::polygons`](crate::input::ReviewInput::polygons)) and the
//! printer's pixel size
//! ([`ReviewInput::pixel_size_mm`](crate::input::ReviewInput::pixel_size_mm)).
//!
//! Not implemented yet: returns no finding.

use crate::finding::Finding;
use crate::runner::ReviewContext;

/// Settings of the thin-feature check.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThinFeatureOptions {}

/// Finds the features thinner than the printer resolves.
pub fn run(ctx: &ReviewContext<'_>, options: &ThinFeatureOptions) -> Vec<Finding> {
    let _ = (ctx, options);
    Vec::new()
}

#[cfg(test)]
#[path = "tests_thin.rs"]
mod tests;
