/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Output checks (#93): plate edges, maximum height, empty layers and
//! floating parts.
//!
//! Reports [`CheckKind::PlateEdge`](crate::finding::CheckKind::PlateEdge),
//! [`CheckKind::TooTall`](crate::finding::CheckKind::TooTall),
//! [`CheckKind::EmptyLayer`](crate::finding::CheckKind::EmptyLayer) and
//! [`CheckKind::FloatingPart`](crate::finding::CheckKind::FloatingPart)
//! findings. The shared data it builds on: solid regions that touch the
//! screen edge (`touches_border`), empty layers
//! ([`LayerRegions::is_empty`](crate::regions::LayerRegions::is_empty)), the
//! layer heights and the printer's maximum height
//! ([`ReviewInput::layers`](crate::input::ReviewInput::layers),
//! [`ReviewInput::max_z_mm`](crate::input::ReviewInput::max_z_mm)), and the
//! printed bodies of
//! [`PlateRegions::group_solid`](crate::regions::PlateRegions::group_solid).
//!
//! Not implemented yet: returns no finding.

use crate::finding::Finding;
use crate::runner::ReviewContext;

/// Settings of the output checks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OutputOptions {}

/// Finds the output problems of the plate.
pub fn run(ctx: &ReviewContext<'_>, options: &OutputOptions) -> Vec<Finding> {
    let _ = (ctx, options);
    Vec::new()
}

#[cfg(test)]
#[path = "tests_output.rs"]
mod tests;
