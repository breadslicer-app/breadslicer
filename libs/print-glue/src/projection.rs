/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Projection from a resolved print configuration to a slice
//! profile.

use std::fmt;

use sc_print_primitives::PositiveFiniteMm;
use sc_print_profiles::{PrinterHardware, ResolvedPrintConfig};
use sc_sl1cer::profile::{AaLevel, InvalidLayerSchedule, LayerSchedule, PackingMode, SliceProfile};

/// Why a scene height cannot be covered by a valid layer count.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UnschedulableHeight {
    /// `z_max` is NaN or infinite, so no layer count is defined for
    /// it.
    NonFiniteHeight { z_max_mm: f64 },
    /// Covering `z_max` needs more layers than a `u32` layer count
    /// can represent.
    LayerCountOverflow { z_max_mm: f64 },
}

impl fmt::Display for UnschedulableHeight {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonFiniteHeight { z_max_mm } => write!(
                f,
                "the scene height z = {z_max_mm} mm is not a finite value",
            ),
            Self::LayerCountOverflow { z_max_mm } => write!(
                f,
                "covering the scene height z = {z_max_mm} mm needs more layers than a schedule can hold",
            ),
        }
    }
}

impl std::error::Error for UnschedulableHeight {}

/// Why [`project_to_slice_profile`] could not build a
/// [`SliceProfile`].
#[derive(Debug)]
pub enum ProjectionError {
    /// The layer schedule could not be built from the resolved
    /// layer height, the first-layer height, and the layer count.
    LayerSchedule(InvalidLayerSchedule),
    /// The scene height does not yield a representable layer count.
    UnschedulableHeight(UnschedulableHeight),
}

impl fmt::Display for ProjectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LayerSchedule(err) => {
                write!(f, "could not build the layer schedule: {err}")
            }
            Self::UnschedulableHeight(err) => {
                write!(f, "could not derive the layer count: {err}")
            }
        }
    }
}

impl std::error::Error for ProjectionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::LayerSchedule(err) => Some(err),
            Self::UnschedulableHeight(err) => Some(err),
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
    project_hardware_to_slice_profile(
        &config.printer.hardware,
        config.material_resolved.layer_height_mm,
        aa_level,
        packing_mode,
        first_layer_height_mm,
        total_layers,
    )
}

