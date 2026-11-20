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
use std::num::NonZeroU32;
use std::sync::Arc;

pub use sc_print_primitives::{
    AaLevel, AxisOutOfRange, BuildVolume, LengthOutOfRange, Mirror, ParseLengthError,
    PixelResolution, PositiveFiniteMm, UnknownAaLevel, ZeroPixelExtentError,
};

/// Why [`LayerSchedule::uniform`] rejected its inputs. The two
/// length variants carry the underlying [`LengthOutOfRange`] from the
/// per-field [`PositiveFiniteMm`] check; the zero-count variant is a unit.
#[derive(Debug)]
pub enum InvalidLayerSchedule {
    /// `layer_height_mm` failed the strictly-positive-finite invariant.
    LayerHeightOutOfRange(LengthOutOfRange),
    /// `first_layer_height_mm` failed the strictly-positive-finite invariant.
    FirstLayerHeightOutOfRange(LengthOutOfRange),
    /// `total_layers` was zero.
    ZeroTotalLayers,
}

impl fmt::Display for InvalidLayerSchedule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LayerHeightOutOfRange(source) => write!(
                f,
                "layer_height_mm must be a finite positive length in millimetres, got {}",
                source.value,
            ),
            Self::FirstLayerHeightOutOfRange(source) => write!(
                f,
                "first_layer_height_mm must be a finite positive length in millimetres, got {}",
                source.value,
            ),
            Self::ZeroTotalLayers => write!(f, "total_layers must be non-zero"),
        }
    }
}

impl std::error::Error for InvalidLayerSchedule {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::LayerHeightOutOfRange(source) | Self::FirstLayerHeightOutOfRange(source) => {
                Some(source)
            }
            Self::ZeroTotalLayers => None,
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

/// Validated payload of [`LayerSchedule::Uniform`]: both heights are
/// strictly positive finite millimetres and the layer count is
/// non-zero, held behind private fields so no raw-float state can
/// bypass them. Every plane sequence derived from such a payload is
/// strictly monotone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UniformSchedule {
    layer_height: PositiveFiniteMm,
    first_layer_height: PositiveFiniteMm,
    total_layers: NonZeroU32,
}

impl UniformSchedule {
    /// Assemble from already-validated parts. Infallible: the
    /// invariants ride on the parameter types.
    pub fn new(
        layer_height: PositiveFiniteMm,
        first_layer_height: PositiveFiniteMm,
        total_layers: NonZeroU32,
    ) -> Self {
        Self {
            layer_height,
            first_layer_height,
            total_layers,
        }
    }

    /// The height of every slab above the first, in millimetres.
    #[inline]
    pub fn layer_height(&self) -> PositiveFiniteMm {
        self.layer_height
    }

    /// The height of the first slab, in millimetres.
    #[inline]
    pub fn first_layer_height(&self) -> PositiveFiniteMm {
        self.first_layer_height
    }

    /// The number of slabs.
    #[inline]
    pub fn total_layers(&self) -> NonZeroU32 {
        self.total_layers
    }
}

/// Validated payload of [`LayerSchedule::Variable`]: each slab's top, in
/// millimetres above the build plate, strictly increasing and at least one,
/// held behind a private field so no raw-float state can bypass it. Slab 0
/// starts at the plate; slab `k` starts at the top of slab `k - 1` (#60).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VariableSchedule {
    tops: Arc<[PositiveFiniteMm]>,
    nominal: PositiveFiniteMm,
}

/// Why [`VariableSchedule::new`] rejected its slab tops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvalidSlabTops {
    /// There are none.
    Empty,
    /// A top does not lie above the one before it.
    NotIncreasing {
        /// The position of the offending top.
        index: usize,
    },
}

impl fmt::Display for InvalidSlabTops {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("a variable schedule needs at least one slab"),
            Self::NotIncreasing { index } => {
                write!(f, "slab {index} does not top out above the slab below it")
            }
        }
    }
}

impl std::error::Error for InvalidSlabTops {}

