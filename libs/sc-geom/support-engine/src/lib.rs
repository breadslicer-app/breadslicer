//! Supports for the SoulCrafted Slicer: where they go, what they are, how they
//! are braced, and their mesh. Some modules derive from earlier work; see
//! `NOTICE`.

// `!(x > 0)` reads NaN as failing every test, and the conditions keep that
// form rather than restate it per call site.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

pub mod features;
pub mod native;
pub mod perf;
pub mod pipeline;
pub mod supports;
pub mod geom;
pub mod bvh;
pub mod utils;
pub mod volume_analysis;
