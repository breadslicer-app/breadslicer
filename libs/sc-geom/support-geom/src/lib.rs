//! Geometry the support crates share: vectors and matrices, rays and the
//! model's BVH, small helpers, and run timings.

// `!(x > 0)` reads NaN as failing every test, and the conditions keep that
// form rather than restate it per call site.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

pub mod bvh;
pub mod geom;
pub mod perf;
pub mod utils;
