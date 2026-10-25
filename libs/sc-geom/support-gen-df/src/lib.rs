//! Native Rust port of the DragonFruit support system for the SoulCrafted Slicer.
//!
//! Modules mirror DragonFruit's `src/supports/` tree in snake case, one Rust
//! file per DragonFruit file, each naming its origin. See `NOTICE`.

pub mod js_math;
pub mod supports;
pub mod three;
pub mod three_mesh_bvh;
