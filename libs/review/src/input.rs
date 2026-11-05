/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! What the review reads: [`ReviewInput`].
//!
//! The input is one slicing run as the engine produced it: one contour
//! stack per model leaf, all cut on the same layer schedule; plus the
//! printer's screen (plate size, pixel resolution, mirror) and its maximum
//! height. Stacks are borrowed, so a frontend that holds the per-leaf stacks
//! of a run (`sc_print_glue::SlicedPlate::leaves`) can review them without
//! copying any contour.
//!
//! Supports made here never come in as contours (#432): what the review
//! knows of them is data, their tips ([`ReviewInput::with_tips`]), the layers
//! they stand on and the box they take ([`ReviewInput::with_support_extent`]).
//! Supports that came in the file, with no data, are a model like any other.

use std::fmt;
use std::ops::Range;

use sc_print_primitives::PositiveFiniteMm;
use sc_sl1cer::LeafSlices;
use sc_sl1cer::contour::ContourStack;
use sc_sl1cer::profile::{LayerSchedule, SliceProfile};
use sc_sl1cer::raster::pixel_map::PixelMap;

use crate::finding::PlatePoint;
use crate::tips::TipCone;

/// One leaf's contours.
#[derive(Clone, Copy, Debug)]
pub struct ReviewLeaf<'a> {
    /// The leaf's display name, when it has one.
    pub name: Option<&'a str>,
    /// The leaf's contours, one layer per scheduled layer.
    pub stack: &'a ContourStack,
}

impl<'a> ReviewLeaf<'a> {
    /// A leaf with these contours.
    #[inline]
    pub const fn new(name: Option<&'a str>, stack: &'a ContourStack) -> Self {
        Self { name, stack }
    }

    /// The leaf the engine sliced into `slices`, with its name.
    #[inline]
    pub fn from_slices(slices: &'a LeafSlices) -> Self {
        Self::new(slices.leaf_name(), slices.stack())
    }
}

/// The slab of resin one layer cures, in millimetres above the plate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayerSpan {
    /// Bottom of the slab (the top of the layer below; 0 for the first).
    pub bottom_mm: f64,
    /// Top of the slab.
    pub top_mm: f64,
    /// Height the contours were cut at (the slab's middle).
    pub sample_mm: f64,
}

impl LayerSpan {
    /// The slab's thickness.
    #[inline]
    pub fn height_mm(&self) -> f64 {
        self.top_mm - self.bottom_mm
    }
}

/// Why [`ReviewInput::new`] refused its stacks.
#[derive(Clone, Debug, PartialEq)]
pub enum ReviewInputError {
    /// A stack has a different number of layers than the schedule.
    LayerCountMismatch {
        /// Index of the leaf in the list given.
        leaf: usize,
        /// Layers in the schedule.
        expected: usize,
        /// Layers in the stack.
        found: usize,
    },
    /// A stack layer was cut at another height than the schedule's.
    LayerZMismatch {
        /// Index of the leaf in the list given.
        leaf: usize,
        /// The layer index.
        layer: usize,
        /// The schedule's cutting height.
        expected_z_mm: f64,
        /// The stack's cutting height.
        found_z_mm: f64,
    },
}

impl fmt::Display for ReviewInputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LayerCountMismatch {
                leaf,
                expected,
                found,
            } => write!(
                f,
                "leaf {leaf} has {found} layers, but the layer schedule has {expected}"
            ),
            Self::LayerZMismatch {
                leaf,
                layer,
                expected_z_mm,
                found_z_mm,
            } => write!(
                f,
                "leaf {leaf} cut layer {layer} at z = {found_z_mm} mm, but the layer schedule cuts it at z = {expected_z_mm} mm"
            ),
        }
    }
}

impl std::error::Error for ReviewInputError {}

/// One slicing run, ready to review.
///
/// Every stack has one layer per scheduled layer, cut at the schedule's
/// heights; [`ReviewInput::new`] checks it.
#[derive(Clone, Debug)]
pub struct ReviewInput<'a> {
    leaves: Vec<ReviewLeaf<'a>>,
    layers: Vec<LayerSpan>,
    pixel_map: PixelMap,
    max_z_mm: f64,
    raft_top_mm: Option<f64>,
    support_lift_mm: Option<f64>,
    tips: Vec<TipCone>,
    support_layers: Range<usize>,
    support_box_mm: Option<[f64; 5]>,
}

