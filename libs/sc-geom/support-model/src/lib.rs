//! What a support is: its kinds and parts (tips, shafts, roots), the state
//! that holds them, the settings and zone rules they are made with, and the
//! checks of a shape against the model.

// `!(x > 0)` reads NaN as failing every test, and the conditions keep that
// form rather than restate it per call site.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

pub mod auto_bracing;
pub mod auto_support;
pub mod bezier;
pub mod collision_avoidance;
pub mod cone_axis_policy;
pub mod constants;
pub mod model_mesh;
pub mod numeric_constraint;
pub mod rendering;
pub mod settings;
pub mod shafts;
pub mod support_kinds;
pub mod tips;
pub mod types;