/// Build a [`SliceProfile`] from the printer hardware and a layer height
/// directly, without a fully-resolved print configuration.
///
/// Geometry slicing needs only the hardware (build volume, pixel
/// resolution, mirror) plus the layer height; the print parameters
/// (exposure, lift, speeds) belong to the SLA output step, not to
/// producing contours. This lets a scene be sliced with any
/// printer + material pairing, even one whose exposure calibration does
/// not resolve.
pub fn project_hardware_to_slice_profile(
    hardware: &PrinterHardware,
    layer_height_mm: PositiveFiniteMm,
    aa_level: AaLevel,
    packing_mode: PackingMode,
    first_layer_height_mm: PositiveFiniteMm,
    total_layers: u32,
) -> Result<SliceProfile, ProjectionError> {
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
///
/// # Errors
///
/// - [`UnschedulableHeight::NonFiniteHeight`] when `z_max` is NaN or
///   infinite.
/// - [`UnschedulableHeight::LayerCountOverflow`] when the required
///   count does not fit in a `u32` (the guard also catches a NaN
///   quotient from out-of-contract layer heights, keeping the
///   function total for any f64 input).
pub fn total_layers_for_height(
    z_max: f64,
    layer_height: f64,
    first_layer_height: f64,
) -> Result<u32, UnschedulableHeight> {
    if !z_max.is_finite() {
        return Err(UnschedulableHeight::NonFiniteHeight { z_max_mm: z_max });
    }
    if z_max <= first_layer_height {
        return Ok(1);
    }
    let beyond_first = z_max - first_layer_height;
    let extra = (beyond_first / layer_height).ceil();
    // `extra < u32::MAX` keeps `1 + extra` representable; the negated
    // form is deliberate so a NaN `extra` also lands in the error arm.
    if !(extra.is_finite() && extra < f64::from(u32::MAX)) {
        return Err(UnschedulableHeight::LayerCountOverflow { z_max_mm: z_max });
    }
    Ok(1 + extra as u32)
}

#[cfg(test)]
mod total_layers_tests {
    use super::{UnschedulableHeight, total_layers_for_height};

    #[test]
    fn returns_one_when_model_fits_in_the_first_slab() {
        assert_eq!(total_layers_for_height(0.04, 0.1, 0.05), Ok(1));
        assert_eq!(total_layers_for_height(0.05, 0.1, 0.05), Ok(1));
    }

    #[test]
    fn adds_extra_layers_to_cover_above_first_slab() {
        // first slab covers [0, 0.05]; each extra slab is 0.1 thick.
        // z_max = 0.5: needs (0.5 - 0.05) / 0.1 = 4.5, ceil = 5 extra slabs.
        assert_eq!(total_layers_for_height(0.5, 0.1, 0.05), Ok(6));
    }

    #[test]
    fn uniform_schedule_matches_classical_count() {
        // h0 = h = 0.2, z_max = 2.0. classical: ceil(2.0 / 0.2) = 10.
        assert_eq!(total_layers_for_height(2.0, 0.2, 0.2), Ok(10));
    }

    #[test]
    fn absurd_heights_do_not_panic() {
        // An infinite or astronomically large z_max used to saturate
        // the f64-to-u32 cast at u32::MAX and overflow the `1 +`.
        for z_max in [f64::INFINITY, f64::NAN, 2.6e39] {
            let outcome = std::panic::catch_unwind(|| total_layers_for_height(z_max, 0.05, 0.05));
            assert!(
                outcome.is_ok(),
                "z_max = {z_max} mm must not panic the layer-count derivation",
            );
        }
    }

    #[test]
    fn non_finite_heights_are_rejected() {
        for z_max in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            let err = total_layers_for_height(z_max, 0.05, 0.05)
                .expect_err("a non-finite z_max has no layer count");
            assert!(
                matches!(err, UnschedulableHeight::NonFiniteHeight { z_max_mm }
                    if z_max_mm.is_nan() == z_max.is_nan()),
                "z_max = {z_max}: got {err:?}",
            );
        }
    }

    #[test]
    fn heights_beyond_u32_layers_are_rejected() {
        // (2.6e39 - 0.05) / 0.05 is far past u32::MAX. Formerly this
        // saturated the cast and overflowed the `1 +`.
        assert_eq!(
            total_layers_for_height(2.6e39, 0.05, 0.05),
            Err(UnschedulableHeight::LayerCountOverflow { z_max_mm: 2.6e39 }),
        );
    }

    #[test]
    fn layer_count_just_below_the_u32_ceiling_stays_ok() {
        // h = h0 = 1.0 keeps extra == z_max - 1 exactly (integers of
        // this size are exact in f64): 4_294_966_000 layers fit,
        // 5e9 layers do not.
        assert_eq!(
            total_layers_for_height(4_294_966_000.0, 1.0, 1.0),
            Ok(4_294_966_000),
        );
        assert_eq!(
            total_layers_for_height(5.0e9, 1.0, 1.0),
            Err(UnschedulableHeight::LayerCountOverflow { z_max_mm: 5.0e9 }),
        );
    }

    #[test]
    fn display_names_the_offending_height() {
        let non_finite = UnschedulableHeight::NonFiniteHeight { z_max_mm: f64::NAN };
        assert_eq!(
            non_finite.to_string(),
            "the scene height z = NaN mm is not a finite value",
        );
        let overflow = UnschedulableHeight::LayerCountOverflow {
            z_max_mm: 5_000_000_000.0,
        };
        assert_eq!(
            overflow.to_string(),
            "covering the scene height z = 5000000000 mm needs more layers than a schedule can hold",
        );
    }
}
