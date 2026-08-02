/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Millimetre-to-pixel projection for the raster tier.
//!
//! [`PixelMap`] derives the per-axis mm-to-pixel ratios from a
//! [`BuildVolume`] and a [`PixelResolution`], and applies the
//! per-axis [`Mirror`] flags so downstream scanline fill code can
//! ignore raster orientation.
//!
//! Frame conventions:
//!
//! - Input points are printer-frame millimetres: XY centred on the
//!   plate origin, the frame the build volume's envelope corners
//!   define. Mm `(0, 0)` (the plate centre) projects to the raster
//!   centre; the envelope's minimum XY corner projects to pixel
//!   `(0, 0)` and its maximum XY corner to pixel
//!   `(pixel_resolution.x(), pixel_resolution.y())`.
//! - Pixel coordinates are subpixel `f64`, not integer indices.
//!   Integer rounding is the scanline fill's job; keeping subpixel
//!   precision through this layer lets analytic-coverage AA read
//!   fractional positions without re-projecting.

use crate::geom2d::Point2D;
use crate::profile::{BuildVolume, Mirror, PixelResolution};

/// Mapping from millimetres in the build-plate frame to subpixel
/// coordinates in raster space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PixelMap {
    build_volume: BuildVolume,
    pixel_resolution: PixelResolution,
    mirror: Mirror,
}

