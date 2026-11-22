//! The supports' mesh as the slicer reads it, and the rafts they stand on.

// `!(x > 0)` reads NaN as failing every test, and the conditions keep that
// form rather than restate it per call site.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

pub mod features;
pub mod rafts;
