/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Row-major run-length-encoded raster types.
//!
//! Runs may span row breaks: a single [`RleRun`] of value `255` and
//! length `width_px + 3` covers the last three pixels of one row and
//! all but the last three pixels of the next. The layout matches the
//! CTB / RLE7-style on-disk format so a future encoder can consume
//! the same stream with minimal adapter code, and lets a streaming
//! scanline pass emit pixels without per-row bookkeeping.
//!
//! [`RleRunAccumulator`] is the building block: callers push
//! `(value, length)` pairs in row-major order and the accumulator
//! merges adjacent same-value pushes on the fly. The accumulator
//! exposes [`RleRunAccumulator::into_runs`] to surrender ownership of
//! the run buffer once a layer is fully encoded.

/// One row-major run of `length` pixels at the 8-bit grayscale
/// `value`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RleRun {
    pub value: u8,
    pub length: u32,
}

/// One layer's worth of row-major grayscale pixels in RLE form.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerRaster {
    z_mm: f64,
    width_px: u32,
    height_px: u32,
    runs: Vec<RleRun>,
}

impl LayerRaster {
    #[inline]
    pub fn new(z_mm: f64, width_px: u32, height_px: u32, runs: Vec<RleRun>) -> Self {
        Self {
            z_mm,
            width_px,
            height_px,
            runs,
        }
    }

    #[inline]
    pub fn z_mm(&self) -> f64 {
        self.z_mm
    }

    #[inline]
    pub fn width_px(&self) -> u32 {
        self.width_px
    }

    #[inline]
    pub fn height_px(&self) -> u32 {
        self.height_px
    }

    #[inline]
    pub fn runs(&self) -> &[RleRun] {
        &self.runs
    }
}

/// Every raster layer produced for one slicing job, in the same
/// order as the upstream [`ContourStack`](crate::contour::ContourStack).
#[derive(Clone, Debug, PartialEq)]
pub struct LayerRasterStack {
    layers: Vec<LayerRaster>,
}

impl LayerRasterStack {
    #[inline]
    pub fn new(layers: Vec<LayerRaster>) -> Self {
        Self { layers }
    }

    #[inline]
    pub fn layers(&self) -> &[LayerRaster] {
        &self.layers
    }
}

/// Streaming builder for a [`LayerRaster`]'s run buffer. Adjacent
/// pushes of the same `value` are merged into a single run on the
/// fly; zero-length pushes are dropped.
#[derive(Clone, Debug, Default)]
pub struct RleRunAccumulator {
    runs: Vec<RleRun>,
}

impl RleRunAccumulator {
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a `(value, length)` pair. If `length` is zero the call is
    /// a no-op. If `value` matches the last run already in the buffer
    /// the two runs are merged into one.
    pub fn push(&mut self, value: u8, length: u32) {
        if length == 0 {
            return;
        }
        if let Some(last) = self.runs.last_mut()
            && last.value == value
        {
            last.length += length;
            return;
        }
        self.runs.push(RleRun { value, length });
    }

    /// Number of distinct runs currently held.
    #[inline]
    pub fn len(&self) -> usize {
        self.runs.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// Surrender ownership of the run buffer.
    #[inline]
    pub fn into_runs(self) -> Vec<RleRun> {
        self.runs
    }
}

#[cfg(test)]
mod rle_tests {
    use super::*;

    #[test]
    fn empty_accumulator_produces_no_runs() {
        let acc = RleRunAccumulator::new();
        assert!(acc.is_empty());
        assert_eq!(acc.into_runs(), Vec::<RleRun>::new());
    }

    #[test]
    fn single_push_produces_one_run() {
        let mut acc = RleRunAccumulator::new();
        acc.push(0, 100);
        assert_eq!(acc.len(), 1);
        assert_eq!(
            acc.into_runs(),
            vec![RleRun {
                value: 0,
                length: 100,
            }],
        );
    }

    #[test]
    fn two_distinct_values_become_two_runs() {
        let mut acc = RleRunAccumulator::new();
        acc.push(0, 100);
        acc.push(255, 50);
        assert_eq!(
            acc.into_runs(),
            vec![
                RleRun {
                    value: 0,
                    length: 100,
                },
                RleRun {
                    value: 255,
                    length: 50,
                },
            ],
        );
    }

    #[test]
    fn consecutive_same_value_pushes_collapse_to_one_run() {
        let mut acc = RleRunAccumulator::new();
        acc.push(0, 100);
        acc.push(255, 50);
        acc.push(255, 50);
        acc.push(0, 1);
        assert_eq!(
            acc.into_runs(),
            vec![
                RleRun {
                    value: 0,
                    length: 100,
                },
                RleRun {
                    value: 255,
                    length: 100,
                },
                RleRun {
                    value: 0,
                    length: 1,
                },
            ],
        );
    }

    #[test]
    fn zero_length_push_is_a_no_op() {
        let mut acc = RleRunAccumulator::new();
        acc.push(0, 0);
        assert!(acc.is_empty());

        acc.push(255, 5);
        acc.push(255, 0);
        acc.push(255, 3);
        assert_eq!(
            acc.into_runs(),
            vec![RleRun {
                value: 255,
                length: 8,
            }],
        );
    }

    #[test]
    fn into_runs_drains_the_buffer() {
        let mut acc = RleRunAccumulator::new();
        acc.push(128, 7);
        let runs = acc.into_runs();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].value, 128);
        assert_eq!(runs[0].length, 7);
    }

    #[test]
    fn layer_raster_accessors_round_trip_construction_inputs() {
        let runs = vec![
            RleRun {
                value: 0,
                length: 100,
            },
            RleRun {
                value: 255,
                length: 50,
            },
        ];
        let layer = LayerRaster::new(0.05, 200, 100, runs.clone());
        assert_eq!(layer.z_mm(), 0.05);
        assert_eq!(layer.width_px(), 200);
        assert_eq!(layer.height_px(), 100);
        assert_eq!(layer.runs(), runs.as_slice());
    }

    #[test]
    fn layer_raster_stack_accessors_round_trip_construction_inputs() {
        let layers = vec![
            LayerRaster::new(0.05, 10, 10, vec![]),
            LayerRaster::new(0.15, 10, 10, vec![]),
        ];
        let stack = LayerRasterStack::new(layers.clone());
        assert_eq!(stack.layers(), layers.as_slice());
    }
}
