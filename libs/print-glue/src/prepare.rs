/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Preparation of a scene's slice profile for a chosen printer + material: the
//! resolved layer height, the first layer on the printer's Z step, and the
//! layer count that covers the scene.

use std::fmt;

use sc_print_primitives::PositiveFiniteMm;
use sc_print_profiles::{
    EffectiveMaterial, PrinterHardware, PrinterProfile, ResolveError, resolve_layer_height,
    snap_to_z_step,
};
use sc_scene::Scene;
use sc_scene::error::SceneError;
use sc_sl1cer::profile::{AaLevel, PackingMode, SliceProfile};

use crate::projection::{
    UnschedulableHeight, project_hardware_to_slice_profile, total_layers_for_height,
};

/// Why a scene cannot yield a slice profile.
#[derive(Debug)]
pub enum UnsliceableScene {
    /// The scene's bounding box could not be computed.
    Aabb(SceneError),
    /// The scene contributes no geometry to slice.
    NoGeometry,
    /// The scene's top is at or below the build plate (z = 0), so nothing sits
    /// above the plate to slice.
    TopAtOrBelowPlate {
        /// The scene's top in world millimetres.
        z_max_mm: f64,
    },
    /// The scene height does not yield a schedulable layer count.
    Unschedulable(UnschedulableHeight),
}

impl fmt::Display for UnsliceableScene {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Aabb(err) => {
                write!(f, "the scene's bounding box could not be computed: {err}")
            }
            Self::NoGeometry => write!(f, "the scene contributes no geometry to slice"),
            Self::TopAtOrBelowPlate { z_max_mm } => write!(
                f,
                "the scene's top is at z = {z_max_mm} mm, at or below the build plate at z = 0",
            ),
            Self::Unschedulable(err) => {
                write!(f, "the scene height yields no valid layer count: {err}")
            }
        }
    }
}

impl std::error::Error for UnsliceableScene {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Aabb(err) => Some(err),
            Self::Unschedulable(err) => Some(err),
            Self::NoGeometry | Self::TopAtOrBelowPlate { .. } => None,
        }
    }
}

/// Why a printer + material selection cannot slice a scene.
#[derive(Debug)]
pub enum UnsliceableSelection {
    /// No layer height resolves for the printer + material pairing.
    LayerHeight(ResolveError),
    /// The scene cannot yield a slice profile.
    Scene(UnsliceableScene),
}

impl fmt::Display for UnsliceableSelection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LayerHeight(err) => write!(
                f,
                "no layer height resolves for this printer and material: {err}",
            ),
            Self::Scene(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for UnsliceableSelection {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::LayerHeight(err) => Some(err),
            Self::Scene(err) => Some(err),
        }
    }
}

/// Build a [`SliceProfile`] for `scene` from a chosen printer and material.
///
/// Geometry slicing consumes exactly 1 material parameter, the layer height,
/// resolved here through [`resolve_layer_height`] (with `layer_height_request`
/// as the per-print choice, constraint-checked by the resolver). The full print
/// configuration (exposure, motion, bottom layers) is the input of the
/// printer-file export step and never gates slicing. The scene derivation
/// itself is [`slice_profile_for_scene`].
///
/// A chosen first-layer height is quantised onto the printer's z-step grid
/// ([`snap_to_z_step`]); the declared layer-height range does not bind it,
/// since first layers legitimately run thicker than the normal-layer curing
/// range. The fallback (no first-layer choice) inherits the resolved layer
/// height, which is already on the grid.
pub(crate) fn slice_profile_for_selection(
    scene: &Scene,
    printer: &PrinterProfile,
    material: &EffectiveMaterial,
    layer_height_request: Option<PositiveFiniteMm>,
    first_layer_height: Option<PositiveFiniteMm>,
    aa_level: AaLevel,
    packing_mode: PackingMode,
) -> Result<SliceProfile, UnsliceableSelection> {
    let layer_height = resolve_layer_height(printer, material, layer_height_request)
        .map_err(UnsliceableSelection::LayerHeight)?;
    let first_layer_height =
        first_layer_height.map(|value| snap_to_z_step(&printer.constraints, value));
    slice_profile_for_scene(
        scene,
        &printer.hardware,
        layer_height,
        first_layer_height,
        aa_level,
        packing_mode,
    )
    .map_err(UnsliceableSelection::Scene)
}

/// Build a [`SliceProfile`] for `scene` from the printer hardware plus the
/// per-print slicing parameters.
///
/// Rules, in order:
///
/// 1. The scene must contribute geometry ([`UnsliceableScene::NoGeometry`]).
/// 2. Slicing runs upward from the build plate at z = 0, so the scene's top
///    must sit above it ([`UnsliceableScene::TopAtOrBelowPlate`]).
/// 3. An absent first-layer height falls back to the layer height.
/// 4. The layer count is the smallest count whose slab layout covers the
///    scene's top, its highest vertex (see [`total_layers_for_height`]).
///
/// The layer height is taken as given; resolving it against the printer and
/// material profiles is the caller's responsibility.
pub(crate) fn slice_profile_for_scene(
    scene: &Scene,
    hardware: &PrinterHardware,
    layer_height: PositiveFiniteMm,
    first_layer_height: Option<PositiveFiniteMm>,
    aa_level: AaLevel,
    packing_mode: PackingMode,
) -> Result<SliceProfile, UnsliceableScene> {
    // Tight, not the transformed-box bound: a rotated model's box can stand
    // tens of millimetres above its top, all of it empty layers.
    let aabb = scene
        .tight_aabb()
        .map_err(UnsliceableScene::Aabb)?
        .ok_or(UnsliceableScene::NoGeometry)?;
    let z_max = f64::from(aabb.max().coords()[2]);
    if z_max <= 0.0 {
        return Err(UnsliceableScene::TopAtOrBelowPlate { z_max_mm: z_max });
    }
    let first_layer_height = first_layer_height.unwrap_or(layer_height);
    let total_layers =
        total_layers_for_height(z_max, layer_height.value(), first_layer_height.value())
            .map_err(UnsliceableScene::Unschedulable)?;
    Ok(project_hardware_to_slice_profile(
        hardware,
        layer_height,
        aa_level,
        packing_mode,
        first_layer_height,
        total_layers,
    ))
}
