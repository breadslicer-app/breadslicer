/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Projection from a resolved print configuration to a slice
//! profile.

use std::fmt;
use std::num::NonZeroU32;

use sc_print_primitives::PositiveFiniteMm;
use sc_print_profiles::{PrinterHardware, ResolvedPrintConfig};
use sc_sl1cer::plane_intersect::on_plane_band_mm;
use sc_sl1cer::profile::{AaLevel, LayerSchedule, PackingMode, SliceProfile, UniformSchedule};

/// Why a scene height cannot be covered by a valid layer count.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UnschedulableHeight {
    /// `z_max` is NaN or infinite, so no layer count is defined for
    /// it.
    NonFiniteHeight {
        /// The offending `z_max` value.
        z_max_mm: f64,
    },
    /// Covering `z_max` needs more layers than a `u32` layer count
    /// can represent.
    LayerCountOverflow {
        /// The offending `z_max` value.
        z_max_mm: f64,
    },
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
    total_layers: NonZeroU32,
) -> SliceProfile {
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
    total_layers: NonZeroU32,
) -> SliceProfile {
    let layer_schedule = LayerSchedule::Uniform(UniformSchedule::new(
        layer_height_mm,
        first_layer_height_mm,
        total_layers,
    ));

    SliceProfile::new(
        hardware.build_volume(),
        hardware.pixel_resolution(),
        layer_schedule,
        hardware.mirror(),
        aa_level,
        packing_mode,
    )
}

/// The number of layers that print something of a model up to `z_max`
/// millimetres: those cut below its top. Slab 0 spans
/// `[0, first_layer_height]`; slab `k >= 1` spans
/// `[first_layer_height + (k-1)*layer_height, first_layer_height + k*layer_height]`,
/// and each is cut at its centre ([`LayerSchedule::iter_z`]).
///
/// A layer counts when the top lies above its cutting plane by more than
/// the slicer's on-plane band ([`on_plane_band_mm`]): a top in the lower
/// half of the last covering slab, or a top face exactly at its cutting
/// height, cuts nothing there, so that layer is left out (#226). Before,
/// the count covered the top with whole slabs, and about 40 % of heights
/// that are a whole number of layers ended in one empty layer from
/// rounding. Always at least 1.
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
) -> Result<NonZeroU32, UnschedulableHeight> {
    if !z_max.is_finite() {
        return Err(UnschedulableHeight::NonFiniteHeight { z_max_mm: z_max });
    }
    if z_max <= first_layer_height {
        return Ok(NonZeroU32::MIN);
    }
    let beyond_first = z_max - first_layer_height;
    let extra = (beyond_first / layer_height).ceil();
    // `extra < u32::MAX` keeps `1 + extra` representable; the negated
    // form is deliberate so a NaN `extra` also lands in the error arm.
    if !(extra.is_finite() && extra < f64::from(u32::MAX)) {
        return Err(UnschedulableHeight::LayerCountOverflow { z_max_mm: z_max });
    }
    // In range by the guard above; stays total anyway rather than
    // unwrapping.
    let covering = NonZeroU32::MIN
        .checked_add(extra as u32)
        .ok_or(UnschedulableHeight::LayerCountOverflow { z_max_mm: z_max })?;
    // The covering count's last layer is cut at least half a layer below
    // the top of its slab, the one before it a whole layer lower: only the
    // last can miss the model.
    let last = covering.get() - 1;
    let cut = cut_height(last, layer_height, first_layer_height);
    if z_max - cut > on_plane_band_mm(cut) {
        Ok(covering)
    } else {
        Ok(NonZeroU32::new(last).unwrap_or(NonZeroU32::MIN))
    }
}

/// The height layer `k` is cut at, computed as [`LayerSchedule::iter_z`]
/// does so it is the very plane the slicer uses.
fn cut_height(k: u32, layer_height: f64, first_layer_height: f64) -> f64 {
    if k == 0 {
        first_layer_height * 0.5
    } else {
        (f64::from(k) - 0.5).mul_add(layer_height, first_layer_height)
    }
}

#[cfg(test)]
mod total_layers_tests {
    use std::num::NonZeroU32;

    use sc_sl1cer::plane_intersect::on_plane_band_mm;

    use super::{UnschedulableHeight, total_layers_for_height};

