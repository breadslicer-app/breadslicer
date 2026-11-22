//! Automatic bracing: diagonal braces between neighbouring support shafts,
//! and kickstands where braces alone cannot hold a shaft both ways.

// `!(x > 0)` reads NaN as failing every test, and the conditions keep that
// form rather than restate it per call site.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

pub mod auto_brace;
mod grouping;
mod hand;
mod ladder;
mod pairing;
mod samples;
