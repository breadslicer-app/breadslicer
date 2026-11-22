//! Automatic bracing: diagonal braces between neighbouring support shafts,
//! and kickstands where braces alone cannot hold a shaft both ways.

pub mod auto_brace;
mod grouping;
mod hand;
mod ladder;
pub mod mesh_clearance;
mod pairing;
mod samples;
pub mod settings;
pub mod two_axis_detection;
