//! Hollowing for Breadslicer: a model's cavity, its infill and its drains.
//!
//! - [`volume`]: the model as voxels (inside by winding number along the
//!   grid's lines) and how far each is from the surface;
//! - the cavity: exact wall distances near the wall, the cubes carved, and
//!   the wall as a closed contour, the same surface for the preview and the
//!   result (#29);
//! - [`infill`]: lattice or pillar beams across the cavity;
//! - [`drain`]: a closed mesh minus a convex prism, edge-manifold by
//!   construction; [`drill`]: drains into a model that is already hollow;
//! - [`engine`]: the interface: grid request with the effective voxel
//!   reported, preview and apply, solid areas, drains, volumes;
//! - [`mesh`], [`bvh`], [`topology`]: the mesh, ray and nearest-surface
//!   queries, and the edge checks the hollow uses.
//!
//! See `NOTICE` for licensing.

mod air;
pub mod bvh;
mod cavity;
mod contour;
pub mod control;
mod distance;
pub mod drain;
pub mod drill;
pub mod engine;
pub mod grid;
pub mod infill;
pub mod mesh;
pub mod overlap;
pub mod shapes;
pub mod solid;
pub mod topology;
pub mod volume;
mod wall;

pub use control::{CancelToken, Cancelled, Control, Stage};
pub use drill::{DrillTarget, Drilled, sealed_cavities};
pub use engine::{
    DrainDepth, DrainHole, DrainPlug, DrainReport, GridInfo, GridRequest, HollowApplied,
    HollowError, HollowFill, HollowPreview, HollowSettings, HollowStats, ResolutionBound, Session,
    SolidArea,
};
pub use infill::InfillMode;
pub use mesh::{Aabb, IndexedMesh, Vec3};
pub use solid::LassoRegion;
pub use volume::Volume;
