//! Cancellation and progress for the long stages of a hollow.
//!
//! A hollow runs as a cancellable job: every stage reports itself and every
//! long loop polls the token.

pub use bread_engine_base::{CancelToken, Cancelled};

/// The stages a hollow passes through, in order. Session construction covers
/// the first three, a preview or an apply the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    /// Finding where the grid's lines cross the surface.
    Voxelize,
    /// Deciding which voxels are inside.
    FloodFill,
    /// How far each voxel is, roughly, from the surface.
    DistanceField,
    /// Exact distance to the surface near the wall (measured once per
    /// session and wall band, then reused).
    WallDistance,
    /// Deciding which voxels stay material.
    KeepMask,
    /// Contouring the cavity wall.
    CavitySurface,
    /// Building lattice or pillar beams.
    Infill,
    /// Checking the merged mesh and retrying with less smoothing.
    Stabilize,
    /// Cutting drain holes.
    Drains,
    /// Final edge-manifold check.
    Validate,
}

impl Stage {
    pub fn label(self) -> &'static str {
        match self {
            Stage::Voxelize => "Voxelizing surface",
            Stage::FloodFill => "Filling outside",
            Stage::DistanceField => "Measuring wall distance",
            Stage::WallDistance => "Measuring exact wall distance",
            Stage::KeepMask => "Choosing wall voxels",
            Stage::CavitySurface => "Building cavity surface",
            Stage::Infill => "Building infill",
            Stage::Stabilize => "Checking hollow mesh",
            Stage::Drains => "Cutting drain holes",
            Stage::Validate => "Validating result",
        }
    }
}

/// What a hollowing stage reports to.
pub type Control<'a> = bread_engine_base::Control<'a, Stage>;
