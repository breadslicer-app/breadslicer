/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The scene-slicing application service.
//!
//! - [`slice_plate`] slices a scene for a printer + material pairing and yields
//!   a [`SlicedPlate`].
//! - [`schedule_for_selection`] resolves the layer schedule such a slice would
//!   use, without slicing.

use std::fmt;

use sc_print_primitives::{AaLevel, PositiveFiniteMm};
use sc_print_profiles::{EffectiveMaterial, PrinterProfile};
use sc_scene::Scene;
use sc_sl1cer::contour::{ContourStack, LayerAlignmentError, merge_stacks};
use sc_sl1cer::error::SliceSceneError;
use sc_sl1cer::profile::{LayerSchedule, PackingMode, SliceProfile};
use sc_sl1cer::{CancelToken, LeafSlices, SliceOutcome, SliceProgress, slice_scene_with_progress};

use crate::prepare::{UnsliceableSelection, slice_profile_for_selection};

/// The per-print slicing inputs that come from neither profile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlicingParams {
    /// Per-print layer-height choice; `None` resolves it from the printer +
    /// material pairing.
    pub layer_height_request: Option<PositiveFiniteMm>,
    /// Thicker first layer; `None` inherits the layer height.
    pub first_layer_height: Option<PositiveFiniteMm>,
    /// Anti-aliasing level forwarded to the engine's slice profile.
    pub aa_level: AaLevel,
}

/// Why [`slice_plate`] produced no slices.
#[derive(Debug)]
pub enum SlicePlateError {
    /// The selection or the scene yields no slice profile.
    Prepare(UnsliceableSelection),
    /// The engine rejected the scene.
    Engine(SliceSceneError),
}

impl fmt::Display for SlicePlateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Prepare(err) => err.fmt(f),
            Self::Engine(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for SlicePlateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Prepare(err) => Some(err),
            Self::Engine(err) => Some(err),
        }
    }
}

/// How a [`slice_plate`] run ended.
#[derive(Debug)]
pub enum SliceRun {
    /// The run finished and produced the plate's slices.
    Completed(SlicedPlate),
    /// The cancel token was tripped; no slices were produced.
    Cancelled,
}

/// One completed slicing run: every leaf's contour stack, all on the layer grid
/// of one shared schedule.
#[derive(Debug)]
pub struct SlicedPlate {
    pub(crate) leaves: Vec<LeafSlices>,
    pub(crate) profile: SliceProfile,
}

impl SlicedPlate {
    /// The per-leaf stacks, in the engine's discovery order.
    pub fn leaves(&self) -> &[LeafSlices] {
        &self.leaves
    }

    /// Return the layer schedule shared by every leaf stack.
    pub fn schedule(&self) -> &LayerSchedule {
        self.profile.layer_schedule()
    }

    /// The engine input the run was sliced with.
    pub fn profile(&self) -> &SliceProfile {
        &self.profile
    }

    /// Merge the per-leaf stacks into the whole plate's contours.
    ///
    /// Fails only when stacks disagree on their layer grid; the leaves of one
    /// run share one schedule and always merge.
    pub fn merge(self) -> Result<PlateContours, LayerAlignmentError> {
        let stack = merge_stacks(
            self.leaves
                .into_iter()
                .map(LeafSlices::into_stack)
                .collect(),
        )?;
        Ok(PlateContours {
            stack,
            schedule: *self.profile.layer_schedule(),
        })
    }
}

/// The whole plate's contour stack and the layer schedule it was cut on.
#[derive(Debug)]
pub struct PlateContours {
    stack: ContourStack,
    schedule: LayerSchedule,
}

impl PlateContours {
    /// Pair a stack with the layer schedule it was cut on.
    ///
    /// Only the layer counts are compared, so a stack cut on another grid with
    /// the same number of layers is accepted.
    pub fn new(stack: ContourStack, schedule: LayerSchedule) -> Result<Self, MismatchedLayerCount> {
        let stack_layers = stack.layers().len();
        let schedule_layers = schedule.total_layers();
        if u32::try_from(stack_layers) != Ok(schedule_layers) {
            return Err(MismatchedLayerCount {
                stack_layers,
                schedule_layers,
            });
        }
        Ok(Self { stack, schedule })
    }

    /// Return the plate's contour stack.
    pub fn stack(&self) -> &ContourStack {
        &self.stack
    }

    /// Return the layer schedule the stack was cut on.
    pub fn schedule(&self) -> &LayerSchedule {
        &self.schedule
    }

    /// Split the pair into the stack and its schedule.
    pub fn into_parts(self) -> (ContourStack, LayerSchedule) {
        (self.stack, self.schedule)
    }
}

/// Returned when a stack and a schedule disagree on the number of layers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MismatchedLayerCount {
    /// Layers in the stack.
    pub stack_layers: usize,
    /// Layers in the schedule.
    pub schedule_layers: u32,
}

impl fmt::Display for MismatchedLayerCount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the contour stack has {} layers, but its layer schedule has {}",
            self.stack_layers, self.schedule_layers,
        )
    }
}

impl std::error::Error for MismatchedLayerCount {}

/// Slice every mesh leaf of `scene` for the chosen printer and material:
/// resolve the layer schedule from the profiles and `params`, then run the
/// engine under `cancel`, reporting per-layer progress through `on_progress`.
pub fn slice_plate<F>(
    scene: &Scene,
    printer: &PrinterProfile,
    material: &EffectiveMaterial,
    params: &SlicingParams,
    cancel: &CancelToken,
    on_progress: F,
) -> Result<SliceRun, SlicePlateError>
where
    F: Fn(SliceProgress) + Sync,
{
    let profile =
        profile_for(scene, printer, material, params).map_err(SlicePlateError::Prepare)?;
    match slice_scene_with_progress(scene, &profile, cancel, on_progress)
        .map_err(SlicePlateError::Engine)?
    {
        SliceOutcome::Cancelled => Ok(SliceRun::Cancelled),
        SliceOutcome::Completed(leaves) => Ok(SliceRun::Completed(SlicedPlate { leaves, profile })),
    }
}

/// Resolve the layer schedule that slicing `scene` with this selection would
/// use, without slicing.
pub fn schedule_for_selection(
    scene: &Scene,
    printer: &PrinterProfile,
    material: &EffectiveMaterial,
    params: &SlicingParams,
) -> Result<LayerSchedule, UnsliceableSelection> {
    Ok(*profile_for(scene, printer, material, params)?.layer_schedule())
}

fn profile_for(
    scene: &Scene,
    printer: &PrinterProfile,
    material: &EffectiveMaterial,
    params: &SlicingParams,
) -> Result<SliceProfile, UnsliceableSelection> {
    slice_profile_for_selection(
        scene,
        printer,
        material,
        params.layer_height_request,
        params.first_layer_height,
        params.aa_level,
        PackingMode::None,
    )
}
