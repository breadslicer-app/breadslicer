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
//! - The build-plate origin in millimetres sits at the pixel-space
//!   origin. Mm `(0, 0)` projects to pixel `(0, 0)`; mm
//!   `(build_volume.x_mm(), build_volume.y_mm())` projects to pixel
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

    /// Project a 2D point in millimetres to subpixel raster
    /// coordinates. With the corresponding mirror flag set, the
    /// projected axis is reflected across the centre of the
    /// raster.
    pub fn mm_to_pixel(&self, point: Point2D) -> (f64, f64) {
        let res_x = f64::from(self.pixel_resolution.x());
        let res_y = f64::from(self.pixel_resolution.y());
        let raw_x = point.x * res_x / self.build_volume.x_mm();
        let raw_y = point.y * res_y / self.build_volume.y_mm();
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
    fn origin_maps_to_pixel_origin_without_mirror() {
        let pm = unmirrored_pixel_map(60.0, 60.0, 1920, 1080);
        assert_eq!(pm.mm_to_pixel(p(0.0, 0.0)), (0.0, 0.0));
    }

    #[test]
    fn build_volume_corner_maps_to_pixel_resolution_without_mirror() {
        let pm = unmirrored_pixel_map(60.0, 60.0, 1920, 1080);
        assert_eq!(pm.mm_to_pixel(p(60.0, 60.0)), (1920.0, 1080.0));
    }

    #[test]
    fn build_volume_centre_maps_to_raster_centre() {
        let pm = unmirrored_pixel_map(60.0, 40.0, 600, 400);
        assert_eq!(pm.mm_to_pixel(p(30.0, 20.0)), (300.0, 200.0));
    }

    #[test]
    fn axes_are_independent() {
        let pm = unmirrored_pixel_map(60.0, 40.0, 600, 400);
        // 10 mm along X with build = 60 mm and res = 600 px -> 100 px.
        // 30 mm along Y with build = 40 mm and res = 400 px -> 300 px.
        assert_eq!(pm.mm_to_pixel(p(10.0, 30.0)), (100.0, 300.0));
    }

    #[test]
    fn subpixel_input_yields_subpixel_output() {
        let pm = unmirrored_pixel_map(2.0, 2.0, 200, 200);
        let (px, py) = pm.mm_to_pixel(p(0.123, 1.876));
        // 0.123 mm * 100 px/mm = 12.3 px; 1.876 mm * 100 px/mm = 187.6 px.
        assert!((px - 12.3).abs() < 1e-12);
        assert!((py - 187.6).abs() < 1e-12);
    }

    #[test]
    fn mirror_x_reflects_origin_to_right_edge() {
        let pm = PixelMap::new(
            BuildVolume::new(60.0, 60.0, 100.0).unwrap(),
            PixelResolution::new(1920, 1080).unwrap(),
            Mirror { x: true, y: false },
        );
        assert_eq!(pm.mm_to_pixel(p(0.0, 0.0)), (1920.0, 0.0));
        assert_eq!(pm.mm_to_pixel(p(60.0, 60.0)), (0.0, 1080.0));
    }

    #[test]
    fn mirror_y_reflects_origin_to_top_edge() {
        let pm = PixelMap::new(
            BuildVolume::new(60.0, 60.0, 100.0).unwrap(),
            PixelResolution::new(1920, 1080).unwrap(),
            Mirror { x: false, y: true },
        );
        assert_eq!(pm.mm_to_pixel(p(0.0, 0.0)), (0.0, 1080.0));
        assert_eq!(pm.mm_to_pixel(p(60.0, 60.0)), (1920.0, 0.0));
    }

    #[test]
    fn mirror_both_reflects_to_opposite_corner() {
        let pm = PixelMap::new(
            BuildVolume::new(60.0, 60.0, 100.0).unwrap(),
            PixelResolution::new(1920, 1080).unwrap(),
            Mirror { x: true, y: true },
        );
        assert_eq!(pm.mm_to_pixel(p(0.0, 0.0)), (1920.0, 1080.0));
        assert_eq!(pm.mm_to_pixel(p(60.0, 60.0)), (0.0, 0.0));
    }

    #[test]
    fn mirror_preserves_centre() {
        let pm = PixelMap::new(
            BuildVolume::new(60.0, 40.0, 100.0).unwrap(),
            PixelResolution::new(600, 400).unwrap(),
            Mirror { x: true, y: true },
        );
        assert_eq!(pm.mm_to_pixel(p(30.0, 20.0)), (300.0, 200.0));
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
