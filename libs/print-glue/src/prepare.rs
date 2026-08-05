/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Preparation of a scene's slice profile.
//!
//! [`slice_profile_for_scene`] is the derivation both application
//! frontends share: measure the scene's top, refuse a scene with no
//! geometry or one whose top is at or below the build plate (z = 0),
//! fall back to the layer height when no first-layer height is
//! chosen, derive the layer count that covers the scene, and project
//! the printer hardware onto the engine's input port. The layer
//! height is an input here; resolving it against the printer and
//! material profiles is the caller's step.

use std::fmt;

use sc_mesh_core::scene::Scene;
use sc_mesh_core::scene::error::SceneError;
use sc_print_primitives::PositiveFiniteMm;
use sc_print_profiles::PrinterHardware;
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
    /// The scene's top is at or below the build plate (z = 0), so
    /// nothing sits above the plate to slice.
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

/// Build a [`SliceProfile`] for `scene` from the printer hardware
/// plus the per-print slicing parameters.
///
/// Rules, in order:
///
/// 1. The scene must contribute geometry
///    ([`UnsliceableScene::NoGeometry`]).
/// 2. Slicing runs upward from the build plate at z = 0, so the
///    scene's top must sit above it
///    ([`UnsliceableScene::TopAtOrBelowPlate`]).
/// 3. An absent first-layer height falls back to the layer height.
/// 4. The layer count is the smallest count whose slab layout covers
///    the scene's top (see [`total_layers_for_height`]).
///
/// The layer height is taken as given; resolving it against the
/// printer and material profiles is the caller's responsibility.
pub fn slice_profile_for_scene(
    scene: &Scene,
    hardware: &PrinterHardware,
    layer_height: PositiveFiniteMm,
    first_layer_height: Option<PositiveFiniteMm>,
    aa_level: AaLevel,
    packing_mode: PackingMode,
) -> Result<SliceProfile, UnsliceableScene> {
    let aabb = scene
        .aabb()
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
