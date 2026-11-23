//! One call from a scene's models to their supports and rafts: scans,
//! islands, placement, bracing and the mesh, in turn.

// `!(x > 0)` reads NaN as failing every test, and the conditions keep that
// form rather than restate it per call site.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

mod pipeline;
pub mod plate;

pub use pipeline::*;