impl<'a> ReviewInput<'a> {
    /// Pairs `leaves` with the `schedule` they were cut on, the printer's
    /// screen `pixel_map` and its maximum height `max_z`.
    ///
    /// # Errors
    ///
    /// [`ReviewInputError`] when a stack does not follow `schedule`.
    pub fn new(
        leaves: Vec<ReviewLeaf<'a>>,
        schedule: &LayerSchedule,
        pixel_map: PixelMap,
        max_z: PositiveFiniteMm,
    ) -> Result<Self, ReviewInputError> {
        let mut bottom = 0.0;
        let layers: Vec<LayerSpan> = schedule
            .iter_z()
            .zip(schedule.iter_slab_tops())
            .map(|(sample_mm, top_mm)| {
                let span = LayerSpan {
                    bottom_mm: bottom,
                    top_mm,
                    sample_mm,
                };
                bottom = top_mm;
                span
            })
            .collect();
        for (leaf, review_leaf) in leaves.iter().enumerate() {
            let stack_layers = review_leaf.stack.layers();
            if stack_layers.len() != layers.len() {
                return Err(ReviewInputError::LayerCountMismatch {
                    leaf,
                    expected: layers.len(),
                    found: stack_layers.len(),
                });
            }
            for (layer, (span, contours)) in layers.iter().zip(stack_layers).enumerate() {
                if contours.z_mm() != span.sample_mm {
                    return Err(ReviewInputError::LayerZMismatch {
                        leaf,
                        layer,
                        expected_z_mm: span.sample_mm,
                        found_z_mm: contours.z_mm(),
                    });
                }
            }
        }
        Ok(Self {
            leaves,
            layers,
            pixel_map,
            max_z_mm: max_z.value(),
            raft_top_mm: None,
            support_lift_mm: None,
            tips: Vec::new(),
            support_layers: 0..0,
            support_box_mm: None,
        })
    }

    /// Pairs `leaves` with the engine profile they were sliced with: its
    /// schedule, its screen (build volume X and Y, pixel resolution, mirror)
    /// and its maximum height (build volume Z).
    ///
    /// # Errors
    ///
    /// As [`ReviewInput::new`].
    pub fn from_profile(
        leaves: Vec<ReviewLeaf<'a>>,
        profile: &SliceProfile,
    ) -> Result<Self, ReviewInputError> {
        let volume = profile.build_volume();
        let pixel_map = PixelMap::new(
            volume.x(),
            volume.y(),
            *profile.pixel_resolution(),
            profile.mirror(),
        );
        Self::new(leaves, profile.layer_schedule(), pixel_map, volume.z())
    }

    /// The same input, knowing that the generated supports stand on a raft
    /// whose top is `raft_top_mm` above the plate (its plate and wall), or
    /// on none (`None`, the default). The raft check (#131) reads it.
    #[must_use]
    pub fn with_raft_top_mm(mut self, raft_top_mm: Option<f64>) -> Self {
        self.raft_top_mm = raft_top_mm.filter(|top| top.is_finite() && *top > 0.0);
        self
    }

    /// The same input, knowing how high a model should stand off the plate
    /// to leave its supports room under it (the generator's default support
    /// lift, or a higher configured one). The raft check (#131) recommends
    /// it and folds the islands a model in the raft starts below it into
    /// its finding.
    #[must_use]
    pub fn with_support_lift_mm(mut self, support_lift_mm: Option<f64>) -> Self {
        self.support_lift_mm = support_lift_mm.filter(|lift| lift.is_finite() && *lift > 0.0);
        self
    }

    /// The same input, with the tips of the supports made here, in
    /// printer-frame millimetres, as the slice placed them.
    #[must_use]
    pub fn with_tips(mut self, tips: Vec<TipCone>) -> Self {
        self.tips = tips;
        self
    }

    /// The same input, knowing that the supports made here stand on `layers`
    /// and take `box_mm`: `[min x, min y, max x, max y, top]` in plate
    /// millimetres, `None` when that is not known.
    #[must_use]
    pub fn with_support_extent(
        mut self,
        layers: Range<usize>,
        box_mm: Option<[f64; 5]>,
    ) -> Self {
        self.support_layers = layers;
        self.support_box_mm = box_mm;
        self
    }

    /// The tips of the supports made here.
    #[inline]
    pub fn tips(&self) -> &[TipCone] {
        &self.tips
    }

    /// Whether supports made here stand on layer `layer`.
    #[inline]
    pub fn has_support_on(&self, layer: usize) -> bool {
        self.support_layers.contains(&layer)
    }

    /// The box the supports made here take, when it is known:
    /// `[min x, min y, max x, max y, top]` in plate millimetres.
    #[inline]
    pub fn support_box_mm(&self) -> Option<[f64; 5]> {
        self.support_box_mm
    }

