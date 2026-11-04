/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Pre-flight review of a sliced resin plate.
//!
//! `sc-review` reads the contours of one slicing run, knowing which leaves
//! are models and which are supports, and reports what is likely to fail
//! before anything is printed: islands, resin traps and suction cups,
//! features thinner than a pixel, and output problems (plate edges, maximum
//! height, empty layers, floating parts). Each [`Finding`] names its layers
//! and a point to centre the camera on, and may suggest a one-click fix.
//!
//! # Pipeline
//!
//! 1. Build a [`ReviewInput`]: the per-leaf contour stacks with their
//!    [`Role`], the layer schedule, the printer's screen and its maximum
//!    height ([`ReviewInput::from_profile`] takes the last three from the
//!    engine's `SliceProfile`).
//! 2. [`review`] (or [`review_with`] for options and timings) computes the
//!    shared [`PlateRegions`] once: every layer rasterised at the printer's
//!    resolution, split into solid regions (8-connected) and air regions
//!    (4-connected), with the pixel overlaps between consecutive layers.
//! 3. Each check in [`checks`] reads the regions through a
//!    [`ReviewContext`] and returns findings; the runner sorts them by
//!    layer, then check.
//!
//! A cavity too small to drain is offered a fill instead (#132): the
//! report carries its pixels as a [`CavityFill`], and [`fill_stack`] turns
//! the fills kept from one review into a contour stack that cures them solid
//! on the next slice of the same scene.
//!
//! Fix All (#156) adds two more fixes of the same kind: a floating speck is
//! offered an erasure (its pixels, removed on the next slice with
//! [`erase_stack`]), and a thin model feature a thickening (its pixels
//! grown, cured with [`fill_stack`] as a fill is).
//!
//! The crate does no I/O and depends only on the slicing engine's types.

pub mod checks;
pub mod clock;
pub mod fill;
pub mod finding;
pub mod input;
pub mod regions;
pub mod runner;
pub mod runs;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod union_find;

pub use fill::{CavityFill, FillSlab, PlateRect, erase_stack, fill_stack};
pub use finding::{
    CheckKind, Finding, MAX_FOOTPRINT_LAYERS, PlatePoint, RoleScope, Severity, SuggestedFix,
};
pub use input::{LayerSpan, ReviewInput, ReviewInputError, ReviewLeaf, Role};
pub use regions::{
    AIR_CONNECTIVITY, Connectivity, LayerLinks, LayerPixels, LayerRegions, Overlap, PixelBox,
    PlateRegions, Region, SOLID_CONNECTIVITY, label_runs, measure_runs,
};
pub use runner::{
    ReviewContext, ReviewOptions, ReviewReport, ReviewTimings, review, review_regions, review_with,
};
pub use runs::Run;
pub use union_find::{Groups, RegionRef, UnionFind};
