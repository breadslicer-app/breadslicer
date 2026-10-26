//! Native port of DragonFruit's voxel hollowing engine for the SoulCrafted
//! Slicer, with SynrgStudio's model-local grid, parallel voxelization and
//! surface-only preview, and an exact pure-Rust drain cut.
//!
//! - [`hollowing`]: DragonFruit's engine (`hollow_voxel`, `HollowSession`),
//!   ported from `rust/dragonfruit-mesh-repair/src/hollowing.rs` with the same
//!   arithmetic, so its outputs match DragonFruit's own build;
//! - [`drain`]: closed mesh minus a convex prism, edge-manifold by
//!   construction (DragonFruit uses manifold3d here);
//! - [`punch`]: DragonFruit's `punch_cylinders` on that cut;
//! - [`engine`]: SoulCrafted's interface: grid request with the effective
//!   voxel reported, preview and apply, solid areas, drains, volumes;
//! - [`mesh`], [`bvh`], [`topology`], [`smoothing`]: the parts of
//!   DragonFruit's mesh core the engine uses.
//!
//! See `NOTICE` for origin and licensing, and `docs/sc/HOLLOW-PORT.md`.

pub mod bvh;
pub mod control;
pub mod drain;
pub mod engine;
pub mod hollowing;
pub mod mesh;
pub mod punch;
pub mod shapes;
mod smoothing;
pub mod topology;

pub use control::{CancelToken, Cancelled, Control, Stage};
pub use engine::{
    DrainDepth, DrainHole, DrainReport, GridInfo, GridRequest, HollowApplied, HollowError,
    HollowFill, HollowPreview, HollowSettings, HollowStats, ResolutionBound, Session, SolidArea,
};
pub use hollowing::InfillMode;
pub use mesh::{Aabb, IndexedMesh, Vec3};