    /// The lift recommended for a model in the raft: the support lift given
    /// to [`Self::with_support_lift_mm`], never below the top of the raft's
    /// last layer; `None` without a raft.
    pub fn recommended_lift_mm(&self) -> Option<f64> {
        let zone = self.raft_layers();
        let raft_clear = match zone {
            0 => self.raft_top_mm?,
            n => self.layers[n - 1].top_mm,
        };
        Some(
            self.support_lift_mm
                .map_or(raft_clear, |lift| lift.max(raft_clear)),
        )
    }

    /// The layers that start below [`Self::recommended_lift_mm`]: where an
    /// island of a model in the raft is the raft's doing. 0 without a raft.
    pub fn lift_layers(&self) -> usize {
        let Some(lift) = self.recommended_lift_mm() else {
            return 0;
        };
        self.layers
            .iter()
            .take_while(|span| span.bottom_mm < lift - 1e-6)
            .count()
    }

    /// The top of the supports' raft above the plate, when there is one.
    #[inline]
    pub fn raft_top_mm(&self) -> Option<f64> {
        self.raft_top_mm
    }

    /// The layers the raft takes from the plate up: every layer whose slab
    /// starts below the raft's top. 0 without a raft.
    pub fn raft_layers(&self) -> usize {
        let Some(top) = self.raft_top_mm else {
            return 0;
        };
        // A hair under a slab's bottom does not reach into it.
        self.layers
            .iter()
            .take_while(|span| span.bottom_mm < top - 1e-6)
            .count()
    }

    /// The leaves, in the order given.
    #[inline]
    pub fn leaves(&self) -> &[ReviewLeaf<'a>] {
        &self.leaves
    }

    /// Every layer's slab, from the plate up.
    #[inline]
    pub fn layers(&self) -> &[LayerSpan] {
        &self.layers
    }

    /// The number of layers.
    #[inline]
    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }

    /// The printer's screen: plate size, pixel resolution and mirror.
    #[inline]
    pub fn pixel_map(&self) -> &PixelMap {
        &self.pixel_map
    }

    /// The screen width in pixels.
    #[inline]
    pub fn width_px(&self) -> u32 {
        self.pixel_map.pixel_resolution().x()
    }

    /// The screen height in pixels.
    #[inline]
    pub fn height_px(&self) -> u32 {
        self.pixel_map.pixel_resolution().y()
    }

    /// The size of one pixel in millimetres, `[x, y]`.
    pub fn pixel_size_mm(&self) -> [f64; 2] {
        [
            self.pixel_map.plate_x().value() / f64::from(self.width_px()),
            self.pixel_map.plate_y().value() / f64::from(self.height_px()),
        ]
    }

    /// The area of one pixel in square millimetres.
    pub fn pixel_area_mm2(&self) -> f64 {
        let [x, y] = self.pixel_size_mm();
        x * y
    }

    /// The printer's maximum printing height in millimetres.
    #[inline]
    pub fn max_z_mm(&self) -> f64 {
        self.max_z_mm
    }

    /// Converts a point in raster space (pixels, `(0, 0)` at the corner of
    /// pixel `(0, 0)`; the centre of pixel `(x, y)` is `(x + 0.5, y + 0.5)`)
    /// back to printer-frame millimetres, undoing the screen's mirror.
    pub fn pixel_to_plate(&self, pixel: [f64; 2]) -> [f64; 2] {
        pixel_to_plate(&self.pixel_map, pixel)
    }

    /// The camera focus for raster point `pixel` on layer `layer`: the point
    /// in millimetres at the height the layer was cut at.
    ///
    /// # Panics
    ///
    /// When `layer` is out of range.
    pub fn focus(&self, layer: usize, pixel: [f64; 2]) -> PlatePoint {
        let [x_mm, y_mm] = self.pixel_to_plate(pixel);
        PlatePoint::new(x_mm, y_mm, self.layers[layer].sample_mm)
    }

    /// Where a support for solid that starts on layer `layer` at `focus`
    /// touches it: the same spot, at the bottom of the layer. The surface
    /// nearest that point is the solid's underside, which a support holds;
    /// the one nearest the layer's middle can be a side wall.
    ///
    /// # Panics
    ///
    /// When `layer` is out of range.
    pub fn underside(&self, layer: usize, focus: PlatePoint) -> PlatePoint {
        PlatePoint::new(focus.x_mm, focus.y_mm, self.layers[layer].bottom_mm)
    }
}

/// The inverse of [`PixelMap::mm_to_pixel`]: raster space back to
/// printer-frame millimetres.
pub fn pixel_to_plate(pixel_map: &PixelMap, pixel: [f64; 2]) -> [f64; 2] {
    let res_x = f64::from(pixel_map.pixel_resolution().x());
    let res_y = f64::from(pixel_map.pixel_resolution().y());
    let plate_x = pixel_map.plate_x().value();
    let plate_y = pixel_map.plate_y().value();
    let mirror = pixel_map.mirror();
    let raw_x = if mirror.x { res_x - pixel[0] } else { pixel[0] };
    let raw_y = if mirror.y { res_y - pixel[1] } else { pixel[1] };
    [
        raw_x * plate_x / res_x - 0.5 * plate_x,
        raw_y * plate_y / res_y - 0.5 * plate_y,
    ]
}

