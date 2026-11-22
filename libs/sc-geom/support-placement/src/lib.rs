//! Where supports go: a model's islands, the contacts the zone rules ask
//! for, how each is routed and built, and a model's run.

// `!(x > 0)` reads NaN as failing every test, and the conditions keep that
// form rather than restate it per call site.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

pub mod auto_support;
pub mod native;
pub mod placement_logic;
pub mod placement_logic_v3;
pub mod support_types;
pub mod volume_analysis;