impl PixelMap {
    /// Assemble a [`PixelMap`] from its already-validated components.
    /// Infallible: the build volume and pixel resolution are checked
    /// by their own constructors.
    #[inline]
    pub fn new(
        build_volume: BuildVolume,
        pixel_resolution: PixelResolution,
        mirror: Mirror,
    ) -> Self {
        Self {
            build_volume,
            pixel_resolution,
            mirror,
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
    pub fn mirror(&self) -> Mirror {
        self.mirror
    }

    /// Project a 2D point in printer-frame millimetres (XY centred
    /// on the plate origin) to subpixel raster coordinates. With the
    /// corresponding mirror flag set, the projected axis is
    /// reflected across the centre of the raster; in printer-frame
    /// terms that reflection is the same as negating the coordinate
    /// before projecting.
    pub fn mm_to_pixel(&self, point: Point2D) -> (f64, f64) {
        let res_x = f64::from(self.pixel_resolution.x());
        let res_y = f64::from(self.pixel_resolution.y());
        let bx = self.build_volume.x_mm();
        let by = self.build_volume.y_mm();
        let raw_x = (point.x + 0.5 * bx) * res_x / bx;
        let raw_y = (point.y + 0.5 * by) * res_y / by;
        let px_x = if self.mirror.x { res_x - raw_x } else { raw_x };
        let px_y = if self.mirror.y { res_y - raw_y } else { raw_y };
        (px_x, px_y)
    }
}

#[cfg(test)]
mod pixel_map_tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point2D {
        Point2D::new(x, y)
    }

    fn unmirrored_pixel_map(bx: f64, by: f64, res_x: u32, res_y: u32) -> PixelMap {
        PixelMap::new(
            BuildVolume::new(bx, by, 100.0).unwrap(),
            PixelResolution::new(res_x, res_y).unwrap(),
            Mirror::default(),
        )
    }

    #[test]
    fn plate_centre_maps_to_raster_centre_without_mirror() {
        let pm = unmirrored_pixel_map(60.0, 60.0, 1920, 1080);
        assert_eq!(pm.mm_to_pixel(p(0.0, 0.0)), (960.0, 540.0));
    }

    #[test]
    fn envelope_min_corner_maps_to_pixel_origin_without_mirror() {
        let pm = unmirrored_pixel_map(60.0, 60.0, 1920, 1080);
        assert_eq!(pm.mm_to_pixel(p(-30.0, -30.0)), (0.0, 0.0));
    }

    #[test]
    fn envelope_max_corner_maps_to_pixel_resolution_without_mirror() {
        let pm = unmirrored_pixel_map(60.0, 60.0, 1920, 1080);
        assert_eq!(pm.mm_to_pixel(p(30.0, 30.0)), (1920.0, 1080.0));
    }

    #[test]
    fn axes_are_independent() {
        let pm = unmirrored_pixel_map(60.0, 40.0, 600, 400);
        // -20 mm along X with build = 60 mm and res = 600 px -> 100 px.
        // +10 mm along Y with build = 40 mm and res = 400 px -> 300 px.
        assert_eq!(pm.mm_to_pixel(p(-20.0, 10.0)), (100.0, 300.0));
    }

    #[test]
    fn subpixel_input_yields_subpixel_output() {
        let pm = unmirrored_pixel_map(2.0, 2.0, 200, 200);
        let (px, py) = pm.mm_to_pixel(p(-0.877, 0.876));
        // (-0.877 + 1) mm * 100 px/mm = 12.3 px;
        // (0.876 + 1) mm * 100 px/mm = 187.6 px.
        assert!((px - 12.3).abs() < 1e-12);
        assert!((py - 187.6).abs() < 1e-12);
    }

    #[test]
    fn mirror_x_reflects_min_corner_to_right_edge() {
        let pm = PixelMap::new(
            BuildVolume::new(60.0, 60.0, 100.0).unwrap(),
            PixelResolution::new(1920, 1080).unwrap(),
            Mirror { x: true, y: false },
        );
        assert_eq!(pm.mm_to_pixel(p(-30.0, -30.0)), (1920.0, 0.0));
        assert_eq!(pm.mm_to_pixel(p(30.0, 30.0)), (0.0, 1080.0));
    }

    #[test]
    fn mirror_y_reflects_min_corner_to_top_edge() {
        let pm = PixelMap::new(
            BuildVolume::new(60.0, 60.0, 100.0).unwrap(),
            PixelResolution::new(1920, 1080).unwrap(),
            Mirror { x: false, y: true },
        );
        assert_eq!(pm.mm_to_pixel(p(-30.0, -30.0)), (0.0, 1080.0));
        assert_eq!(pm.mm_to_pixel(p(30.0, 30.0)), (1920.0, 0.0));
    }

    #[test]
    fn mirror_both_reflects_to_opposite_corner() {
        let pm = PixelMap::new(
            BuildVolume::new(60.0, 60.0, 100.0).unwrap(),
            PixelResolution::new(1920, 1080).unwrap(),
            Mirror { x: true, y: true },
        );
        assert_eq!(pm.mm_to_pixel(p(-30.0, -30.0)), (1920.0, 1080.0));
        assert_eq!(pm.mm_to_pixel(p(30.0, 30.0)), (0.0, 0.0));
    }

    #[test]
    fn mirror_preserves_plate_centre() {
        let pm = PixelMap::new(
            BuildVolume::new(60.0, 40.0, 100.0).unwrap(),
            PixelResolution::new(600, 400).unwrap(),
            Mirror { x: true, y: true },
        );
        assert_eq!(pm.mm_to_pixel(p(0.0, 0.0)), (300.0, 200.0));
    }

    #[test]
    fn mirror_equals_negating_the_printer_coordinate() {
        let plain = unmirrored_pixel_map(60.0, 40.0, 600, 400);
        let mirrored = PixelMap::new(
            BuildVolume::new(60.0, 40.0, 100.0).unwrap(),
            PixelResolution::new(600, 400).unwrap(),
            Mirror { x: true, y: true },
        );
        assert_eq!(
            mirrored.mm_to_pixel(p(12.5, -7.25)),
            plain.mm_to_pixel(p(-12.5, 7.25)),
        );
    }

    #[test]
    fn accessors_round_trip_construction_inputs() {
        let bv = BuildVolume::new(60.0, 40.0, 100.0).unwrap();
        let res = PixelResolution::new(600, 400).unwrap();
        let mirror = Mirror { x: true, y: false };
        let pm = PixelMap::new(bv, res, mirror);
        assert_eq!(pm.build_volume(), &bv);
        assert_eq!(pm.pixel_resolution(), &res);
        assert_eq!(pm.mirror(), mirror);
    }
}
