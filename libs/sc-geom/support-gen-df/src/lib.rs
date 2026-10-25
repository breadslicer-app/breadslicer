//! Native Rust port of the DragonFruit support system for the SoulCrafted Slicer.
//!
//! Modules mirror DragonFruit's `src/supports/` tree in snake case, one Rust
//! file per DragonFruit file, each naming its origin. See `NOTICE`.

// `!(x > 0)` is how DragonFruit's comparisons treat NaN (it fails every test);
// the ported conditions keep that form rather than restate it per call site.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

pub mod clipper;
pub mod js_math;
pub mod native;
pub mod pipeline;
pub mod supports;
pub mod three;
pub mod three_mesh_bvh;
pub mod utils;
pub mod volume_analysis;