    fn n(count: u32) -> NonZeroU32 {
        NonZeroU32::new(count).expect("test counts are non-zero")
    }

    #[test]
    fn returns_one_when_model_fits_in_the_first_slab() {
        assert_eq!(total_layers_for_height(0.04, 0.1, 0.05), Ok(n(1)));
        assert_eq!(total_layers_for_height(0.05, 0.1, 0.05), Ok(n(1)));
    }

    #[test]
    fn adds_extra_layers_to_cover_above_first_slab() {
        // first slab covers [0, 0.05]; each extra slab is 0.1 thick.
        // z_max = 0.55: needs (0.55 - 0.05) / 0.1 = 5 extra slabs, the
        // last cut at 0.5.
        assert_eq!(total_layers_for_height(0.55, 0.1, 0.05), Ok(n(6)));
        // z_max = 0.5: 5 extra slabs cover it, but the last is cut at
        // 0.5, the top itself, and prints nothing.
        assert_eq!(total_layers_for_height(0.5, 0.1, 0.05), Ok(n(5)));
    }

    /// The layer heights are cut at: the centres of the slabs.
    fn cuts(count: NonZeroU32, h: f64, h0: f64) -> Vec<f64> {
        sc_sl1cer::profile::LayerSchedule::uniform(h, h0, count.get())
            .unwrap()
            .iter_z()
            .collect()
    }

    #[test]
    fn every_layer_is_cut_below_the_top_and_none_below_it_is_left_out() {
        // 4000 heights at 0.05 mm steps, the whole number of layers the
        // research simulated (1602 ended in an empty layer), plus heights
        // in between, with and without a thicker first layer.
        for (h, h0) in [(0.05, 0.05), (0.05, 0.1), (0.03, 0.05)] {
            for step in 1..=4000 {
                for z_max in [
                    f64::from(step) * 0.05,
                    f64::from(step) * 0.05 + 0.021,
                    f64::from(step) * 0.05 - 0.007,
                ] {
                    // The top is an f32 vertex height.
                    let z_max = f64::from(z_max as f32);
                    let count = total_layers_for_height(z_max, h, h0).unwrap();
                    let planes = cuts(count, h, h0);
                    let last = *planes.last().unwrap();
                    assert!(
                        count.get() == 1 || z_max - last > on_plane_band_mm(last),
                        "z_max {z_max}, h {h}, h0 {h0}: the last layer, cut at {last}, misses the top",
                    );
                    let next = cuts(count.saturating_add(1), h, h0)[count.get() as usize];
                    assert!(
                        z_max - next <= on_plane_band_mm(next),
                        "z_max {z_max}, h {h}, h0 {h0}: the layer cut at {next} is left out",
                    );
                }
            }
        }
    }

    #[test]
    fn a_top_exactly_at_a_cutting_height_adds_no_layer() {
        // 100.275 mm is the centre of layer 2005 at 50 um; as an f32 it
        // sits 1.5e-6 mm above it, on the plane for the slicer.
        let z_max = f64::from(100.275_f32);
        assert_eq!(total_layers_for_height(z_max, 0.05, 0.05), Ok(n(2005)));
        assert_eq!(total_layers_for_height(0.275, 0.05, 0.05), Ok(n(5)));
    }

    #[test]
    fn the_siraya_test_model_ends_on_a_printed_layer() {
        // 20.218 mm at 50 um was 405 layers, the last cut at 20.225 mm,
        // above the model.
        let z_max = f64::from(20.218_f32);
        assert_eq!(total_layers_for_height(z_max, 0.05, 0.05), Ok(n(404)));
    }

    #[test]
    fn uniform_schedule_matches_classical_count() {
        // h0 = h = 0.2, z_max = 2.0. classical: ceil(2.0 / 0.2) = 10.
        assert_eq!(total_layers_for_height(2.0, 0.2, 0.2), Ok(n(10)));
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
        // this size are exact in f64): 4_294_966_000 layers cover the
        // top and fit, 5e9 layers do not. The last is cut half a layer
        // below the top, inside the on-plane band (one f32 step, 512 mm
        // up there), so it is left out.
        assert_eq!(
            total_layers_for_height(4_294_966_000.0, 1.0, 1.0),
            Ok(n(4_294_965_999)),
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
