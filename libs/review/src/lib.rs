/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Pre-flight review of a sliced resin plate.
//!
//! `sc-review` reads the contours of one slicing run's models and what the
//! engine knows of its supports, and reports what is likely to fail before
//! anything is printed: islands, sudden growth of the surface, resin traps
//! and suction cups, and output problems (plate edges, maximum height, empty
//! layers). Each [`Finding`] names its layers and a point to centre the
//! camera on, and may suggest a one-click fix.
//!
//! # Pipeline
//!
//! 1. Build a [`ReviewInput`]: the model leaves' contour stacks, the layer
//!    schedule, the printer's screen and its maximum height
//!    ([`ReviewInput::from_profile`] takes the last three from the engine's
//!    `SliceProfile`), and the supports as data. Support geometry never
//!    comes in, so the cost grows with the model and not with the supports.
//! 2. [`review`] (or [`review_with`] for options, timings, progress and
//!    Cancel) sweeps the layers once ([`regions`]): each is rasterised, cut
//!    of what is thinner than a pixel, split into solid and air regions and
//!    linked with the layer below, in parallel, keeping small tables and no
//!    image.
//! 3. Each check in [`checks`] reads the tables through a [`ReviewContext`]
//!    and returns findings; the runner sorts them by layer, then check.
//!
//! The review also gives what it took off the layers by itself, debris and
//! stray pixels ([`ReviewReport::erased`]): [`erase_stack`] turns that into
//! a stack that, merged with the plate, leaves those pixels out of the
//! preview and the printer file, on this same slice.
//!
//! An island's [`ReviewReport::islands`] entry has its pixels, layer by layer:
//! [`plan_tips`] turns the list into the points to put a tip on (#437).
//!
//! A small sealed trap is offered a fill (#132, #431): the report carries
//! its pixels as a [`CavityFill`], and [`fill_stack`] turns the fills kept
//! from one review into a contour stack that cures them solid on the next
//! slice of the same scene.
//!
//! The crate does no I/O and depends only on the slicing engine's types.

pub mod checks;
pub mod fill;
pub mod finding;
pub mod input;
pub mod regions;
pub mod runner;
pub mod runs;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod tip_plan;
pub mod tips;
pub mod union_find;

pub use checks::islands::{Held, Island, Part};
pub use fill::{CavityFill, FillSlab, PlateRect, erase_stack, fill_stack};
pub use finding::{
    CheckKind, Finding, MAX_FOOTPRINT_LAYERS, PlatePoint, RoleScope, Severity, SuggestedFix,
};
pub use input::{LayerSpan, ReviewInput, ReviewInputError, ReviewLeaf};
pub use regions::{
    AIR_CONNECTIVITY, Connectivity, DEFAULT_MIN_LINK_MM2, LayerLinks, LayerPixels, LayerRegions,
    Overlap, PartStart, PixelBox, PlateRegions, Region, SOLID_CONNECTIVITY, SweepOptions,
    SweepWatch, label_runs, link_holds, measure_runs,
};
pub use runner::{
    ReviewContext, ReviewOptions, ReviewReport, ReviewTimings, check_regions, review, review_with,
};
pub use runs::Run;
pub use tip_plan::{IslandLayer, IslandPixels, PlannedTip, TipPlan, TipPlanOptions, plan_tips};
pub use tips::TipCone;
pub use union_find::{Groups, RegionRef, UnionFind};
