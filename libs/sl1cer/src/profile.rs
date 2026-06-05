/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Typed slicer profile: build volume, pixel resolution, layer
//! schedule, mirror flags, AA level, packing mode.
//!
//! Each component owns its own checked constructor. Once a component
//! exists, its invariants hold by type, so [`SliceProfile::new`] just
//! assembles them and never fails.

use std::fmt;
use std::str::FromStr;

use sc_mesh_core::primitives::CoordAxis;
pub use sc_print_primitives::{NegativeLengthError, ParseLengthError, PositiveFiniteMm};

/// Why a [`SliceProfile`] component constructor rejected its input.
///
/// `CoordAxis` is reused from `sc-mesh-core` for the per-axis
/// variants: build volume uses X / Y / Z; pixel resolution only emits
/// X or Y in practice.
#[derive(Debug)]
pub enum SliceProfileError {
    /// A build volume axis was zero, negative, or non-finite.
    BuildVolumeAxisInvalid { axis: CoordAxis, value: f64 },
    /// A pixel resolution axis was zero.
    PixelResolutionZero { axis: CoordAxis },
    /// `layer_height_mm` was zero, negative, or non-finite.
    LayerHeightInvalid { value: f64 },
    /// `first_layer_height_mm` was zero, negative, or non-finite.
    FirstLayerHeightInvalid { value: f64 },
    /// `total_layers` was zero.
    TotalLayersZero,
}

impl fmt::Display for SliceProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BuildVolumeAxisInvalid { axis, value } => write!(
                f,
                "build volume {axis} axis must be a finite positive length in millimetres, got {value}",
            ),
            Self::PixelResolutionZero { axis } => {
                write!(f, "pixel resolution {axis} axis must be non-zero")
            }
            Self::LayerHeightInvalid { value } => write!(
                f,
                "layer_height_mm must be a finite positive length in millimetres, got {value}",
            ),
            Self::FirstLayerHeightInvalid { value } => write!(
                f,
                "first_layer_height_mm must be a finite positive length in millimetres, got {value}",
            ),
            Self::TotalLayersZero => write!(f, "total_layers must be non-zero"),
        }
    }
}

impl std::error::Error for SliceProfileError {}

/// Build volume in millimetres, validated finite and strictly positive
/// per axis on construction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BuildVolume {
    x_mm: f64,
    y_mm: f64,
    z_mm: f64,
}

impl BuildVolume {
    pub fn new(x_mm: f64, y_mm: f64, z_mm: f64) -> Result<Self, SliceProfileError> {
        Self::check_axis(CoordAxis::X, x_mm)?;
        Self::check_axis(CoordAxis::Y, y_mm)?;
        Self::check_axis(CoordAxis::Z, z_mm)?;
        Ok(Self { x_mm, y_mm, z_mm })
    }

    fn check_axis(axis: CoordAxis, value: f64) -> Result<(), SliceProfileError> {
        if !value.is_finite() || value <= 0.0 {
            Err(SliceProfileError::BuildVolumeAxisInvalid { axis, value })
        } else {
            Ok(())
        }
    }

    #[inline]
    pub fn x_mm(&self) -> f64 {
        self.x_mm
    }

    #[inline]
    pub fn y_mm(&self) -> f64 {
        self.y_mm
    }

    #[inline]
    pub fn z_mm(&self) -> f64 {
        self.z_mm
    }

    /// Minimum corner of the build envelope in printer coordinates:
    /// `[-x_mm/2, -y_mm/2, 0]`. XY is centred on the origin; the
    /// floor of the build plate sits at `z = 0`. Matches the GUI's
    /// [`BuildVolume::envelope_min_mm`] in `apps/slicer-gui`.
    #[inline]
    pub fn envelope_min_mm(&self) -> [f64; 3] {
        [-0.5 * self.x_mm, -0.5 * self.y_mm, 0.0]
    }

    /// Maximum corner of the build envelope in printer coordinates:
    /// `[+x_mm/2, +y_mm/2, z_mm]`. Mirror of [`Self::envelope_min_mm`].
    #[inline]
    pub fn envelope_max_mm(&self) -> [f64; 3] {
        [0.5 * self.x_mm, 0.5 * self.y_mm, self.z_mm]
    }
}

/// Pixel resolution validated non-zero per axis on construction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelResolution {
    x: u32,
    y: u32,
}

impl PixelResolution {
    pub fn new(x: u32, y: u32) -> Result<Self, SliceProfileError> {
        if x == 0 {
            return Err(SliceProfileError::PixelResolutionZero { axis: CoordAxis::X });
        }
        if y == 0 {
            return Err(SliceProfileError::PixelResolutionZero { axis: CoordAxis::Y });
        }
        Ok(Self { x, y })
    }

    #[inline]
    pub fn x(&self) -> u32 {
        self.x
    }

