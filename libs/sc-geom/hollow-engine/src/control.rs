//! Cancellation and progress for the long stages of a hollow.
//!
//! DragonFruit runs its hollow to completion on a native worker; SoulCrafted
//! runs it as a cancellable job, so every stage reports itself and every
//! long loop polls the token.

pub use engine_base::{CancelToken, Cancelled};

/// The stages a hollow passes through, in order. Session construction covers
/// the first four, a preview or an apply the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    /// Marking the voxels the surface passes through.
    Voxelize,
    /// Flood-filling the outside.
    FloodFill,
    /// Restoring sealed voids of the source with a parity test.
    ParityRefine,
    /// Two-pass chamfer distance to the surface.
    DistanceField,
    /// Exact distance to the surface near the wall (measured once per
    /// session and wall band, then reused).
    WallDistance,
    /// Deciding which voxels stay material.
    KeepMask,
    /// Contouring (and smoothing) the cavity wall.
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
            Stage::ParityRefine => "Restoring sealed voids",
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
pub type Control<'a> = engine_base::Control<'a, Stage>;
