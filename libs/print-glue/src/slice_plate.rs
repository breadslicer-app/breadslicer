/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The scene-slicing application service.
//!
//! - [`slice_plate`] slices a scene for a printer + material pairing and yields
//!   a [`SlicedPlate`], with the material's scale compensation applied to each
//!   model and its supports (#227).
//! - [`schedule_for_selection`] resolves the layer schedule such a slice would
//!   use, without slicing.

use std::fmt;
use std::num::NonZeroU32;
use std::sync::Arc;

use sc_print_primitives::{AaLevel, PositiveFiniteMm, ScaleCompensation};
use sc_print_profiles::{EffectiveMaterial, PrinterProfile, resolve_scale_compensation};
use sc_scene::Scene;
use sc_scene::node::NodeId;
use sc_sl1cer::contour::{ContourStack, LayerAlignmentError, merge_stacks};
use sc_sl1cer::error::SliceSceneError;
use sc_sl1cer::profile::{LayerSchedule, PackingMode, SliceProfile, UniformSchedule};
use sc_sl1cer::{CancelToken, LeafSlices, SliceOutcome, SliceProgress, slice_scene_with_progress};

use crate::prepare::{UnsliceableSelection, slice_profile_for_selection};
use crate::scale_compensation::{
    LeafPivots, SupportOwners, compensated_heights, compensated_scene, scale_factors,
    support_box_mm,
};

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
    pub(crate) scale: ScaleCompensation,
    /// The point each leaf was scaled about in X and Y, for a compensation
    /// that changes anything.
    pub(crate) pivots: LeafPivots,
    /// The box the support leaves take once scaled.
    pub(crate) support_box_mm: Option<[f64; 5]>,
}

impl SlicedPlate {
    /// Return the per-leaf stacks, in the engine's discovery order.
    pub fn leaves(&self) -> &[LeafSlices] {
        &self.leaves
    }

    /// Return the layer schedule shared by every leaf stack.
    pub fn schedule(&self) -> &LayerSchedule {
        self.profile.layer_schedule()
    }

    /// Return the engine input the run was sliced with.
    pub fn profile(&self) -> &SliceProfile {
        &self.profile
    }

    /// Return the scale compensation the leaves were sliced with.
    pub fn scale_compensation(&self) -> ScaleCompensation {
        self.scale
    }

    /// Return where a height of `height_mm` above the plate in the scene
    /// lands in the sliced leaves, once scaled in Z about the plate.
    pub fn compensated_height_mm(&self, height_mm: f64) -> f64 {
        height_mm * scale_factors(self.scale)[2]
    }

    /// Return where `point` (millimetres, as the scene has it) of leaf `leaf`
    /// lands in the sliced leaves, once scaled about that leaf's pivot in X
    /// and Y and about the plate in Z.
    pub fn compensated_point_mm(&self, leaf: NodeId, point: [f64; 3]) -> [f64; 3] {
        let [fx, fy, fz] = scale_factors(self.scale);
        let [cx, cy] = self
            .pivots
            .iter()
            .find(|(id, _)| *id == leaf)
            .map_or([0.0, 0.0], |(_, pivot)| *pivot);
        [
            cx + (point[0] - cx) * fx,
            cy + (point[1] - cy) * fy,
            point[2] * fz,
        ]
    }

    /// Return the box the support leaves take once scaled, `[min x, min y,
    /// max x, max y, top]` in millimetres; `None` without support leaves. It
    /// is read from the meshes' vertices, whatever the layers.
    pub fn support_box_mm(&self) -> Option<[f64; 5]> {
        self.support_box_mm
    }

    /// Merge the per-leaf stacks into the whole plate's contours.
    ///
    /// Fails only when stacks disagree on their layer grid; the leaves of one
    /// run share one schedule and always merge.
    pub fn merge(self) -> Result<PlateContours, LayerAlignmentError> {
        self.merge_with(None)
    }