    #[inline]
    pub fn y(&self) -> u32 {
        self.y
    }
}

/// Per-axis mirror flags for raster output orientation. No invariants
/// to validate; both flags are free.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mirror {
    pub x: bool,
    pub y: bool,
}

/// Per-pixel anti-aliasing factor for the raster tier. `Off` is the
/// binary scanline path; the others apply N-step Y sub-sampling with
/// analytic X coverage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AaLevel {
    Off,
    X2,
    X4,
    X8,
}

/// Returned when [`AaLevel::from_str`] receives an unknown name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownAaLevel {
    pub got: String,
}

impl fmt::Display for UnknownAaLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown AA level \"{}\"; expected one of: 0, off, 2, x2, 4, x4, 8, x8",
            self.got,
        )
    }
}

impl std::error::Error for UnknownAaLevel {}

impl FromStr for AaLevel {
    type Err = UnknownAaLevel;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "0" | "off" => Ok(Self::Off),
            "2" | "x2" => Ok(Self::X2),
            "4" | "x4" => Ok(Self::X4),
            "8" | "x8" => Ok(Self::X8),
            _ => Err(UnknownAaLevel { got: s.to_owned() }),
        }
    }
}

/// How the layer raster's RLE runs are packed. Only `None` (no
/// packing) is currently supported; later printer-format-specific
/// modes will live here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PackingMode {
    #[default]
    None,
}

/// How the layer-by-layer slicing planes are positioned along Z.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LayerSchedule {
    /// Uniform layer height with an optionally distinct first-layer
    /// height. Slab layout in absolute Z above the build plate:
    ///
    /// - Slab 0 covers `[0, h0]`.
    /// - Slab `k` for `k in 1..n` covers
    ///   `[h0 + (k - 1) * h, h0 + k * h]`.
    ///
    /// [`LayerSchedule::iter_z`] yields the centred slicing plane of
    /// each slab: `h0 / 2` for slab 0, and `h0 + (k - 0.5) * h` for
    /// `k >= 1`. When `h0 == h` this degenerates to the classical
    /// `(k + 0.5) * h` formula. Total print height is
    /// `h0 + (n - 1) * h`.
    Uniform {
        layer_height_mm: f64,
        first_layer_height_mm: f64,
        total_layers: u32,
    },
}

impl LayerSchedule {
    pub fn uniform(
        layer_height_mm: f64,
        first_layer_height_mm: f64,
        total_layers: u32,
    ) -> Result<Self, SliceProfileError> {
        if !layer_height_mm.is_finite() || layer_height_mm <= 0.0 {
            return Err(SliceProfileError::LayerHeightInvalid {
                value: layer_height_mm,
            });
        }
        if !first_layer_height_mm.is_finite() || first_layer_height_mm <= 0.0 {
            return Err(SliceProfileError::FirstLayerHeightInvalid {
                value: first_layer_height_mm,
            });
        }
        if total_layers == 0 {
            return Err(SliceProfileError::TotalLayersZero);
        }
        Ok(Self::Uniform {
            layer_height_mm,
            first_layer_height_mm,
            total_layers,
        })
    }

    /// Iterator of absolute Z values (millimetres above the build
    /// plate) where each layer slab should be sampled. Always strictly
    /// monotonically increasing under the constructor's invariants.
    pub fn iter_z(&self) -> impl Iterator<Item = f64> {
        match *self {
            Self::Uniform {
                layer_height_mm: h,
                first_layer_height_mm: h0,
                total_layers: n,
            } => (0..n).map(move |k| {
                if k == 0 {
                    h0 * 0.5
                } else {
                    (f64::from(k) - 0.5).mul_add(h, h0)
                }
            }),
        }
    }
}

/// Slicer-side print profile: only the fields the slicer itself reads.
/// Exposure, lift, fade and printer-id fields live on the eventual
/// SL1 / CTB writer profiles in their own crate.
#[derive(Clone, Debug, PartialEq)]
pub struct SliceProfile {
    build_volume: BuildVolume,
    pixel_resolution: PixelResolution,
    layer_schedule: LayerSchedule,
    mirror: Mirror,
    aa_level: AaLevel,
    packing_mode: PackingMode,
}

impl SliceProfile {
    /// Assemble a [`SliceProfile`] from its already-validated
    /// components. Infallible: any rejection happens at the component
    /// constructor.
    pub fn new(
        build_volume: BuildVolume,
        pixel_resolution: PixelResolution,
        layer_schedule: LayerSchedule,
        mirror: Mirror,
        aa_level: AaLevel,
        packing_mode: PackingMode,
    ) -> Self {
        Self {
            build_volume,
            pixel_resolution,
            layer_schedule,
            mirror,
            aa_level,
            packing_mode,
        }
    }

    #[inline]
    pub fn build_volume(&self) -> &BuildVolume {
        &self.build_volume
    }

