/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

//! What a layer image covers, read off its run-length rows.
//!
//! Both containers record a few figures per layer beside its image: where the
//! lit pixels sit and how much of the panel they cover. UVtools measures them on
//! a decoded image; here they come from the runs the image is encoded from, so
//! no frame is materialised for them.

use sc_sl1cer::contour::ContourLayer;
use sc_sl1cer::raster::rle::RleRun;

/// A rectangle of pixels, `x1` and `y1` exclusive. Rows count from the image's
/// first row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PixelRect {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

impl PixelRect {
    pub(crate) fn width(&self) -> u32 {
        self.x1 - self.x0
    }
}

/// The figures of one layer image.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct LayerStats {
    /// The smallest rectangle holding every lit pixel, `None` for a dark layer.
    pub bounds: Option<PixelRect>,
    /// The lit fraction of each pixel summed over the layer, in pixels: a
    /// pixel at full exposure counts 1, an anti-aliased edge pixel its share.
    pub covered_pixels: f64,
}

impl LayerStats {
    /// Measures a frame of `runs`, `width` pixels per row, rows from the
    /// image's first.
    pub(crate) fn of(runs: &[RleRun], width: u32) -> Self {
        let width = u64::from(width);
        let mut position = 0_u64;
        let mut covered = 0_u64;
        let mut bounds: Option<PixelRect> = None;

        for run in runs {
            let length = u64::from(run.length);
            if run.value != 0 && length > 0 {
                covered += u64::from(run.value) * length;
                let first = position;
                let last = position + length - 1;
                let (first_row, first_col) = (first / width, first % width);
                let (last_row, last_col) = (last / width, last % width);
                // A run that wraps a row covers every column between.
                let (x0, x1) = if first_row == last_row {
                    (first_col, last_col + 1)
                } else {
                    (0, width)
                };
                let rect = PixelRect {
                    x0: x0 as u32,
                    y0: first_row as u32,
                    x1: x1 as u32,
                    y1: last_row as u32 + 1,
                };
                bounds = Some(match bounds {
                    None => rect,
                    Some(b) => PixelRect {
                        x0: b.x0.min(rect.x0),
                        y0: b.y0.min(rect.y0),
                        x1: b.x1.max(rect.x1),
                        y1: b.y1.max(rect.y1),
                    },
                });
            }
            position += length;
        }

        Self {
            bounds,
            covered_pixels: covered as f64 / 255.0,
        }
    }
}

/// The contour figures of one layer, in square millimetres: how many separate
/// outlines it has, the area inside the largest one (holes included, as CXDLP
/// records it) and the largest area one outline encloses once its holes are
/// taken out (as the Anycubic ZIP scene records it).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ContourStats {
    pub outline_count: u32,
    pub largest_outline_mm2: f64,
    pub largest_solid_mm2: f64,
}

impl ContourStats {
    pub(crate) fn of(layer: &ContourLayer) -> Self {
        let polygons = layer.polygons();
        let largest_outline_mm2 = polygons
            .iter()
            .map(|polygon| polygon.outer().signed_area().abs())
            .fold(0.0_f64, f64::max);
        let largest_solid_mm2 = polygons
            .iter()
            .map(|polygon| {
                let holes: f64 = polygon
                    .holes()
                    .iter()
                    .map(|hole| hole.signed_area().abs())
                    .sum();
                (polygon.outer().signed_area().abs() - holes).max(0.0)
            })
            .fold(0.0_f64, f64::max);
        Self {
            outline_count: u32::try_from(polygons.len()).unwrap_or(u32::MAX),
            largest_outline_mm2,
            largest_solid_mm2,
        }
    }
}

#[cfg(test)]
mod layer_stats_tests {
    use super::*;

    fn run(value: u8, length: u32) -> RleRun {
        RleRun { value, length }
    }

    #[test]
    fn a_dark_frame_has_no_bounds_and_covers_nothing() {
        let stats = LayerStats::of(&[run(0, 100)], 10);
        assert_eq!(stats.bounds, None);
        assert_eq!(stats.covered_pixels, 0.0);
    }

    #[test]
    fn bounds_hold_every_lit_pixel() {
        // 10 x 4 frame: row 1 columns 2..5 full, row 2 column 7 at half.
        let runs = [run(0, 12), run(255, 3), run(0, 12), run(128, 1), run(0, 12)];
        let stats = LayerStats::of(&runs, 10);
        assert_eq!(
            stats.bounds,
            Some(PixelRect {
                x0: 2,
                y0: 1,
                x1: 8,
                y1: 3
            })
        );
        assert!((stats.covered_pixels - (3.0 + 128.0 / 255.0)).abs() < 1e-9);
    }

    #[test]
    fn a_run_wrapping_a_row_spans_the_full_width() {
        let runs = [run(0, 8), run(255, 4), run(0, 8)];
        let stats = LayerStats::of(&runs, 10);
        assert_eq!(
            stats.bounds,
            Some(PixelRect {
                x0: 0,
                y0: 0,
                x1: 10,
                y1: 2
            })
        );
    }
}