    /// Merge the per-leaf stacks, then `extra` stacks cut on the same
    /// schedule, into the whole plate's contours: solid added to the plate
    /// after slicing, such as the cavities a review fills (#132).
    ///
    /// Fails when a stack does not follow the run's layer grid.
    pub fn merge_with(
        self,
        extra: impl IntoIterator<Item = ContourStack>,
    ) -> Result<PlateContours, LayerAlignmentError> {
        let stack = merge_stacks(
            self.leaves
                .into_iter()
                .map(LeafSlices::into_stack)
                .chain(extra)
                .collect(),
        )?;
        Ok(PlateContours {
            stack: Arc::new(stack),
            schedule: *self.profile.layer_schedule(),
        })
    }
}

/// The whole plate's contour stack and the layer schedule it was cut on. The
/// stack is shared: a clone costs nothing, and the stack can outlive the holder
/// on another thread, as a save needs.
#[derive(Clone, Debug)]
pub struct PlateContours {
    stack: Arc<ContourStack>,
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
        Ok(Self {
            stack: Arc::new(stack),
            schedule,
        })
    }

    /// Return the plate's contour stack.
    pub fn stack(&self) -> &ContourStack {
        &self.stack
    }

    /// Return the plate's contour stack as a handle that shares it.
    pub fn shared_stack(&self) -> Arc<ContourStack> {
        Arc::clone(&self.stack)
    }

    /// Return the layer schedule the stack was cut on.
    pub fn schedule(&self) -> &LayerSchedule {
        &self.schedule
    }

    /// Keep only the first `layers` layers, and the schedule's slabs under
    /// them. A count at or past the stack's length keeps everything.
    pub(crate) fn truncated(self, layers: NonZeroU32) -> Self {
        let kept = usize::try_from(layers.get()).unwrap_or(usize::MAX);
        if kept >= self.stack.layers().len() {
            return self;
        }
        let LayerSchedule::Uniform(grid) = self.schedule;
        Self {
            stack: Arc::new(ContourStack::new_unchecked(
                self.stack.layers()[..kept].to_vec(),
            )),
            schedule: LayerSchedule::Uniform(UniformSchedule::new(
                grid.layer_height(),
                grid.first_layer_height(),
                layers,
            )),
        }
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
/// scale each model and its supports by the material's scale compensation,
/// resolve the layer schedule from the profiles and `params`, then run the
/// engine under `cancel`, reporting per-layer progress through `on_progress`.
///
/// `supports` names the support leaves and the model each stands on; every
/// other leaf is a model. With no compensation the scene is sliced as it is.
pub fn slice_plate<F>(
    scene: &Scene,
    printer: &PrinterProfile,
    material: &EffectiveMaterial,
    params: &SlicingParams,
    supports: &SupportOwners,
    cancel: &CancelToken,
    on_progress: F,
) -> Result<SliceRun, SlicePlateError>
where
    F: Fn(SliceProgress) + Sync,
{
    let scale = resolve_scale_compensation(printer, material);
    let (compensated, pivots) = compensated_scene(scene, scale, supports).unzip();
    let scene = compensated.as_ref().unwrap_or(scene);
    let profile =
        profile_for(scene, printer, material, params).map_err(SlicePlateError::Prepare)?;
    let support_box_mm = support_box_mm(scene, supports);
    match slice_scene_with_progress(scene, &profile, cancel, on_progress)
        .map_err(SlicePlateError::Engine)?
    {
        SliceOutcome::Cancelled => Ok(SliceRun::Cancelled),
        SliceOutcome::Completed(leaves) => Ok(SliceRun::Completed(SlicedPlate {
            leaves,
            profile,
            scale,
            pivots: pivots.unwrap_or_default(),
            support_box_mm,
        })),
    }
}

/// Resolve the layer schedule that slicing `scene` with this selection would
/// use, without slicing: the scene's height once scaled in Z by the
/// material's scale compensation, as [`slice_plate`] scales it.
pub fn schedule_for_selection(
    scene: &Scene,
    printer: &PrinterProfile,
    material: &EffectiveMaterial,
    params: &SlicingParams,
) -> Result<LayerSchedule, UnsliceableSelection> {
    let scale = resolve_scale_compensation(printer, material);
    let compensated = compensated_heights(scene, scale);
    let scene = compensated.as_ref().unwrap_or(scene);
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