    #[inline]
    pub fn pixel_resolution(&self) -> &PixelResolution {
        &self.pixel_resolution
    }

    #[inline]
    pub fn layer_schedule(&self) -> &LayerSchedule {
        &self.layer_schedule
    }

    #[inline]
    pub fn mirror(&self) -> Mirror {
        self.mirror
    }

    #[inline]
    pub fn aa_level(&self) -> AaLevel {
        self.aa_level
    }

    #[inline]
    pub fn packing_mode(&self) -> PackingMode {
        self.packing_mode
    }
}

#[cfg(test)]
mod profile_tests {
    use super::*;

    #[test]
    fn build_volume_new_accepts_positive_finite() {
        let bv = BuildVolume::new(60.0, 60.0, 100.0).unwrap();
        assert_eq!(bv.x_mm(), 60.0);
        assert_eq!(bv.y_mm(), 60.0);
        assert_eq!(bv.z_mm(), 100.0);
    }

    #[test]
    fn build_volume_new_rejects_zero_x_axis() {
        let err = BuildVolume::new(0.0, 60.0, 100.0).unwrap_err();
        assert!(matches!(
            err,
            SliceProfileError::BuildVolumeAxisInvalid {
                axis: CoordAxis::X,
                value
            } if value == 0.0
        ));
    }

    #[test]
    fn build_volume_new_rejects_negative_y_axis() {
        let err = BuildVolume::new(60.0, -1.0, 100.0).unwrap_err();
        assert!(matches!(
            err,
            SliceProfileError::BuildVolumeAxisInvalid {
                axis: CoordAxis::Y,
                ..
            }
        ));
    }

    #[test]
    fn build_volume_new_rejects_nan_z_axis() {
        let err = BuildVolume::new(60.0, 60.0, f64::NAN).unwrap_err();
        assert!(matches!(
            err,
            SliceProfileError::BuildVolumeAxisInvalid {
                axis: CoordAxis::Z,
                ..
            }
        ));
    }

    #[test]
    fn build_volume_new_rejects_infinite_x_axis() {
        let err = BuildVolume::new(f64::INFINITY, 60.0, 100.0).unwrap_err();
        assert!(matches!(
            err,
            SliceProfileError::BuildVolumeAxisInvalid {
                axis: CoordAxis::X,
                ..
            }
        ));
    }

    #[test]
    fn build_volume_envelope_centres_xy_on_origin_and_floors_z_at_zero() {
        let bv = BuildVolume::new(192.0, 120.0, 200.0).unwrap();
        assert_eq!(bv.envelope_min_mm(), [-96.0, -60.0, 0.0]);
        assert_eq!(bv.envelope_max_mm(), [96.0, 60.0, 200.0]);
    }

    #[test]
    fn build_volume_envelope_extent_matches_per_axis_dimensions() {
        let bv = BuildVolume::new(60.0, 40.0, 100.0).unwrap();
        let min = bv.envelope_min_mm();
        let max = bv.envelope_max_mm();
        assert!((max[0] - min[0] - bv.x_mm()).abs() < 1e-12);
        assert!((max[1] - min[1] - bv.y_mm()).abs() < 1e-12);
        assert!((max[2] - min[2] - bv.z_mm()).abs() < 1e-12);
    }

    #[test]
    fn pixel_resolution_new_accepts_positive_dimensions() {
        let res = PixelResolution::new(1920, 1080).unwrap();
        assert_eq!(res.x(), 1920);
        assert_eq!(res.y(), 1080);
    }

    #[test]
    fn pixel_resolution_new_rejects_zero_x() {
        let err = PixelResolution::new(0, 1080).unwrap_err();
        assert!(matches!(
            err,
            SliceProfileError::PixelResolutionZero { axis: CoordAxis::X }
        ));
    }

    #[test]
    fn pixel_resolution_new_rejects_zero_y() {
        let err = PixelResolution::new(1920, 0).unwrap_err();
        assert!(matches!(
            err,
            SliceProfileError::PixelResolutionZero { axis: CoordAxis::Y }
        ));
    }

    #[test]
    fn layer_schedule_uniform_rejects_zero_layer_height() {
        let err = LayerSchedule::uniform(0.0, 0.05, 10).unwrap_err();
        assert!(matches!(err, SliceProfileError::LayerHeightInvalid { .. }));
    }

    #[test]
    fn layer_schedule_uniform_rejects_negative_first_layer_height() {
        let err = LayerSchedule::uniform(0.05, -0.01, 10).unwrap_err();
        assert!(matches!(
            err,
            SliceProfileError::FirstLayerHeightInvalid { .. }
        ));
    }

    #[test]
    fn layer_schedule_uniform_rejects_nan_layer_height() {
        let err = LayerSchedule::uniform(f64::NAN, 0.05, 10).unwrap_err();
        assert!(matches!(err, SliceProfileError::LayerHeightInvalid { .. }));
    }