impl VariableSchedule {
    /// Takes each slab's top, which must rise from one slab to the next, and
    /// the print's own layer height, which the slabs vary around and which
    /// the bottom layers keep.
    pub fn new(
        tops: Vec<PositiveFiniteMm>,
        nominal: PositiveFiniteMm,
    ) -> Result<Self, InvalidSlabTops> {
        if tops.is_empty() {
            return Err(InvalidSlabTops::Empty);
        }
        if let Some(index) = tops
            .windows(2)
            .position(|pair| pair[1].value() <= pair[0].value())
        {
            return Err(InvalidSlabTops::NotIncreasing { index: index + 1 });
        }
        Ok(Self {
            tops: tops.into(),
            nominal,
        })
    }

    /// Each slab's top, from the plate up.
    #[inline]
    pub fn tops(&self) -> &[PositiveFiniteMm] {
        &self.tops
    }

    /// The print's own layer height.
    #[inline]
    pub fn nominal_layer_height(&self) -> PositiveFiniteMm {
        self.nominal
    }

    /// The first `layers` slabs, or all of them when there are fewer.
    pub fn truncated(&self, layers: NonZeroU32) -> Self {
        let kept = usize::try_from(layers.get()).unwrap_or(usize::MAX);
        if kept >= self.tops.len() {
            return self.clone();
        }
        Self {
            tops: self.tops[..kept].into(),
            nominal: self.nominal,
        }
    }
}

/// How the layer-by-layer slicing planes are positioned along Z.
#[derive(Clone, Debug, PartialEq, Eq)]
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
    Uniform(UniformSchedule),
    /// A height of its own for each slab (#60): slab `k` covers the span
    /// between the tops of slabs `k - 1` and `k`, the plate below slab 0,
    /// and is cut at the middle of that span.
    Variable(VariableSchedule),
}

impl LayerSchedule {
    /// Validating constructor from raw values. Callers that already
    /// hold typed parts can build the variant directly from a
    /// [`UniformSchedule`] instead.
    pub fn uniform(
        layer_height_mm: f64,
        first_layer_height_mm: f64,
        total_layers: u32,
    ) -> Result<Self, InvalidLayerSchedule> {
        let layer_height = PositiveFiniteMm::new(layer_height_mm)
            .map_err(InvalidLayerSchedule::LayerHeightOutOfRange)?;
        let first_layer_height = PositiveFiniteMm::new(first_layer_height_mm)
            .map_err(InvalidLayerSchedule::FirstLayerHeightOutOfRange)?;
        let total_layers =
            NonZeroU32::new(total_layers).ok_or(InvalidLayerSchedule::ZeroTotalLayers)?;
        Ok(Self::Uniform(UniformSchedule::new(
            layer_height,
            first_layer_height,
            total_layers,
        )))
    }

    /// The number of layers (sampling planes) the schedule yields, the
    /// length of [`iter_z`](Self::iter_z), as a plain count for layer
    /// arithmetic. The typed non-zero value sits on the variant
    /// payload.
    pub fn total_layers(&self) -> u32 {
        match self {
            Self::Uniform(uniform) => uniform.total_layers().get(),
            Self::Variable(variable) => u32::try_from(variable.tops().len()).unwrap_or(u32::MAX),
        }
    }

    /// Whether every slab above the first is as high as the others.
    pub fn is_uniform(&self) -> bool {
        matches!(self, Self::Uniform(_))
    }

    /// The print's own layer height: every slab's above the first in a
    /// uniform schedule, the one a variable schedule varies around.
    pub fn nominal_layer_height(&self) -> PositiveFiniteMm {
        match self {
            Self::Uniform(uniform) => uniform.layer_height(),
            Self::Variable(variable) => variable.nominal_layer_height(),
        }
    }

    /// The first slab's height.
    pub fn first_layer_height(&self) -> PositiveFiniteMm {
        match self {
            Self::Uniform(uniform) => uniform.first_layer_height(),
            Self::Variable(variable) => variable.tops()[0],
        }
    }

    /// The first `layers` slabs of the schedule, or all of them when it has
    /// fewer.
    pub fn truncated(&self, layers: NonZeroU32) -> Self {
        match self {
            Self::Uniform(uniform) => Self::Uniform(UniformSchedule::new(
                uniform.layer_height(),
                uniform.first_layer_height(),
                layers.min(uniform.total_layers()),
            )),
            Self::Variable(variable) => Self::Variable(variable.truncated(layers)),
        }
    }

