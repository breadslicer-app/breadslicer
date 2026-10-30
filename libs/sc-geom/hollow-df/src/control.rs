//! Cancellation and progress for the long stages of a hollow.
//!
//! DragonFruit runs its hollow to completion on a native worker; SoulCrafted
//! runs it as a cancellable job, so every stage reports itself and every
//! long loop polls the token.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Shared flag a caller sets to stop a running hollow at the next poll.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

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

/// The run was cancelled through its [`CancelToken`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

/// What a stage reports to: an optional cancel check and an optional
/// progress sink. Both are shared with the worker threads of a stage.
#[derive(Clone, Copy, Default)]
pub struct Control<'a> {
    cancelled: Option<&'a (dyn Fn() -> bool + Sync)>,
    token: Option<&'a CancelToken>,
    progress: Option<&'a (dyn Fn(Stage) + Sync)>,
}

impl<'a> Control<'a> {
    /// Never cancelled, reports nowhere.
    pub fn none() -> Self {
        Self::default()
    }

    /// Cancelled through `token`, reporting to `progress`.
    pub fn new(
        token: Option<&'a CancelToken>,
        progress: Option<&'a (dyn Fn(Stage) + Sync)>,
    ) -> Self {
        Self {
            cancelled: None,
            token,
            progress,
        }
    }

    /// Cancelled when `cancelled` says so (another job system's flag).
    pub fn with_check(
        cancelled: &'a (dyn Fn() -> bool + Sync),
        progress: Option<&'a (dyn Fn(Stage) + Sync)>,
    ) -> Self {
        Self {
            cancelled: Some(cancelled),
            token: None,
            progress,
        }
    }

    #[inline]
    pub fn is_cancelled(&self) -> bool {
        self.token.is_some_and(CancelToken::is_cancelled) || self.cancelled.is_some_and(|f| f())
    }

    #[inline]
    pub fn check(&self) -> Result<(), Cancelled> {
        if self.is_cancelled() {
            Err(Cancelled)
        } else {
            Ok(())
        }
    }

    /// Polls for cancellation, then reports `stage` as started.
    pub fn stage(&self, stage: Stage) -> Result<(), Cancelled> {
        self.check()?;
        if let Some(progress) = self.progress {
            progress(stage);
        }
        Ok(())
    }
}