    #[test]
    fn layer_schedule_uniform_rejects_zero_total_layers() {
        let err = LayerSchedule::uniform(0.05, 0.05, 0).unwrap_err();
        assert!(matches!(err, SliceProfileError::TotalLayersZero));
    }

    #[test]
    fn layer_schedule_iter_z_three_layers_with_thicker_first() {
        let schedule = LayerSchedule::uniform(0.1, 0.05, 3).unwrap();
        let zs: Vec<f64> = schedule.iter_z().collect();
        assert_eq!(zs, vec![0.025, 0.1, 0.2]);
    }

    #[test]
    fn layer_schedule_iter_z_single_layer_returns_half_first_layer_height() {
        let schedule = LayerSchedule::uniform(0.1, 0.06, 1).unwrap();
        let zs: Vec<f64> = schedule.iter_z().collect();
        assert_eq!(zs, vec![0.03]);
    }

    #[test]
    fn layer_schedule_iter_z_degenerates_to_classical_when_h0_equals_h() {
        // Mathematically `(k - 0.5) * h + h0` equals `(k + 0.5) * h`
        // when h0 == h, but the two expressions take different
        // rounding paths in f64, so compare with a small absolute
        // tolerance instead of bit-equality.
        let h = 0.1;
        let n = 4;
        let schedule = LayerSchedule::uniform(h, h, n).unwrap();
        let zs: Vec<f64> = schedule.iter_z().collect();
        let expected: Vec<f64> = (0..n).map(|k| (f64::from(k) + 0.5) * h).collect();
        for (got, want) in zs.iter().zip(expected.iter()) {
            assert!(
                (got - want).abs() < 1e-12,
                "iter_z produced {got}, classical formula produced {want}",
            );
        }
    }

    #[test]
    fn layer_schedule_iter_z_is_strictly_monotone() {
        let schedule = LayerSchedule::uniform(0.1, 0.05, 5).unwrap();
        let zs: Vec<f64> = schedule.iter_z().collect();
        for w in zs.windows(2) {
            assert!(w[1] > w[0], "expected strictly increasing Z, got {zs:?}");
        }
    }

    #[test]
    fn slice_profile_new_carries_components() {
        let bv = BuildVolume::new(60.0, 60.0, 100.0).unwrap();
        let res = PixelResolution::new(1920, 1080).unwrap();
        let sched = LayerSchedule::uniform(0.05, 0.05, 10).unwrap();
        let profile = SliceProfile::new(
            bv,
            res,
            sched,
            Mirror::default(),
            AaLevel::Off,
            PackingMode::None,
        );
        assert_eq!(profile.build_volume().x_mm(), 60.0);
        assert_eq!(profile.pixel_resolution().x(), 1920);
        assert_eq!(profile.layer_schedule(), &sched);
        assert_eq!(profile.mirror(), Mirror::default());
        assert_eq!(profile.aa_level(), AaLevel::Off);
        assert_eq!(profile.packing_mode(), PackingMode::None);
    }

    #[test]
    fn slice_profile_error_display_renders_axis() {
        let err = BuildVolume::new(0.0, 60.0, 100.0).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("X axis"), "msg: {msg}");
        assert!(msg.contains("got 0"), "msg: {msg}");
    }

    #[test]
    fn aa_level_from_str_parses_numeric_shorthand() {
        assert_eq!(AaLevel::from_str("0"), Ok(AaLevel::Off));
        assert_eq!(AaLevel::from_str("2"), Ok(AaLevel::X2));
        assert_eq!(AaLevel::from_str("4"), Ok(AaLevel::X4));
        assert_eq!(AaLevel::from_str("8"), Ok(AaLevel::X8));
    }

    #[test]
    fn aa_level_from_str_parses_canonical_names() {
        assert_eq!(AaLevel::from_str("off"), Ok(AaLevel::Off));
        assert_eq!(AaLevel::from_str("x2"), Ok(AaLevel::X2));
        assert_eq!(AaLevel::from_str("x4"), Ok(AaLevel::X4));
        assert_eq!(AaLevel::from_str("x8"), Ok(AaLevel::X8));
    }

    #[test]
    fn aa_level_from_str_is_case_insensitive() {
        assert_eq!(AaLevel::from_str("Off"), Ok(AaLevel::Off));
        assert_eq!(AaLevel::from_str("X4"), Ok(AaLevel::X4));
    }

    #[test]
    fn aa_level_from_str_rejects_unknown_name() {
        let err = AaLevel::from_str("3").unwrap_err();
        assert_eq!(err.got, "3");
        let msg = format!("{err}");
        assert!(msg.contains("3"), "msg: {msg}");
        assert!(msg.contains("expected"), "msg: {msg}");
    }
}