    /// Each slab's height, from the plate up.
    pub fn iter_slab_heights(&self) -> impl Iterator<Item = f64> + '_ {
        let mut below = 0.0;
        self.iter_slab_tops().map(move |top| {
            let height = top - below;
            below = top;
            height
        })
    }

    /// Iterator of absolute Z values (millimetres above the build
    /// plate) where each layer slab should be sampled. Always strictly
    /// monotonically increasing: the payload's heights are strictly
    /// positive by type.
    pub fn iter_z(&self) -> impl Iterator<Item = f64> + '_ {
        let (uniform, variable) = self.parts();
        let uniform = uniform.into_iter().flat_map(|uniform| {
            let h = uniform.layer_height().value();
            let h0 = uniform.first_layer_height().value();
            (0..uniform.total_layers().get()).map(move |k| {
                if k == 0 {
                    h0 * 0.5
                } else {
                    (f64::from(k) - 0.5).mul_add(h, h0)
                }
            })
        });
        let variable = variable.into_iter().flat_map(|variable| {
            let mut below = 0.0;
            variable.tops().iter().map(move |top| {
                let middle = (below + top.value()) * 0.5;
                below = top.value();
                middle
            })
        });
        uniform.chain(variable)
    }

    /// The payload, one side or the other.
    fn parts(&self) -> (Option<UniformSchedule>, Option<&VariableSchedule>) {
        match self {
            Self::Uniform(uniform) => (Some(*uniform), None),
            Self::Variable(variable) => (None, Some(variable)),
        }
    }

    /// Iterator of each slab's TOP surface in absolute Z (millimetres
    /// above the build plate): `h0` for slab 0, `h0 + k * h` for slab
    /// `k >= 1`. Always strictly monotonically increasing, like
    /// [`iter_z`](Self::iter_z). Writers that record cured-layer
    /// extents (e.g. slice-file formats) want these values, not the
    /// centred sampling planes.
    pub fn iter_slab_tops(&self) -> impl Iterator<Item = f64> + '_ {
        let (uniform, variable) = self.parts();
        let uniform = uniform.into_iter().flat_map(|uniform| {
            let h = uniform.layer_height().value();
            let h0 = uniform.first_layer_height().value();
            (0..uniform.total_layers().get()).map(move |k| f64::from(k).mul_add(h, h0))
        });
        let variable = variable
            .into_iter()
            .flat_map(|variable| variable.tops().iter().map(|top| top.value()));
        uniform.chain(variable)
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
    fn layer_schedule_uniform_rejects_zero_layer_height() {
        let err = LayerSchedule::uniform(0.0, 0.05, 10).unwrap_err();
        assert!(matches!(
            err,
            InvalidLayerSchedule::LayerHeightOutOfRange(_)
        ));
    }

    #[test]
    fn layer_schedule_uniform_rejects_negative_first_layer_height() {
        let err = LayerSchedule::uniform(0.05, -0.01, 10).unwrap_err();
        assert!(matches!(
            err,
            InvalidLayerSchedule::FirstLayerHeightOutOfRange(_)
        ));
    }

    #[test]
    fn layer_schedule_uniform_rejects_nan_layer_height() {
        let err = LayerSchedule::uniform(f64::NAN, 0.05, 10).unwrap_err();
        assert!(matches!(
            err,
            InvalidLayerSchedule::LayerHeightOutOfRange(_)
        ));
    }

    #[test]
    fn layer_schedule_uniform_rejects_zero_total_layers() {
        let err = LayerSchedule::uniform(0.05, 0.05, 0).unwrap_err();
        assert!(matches!(err, InvalidLayerSchedule::ZeroTotalLayers));
    }

    #[test]
    fn a_variable_schedule_cuts_each_slab_at_its_middle() {
        let mm = |v| PositiveFiniteMm::new(v).unwrap();
        let schedule = LayerSchedule::Variable(
            VariableSchedule::new(vec![mm(0.05), mm(0.1), mm(0.2), mm(0.225)], mm(0.05)).unwrap(),
        );
        assert_eq!(schedule.nominal_layer_height(), mm(0.05));
        assert_eq!(schedule.first_layer_height(), mm(0.05));
        assert_eq!(schedule.total_layers(), 4);
        assert!(!schedule.is_uniform());
        let z: Vec<f64> = schedule.iter_z().collect();
        for (got, want) in z.iter().zip([0.025, 0.075, 0.15, 0.2125]) {
            assert!((got - want).abs() < 1e-12, "{z:?}");
        }
        let heights: Vec<f64> = schedule.iter_slab_heights().collect();
        for (got, want) in heights.iter().zip([0.05, 0.05, 0.1, 0.025]) {
            assert!((got - want).abs() < 1e-12, "{heights:?}");
        }
        let kept = schedule.truncated(NonZeroU32::new(2).unwrap());
        assert_eq!(kept.iter_slab_tops().collect::<Vec<_>>(), vec![0.05, 0.1]);
    }

    #[test]
    fn a_variable_schedule_refuses_tops_that_do_not_rise() {
        let mm = |v| PositiveFiniteMm::new(v).unwrap();
        assert_eq!(
            VariableSchedule::new(vec![], mm(0.05)),
            Err(InvalidSlabTops::Empty)
        );
        assert_eq!(
            VariableSchedule::new(vec![mm(0.1), mm(0.1)], mm(0.05)),
            Err(InvalidSlabTops::NotIncreasing { index: 1 })
        );
    }

    #[test]
    fn schedules_are_eq() {
        // Sound because the payload's validated types exclude NaN and
        // both zeroes; guards against the derive being dropped.
        fn assert_eq_bound<T: Eq>() {}
        assert_eq_bound::<UniformSchedule>();
        assert_eq_bound::<LayerSchedule>();
    }

    #[test]
    fn uniform_schedule_accessors_return_the_parts() {
        let layer_height = PositiveFiniteMm::new(0.05).unwrap();
        let first_layer_height = PositiveFiniteMm::new(0.15).unwrap();
        let total_layers = NonZeroU32::new(10).unwrap();
        let uniform = UniformSchedule::new(layer_height, first_layer_height, total_layers);
        assert_eq!(uniform.layer_height(), layer_height);
        assert_eq!(uniform.first_layer_height(), first_layer_height);
        assert_eq!(uniform.total_layers(), total_layers);
    }

    #[test]
    fn typed_construction_equals_the_validating_constructor() {
        let typed = LayerSchedule::Uniform(UniformSchedule::new(
            PositiveFiniteMm::new(0.05).unwrap(),
            PositiveFiniteMm::new(0.15).unwrap(),
            NonZeroU32::new(10).unwrap(),
        ));
        assert_eq!(typed, LayerSchedule::uniform(0.05, 0.15, 10).unwrap());
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
    fn layer_schedule_iter_slab_tops_three_layers_with_thicker_first() {
        let schedule = LayerSchedule::uniform(0.1, 0.3, 3).unwrap();
        let tops: Vec<f64> = schedule.iter_slab_tops().collect();
        assert_eq!(tops, vec![0.3, 0.4, 0.5]);
    }

    #[test]
    fn layer_schedule_iter_slab_tops_uniform_heights_land_on_multiples() {
        // `k * h + h0` and the mathematical `(k + 1) * h` take
        // different f64 rounding paths, so compare with a small
        // absolute tolerance instead of bit-equality.
        let schedule = LayerSchedule::uniform(0.05, 0.05, 4).unwrap();
        let tops: Vec<f64> = schedule.iter_slab_tops().collect();
        let expected = [0.05, 0.1, 0.15, 0.2];
        assert_eq!(tops.len(), expected.len());
        for (got, want) in tops.iter().zip(expected.iter()) {
            assert!(
                (got - want).abs() < 1e-12,
                "iter_slab_tops produced {got}, expected {want}",
            );
        }
    }

    #[test]
    fn layer_schedule_iter_slab_tops_single_layer_returns_first_height() {
        let schedule = LayerSchedule::uniform(0.1, 0.06, 1).unwrap();
        let tops: Vec<f64> = schedule.iter_slab_tops().collect();
        assert_eq!(tops, vec![0.06]);
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
            sched.clone(),
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
}