#[cfg(test)]
mod input_tests {
    use sc_print_primitives::{Mirror, PixelResolution};
    use sc_sl1cer::contour::ContourLayer;
    use sc_sl1cer::geom2d::Point2D;

    use super::*;

    fn mm(v: f64) -> PositiveFiniteMm {
        PositiveFiniteMm::new(v).unwrap()
    }

    #[test]
    fn pixel_to_plate_inverts_the_projection_under_every_mirror() {
        for (x, y) in [(false, false), (true, false), (false, true), (true, true)] {
            let map = PixelMap::new(
                mm(60.0),
                mm(40.0),
                PixelResolution::new(600, 400).unwrap(),
                Mirror { x, y },
            );
            let point = Point2D::new(12.5, -7.25);
            let pixel = map.mm_to_pixel(point);
            let back = pixel_to_plate(&map, [pixel.0, pixel.1]);
            assert!((back[0] - point.x).abs() < 1e-9, "{back:?}");
            assert!((back[1] - point.y).abs() < 1e-9, "{back:?}");
        }
    }

    #[test]
    fn layers_follow_the_schedule() {
        let schedule = LayerSchedule::uniform(0.05, 0.1, 3).unwrap();
        let map = PixelMap::new(
            mm(10.0),
            mm(10.0),
            PixelResolution::new(100, 100).unwrap(),
            Mirror::default(),
        );
        let input = ReviewInput::new(Vec::new(), &schedule, map, mm(5.0)).unwrap();
        let tops: Vec<f64> = input.layers().iter().map(|l| l.top_mm).collect();
        assert_eq!(input.layer_count(), 3);
        assert_eq!(input.layers()[0].bottom_mm, 0.0);
        assert_eq!(input.layers()[1].bottom_mm, tops[0]);
        assert!((input.layers()[0].height_mm() - 0.1).abs() < 1e-12);
        assert!((input.layers()[2].height_mm() - 0.05).abs() < 1e-12);
        assert_eq!(input.pixel_size_mm(), [0.1, 0.1]);
        assert_eq!(input.raft_layers(), 0);
        let rafted = input.with_raft_top_mm(Some(0.15));
        assert_eq!(rafted.raft_top_mm(), Some(0.15));
        assert_eq!(rafted.raft_layers(), 2);
        assert!((rafted.recommended_lift_mm().unwrap() - 0.15).abs() < 1e-9);
        assert_eq!(rafted.lift_layers(), 2);
        let lifted = rafted.clone().with_support_lift_mm(Some(0.25));
        assert_eq!(lifted.lift_layers(), 3);
        assert_eq!(lifted.recommended_lift_mm(), Some(0.25));
        assert_eq!(input_without_raft_lift(&lifted), None);
        assert_eq!(rafted.clone().with_raft_top_mm(Some(9.0)).raft_layers(), 3);
        assert_eq!(rafted.with_raft_top_mm(Some(-1.0)).raft_top_mm(), None);
    }

    fn input_without_raft_lift(input: &ReviewInput<'_>) -> Option<f64> {
        input.clone().with_raft_top_mm(None).recommended_lift_mm()
    }

    #[test]
    fn stacks_off_the_schedule_are_refused() {
        let schedule = LayerSchedule::uniform(0.05, 0.05, 2).unwrap();
        let map = PixelMap::new(
            mm(10.0),
            mm(10.0),
            PixelResolution::new(100, 100).unwrap(),
            Mirror::default(),
        );
        let short = ContourStack::new_unchecked(vec![ContourLayer::new(0.025, vec![])]);
        let err = ReviewInput::new(
            vec![ReviewLeaf::new(None, &short)],
            &schedule,
            map,
            mm(5.0),
        )
        .unwrap_err();
        assert_eq!(
            err,
            ReviewInputError::LayerCountMismatch {
                leaf: 0,
                expected: 2,
                found: 1
            }
        );
        let shifted = ContourStack::new_unchecked(vec![
            ContourLayer::new(0.025, vec![]),
            ContourLayer::new(0.08, vec![]),
        ]);
        let err = ReviewInput::new(
            vec![ReviewLeaf::new(None, &shifted)],
            &schedule,
            map,
            mm(5.0),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ReviewInputError::LayerZMismatch { layer: 1, .. }
        ));
    }
}
