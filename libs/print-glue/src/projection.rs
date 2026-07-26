/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Projection from a resolved print configuration to a slice
//! profile.

use std::fmt;

use sc_print_primitives::PositiveFiniteMm;
use sc_print_profiles::ResolvedPrintConfig;
use sc_sl1cer::profile::{AaLevel, InvalidLayerSchedule, LayerSchedule, PackingMode, SliceProfile};

/// Why [`project_to_slice_profile`] could not build a
/// [`SliceProfile`].
#[derive(Debug)]
pub enum ProjectionError {
    /// The layer schedule could not be built from the resolved
    /// layer height, the first-layer height, and the layer count.
    LayerSchedule(InvalidLayerSchedule),
}

impl fmt::Display for ProjectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LayerSchedule(err) => {
                write!(f, "could not build the layer schedule: {err}")
            }
        }
    }
}

impl std::error::Error for ProjectionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::LayerSchedule(err) => Some(err),
        }
    }
}

/// Build a [`SliceProfile`] from a resolved print configuration plus
/// the per-print slicing parameters that live outside the printer
/// and material profiles.
///
/// The build volume, pixel resolution, and mirror flags come
/// verbatim from the printer hardware; the layer height comes from
/// the resolved material. The anti-aliasing level, the packing
/// mode, the first-layer height, and the layer count are slicing
/// scene parameters supplied by the caller (the layer count is
/// derived from the mesh bounds, the first-layer height is a
/// per-print choice, and the anti-aliasing level is an app-level
/// setting).
///
/// The build volume is taken as-is from the printer, including its
/// Z extent: the slicing range is governed by the layer schedule,
/// not the build volume's Z, so the printer's machine height is
/// carried through unchanged as metadata.
pub fn project_to_slice_profile(
    config: &ResolvedPrintConfig,
    aa_level: AaLevel,
    packing_mode: PackingMode,
    first_layer_height_mm: PositiveFiniteMm,
    total_layers: u32,
) -> Result<SliceProfile, ProjectionError> {
    let hardware = &config.printer.hardware;
    let layer_height_mm = config.material_resolved.layer_height_mm;

    let layer_schedule = LayerSchedule::uniform(
        layer_height_mm.value(),
        first_layer_height_mm.value(),
        total_layers,
    )
    .map_err(ProjectionError::LayerSchedule)?;

    Ok(SliceProfile::new(
        hardware.build_volume(),
        hardware.pixel_resolution(),
        layer_schedule,
        hardware.mirror(),
        aa_level,
        packing_mode,
    ))
}

/// Pick the smallest layer count whose slab layout covers a model up to
/// `z_max` millimetres. Slab 0 spans `[0, first_layer_height]`; slab
/// `k >= 1` spans
/// `[first_layer_height + (k-1)*layer_height, first_layer_height + k*layer_height]`.
/// The result is the smallest `n` with
/// `first_layer_height + (n-1) * layer_height >= z_max`.
pub fn total_layers_for_height(z_max: f64, layer_height: f64, first_layer_height: f64) -> u32 {
    if z_max <= first_layer_height {
        return 1;
    }
    let beyond_first = z_max - first_layer_height;
    let extra = (beyond_first / layer_height).ceil() as u32;
    1 + extra
}

#[cfg(test)]
mod total_layers_tests {
    use super::total_layers_for_height;

    #[test]
    fn returns_one_when_model_fits_in_the_first_slab() {
        assert_eq!(total_layers_for_height(0.04, 0.1, 0.05), 1);
        assert_eq!(total_layers_for_height(0.05, 0.1, 0.05), 1);
    }

    #[test]
    fn adds_extra_layers_to_cover_above_first_slab() {
        // first slab covers [0, 0.05]; each extra slab is 0.1 thick.
        // z_max = 0.5: needs (0.5 - 0.05) / 0.1 = 4.5, ceil = 5 extra slabs.
        assert_eq!(total_layers_for_height(0.5, 0.1, 0.05), 6);
    }

    #[test]
    fn uniform_schedule_matches_classical_count() {
        // h0 = h = 0.2, z_max = 2.0. classical: ceil(2.0 / 0.2) = 10.
        assert_eq!(total_layers_for_height(2.0, 0.2, 0.2), 10);
    }
}
