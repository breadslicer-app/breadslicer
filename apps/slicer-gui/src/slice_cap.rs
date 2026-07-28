/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Builds the slice-view cap raster: the selected layer rasterised to a
//! single-channel coverage bitmap that fills the 3D cut so it reads
//! solid.
//!
//! The cap is a viewport preview, so it rasterises at a modest display
//! resolution (the longest build-area side maps to at most
//! [`MAX_CAP_SIDE_PX`] texels), not the printer's full pixel grid. The
//! world-centred contour is shifted into the corner-origin raster frame
//! before filling, matching the millimetre-to-pixel convention of
//! [`PixelMap`].

use sc_print_primitives::{BuildVolume as RasterBuildVolume, Mirror, PixelResolution};
use sc_sl1cer::contour::ContourLayer;
use sc_sl1cer::raster::pixel_map::PixelMap;
use sc_sl1cer::raster::rasterize_layer_binary;

use crate::build_volume::BuildVolume;

/// The longest build-area side maps to at most this many texels. A cap
/// is a preview a few hundred screen pixels wide, so a modest bound
/// keeps the per-layer rasterise cheap and inline.
const MAX_CAP_SIDE_PX: u32 = 1024;

/// A rasterised layer cap: single-channel coverage (`0` or `255`) in
/// row-major, bottom-to-top order (row 0 is the minimum-Y edge), sized
/// `width` by `height`, ready to upload as an R8 texture.
pub(crate) struct CapRaster {
    pub coverage: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Rasterise `layer` to a coverage bitmap spanning the build area. The
/// resolution scales with the build dimensions so texels stay roughly
/// square. Returns `None` if the build area is degenerate.
pub(crate) fn build_cap_raster(
    layer: &ContourLayer,
    build_volume: &BuildVolume,
) -> Option<CapRaster> {
    let env_min = build_volume.envelope_min_mm();
    let env_max = build_volume.envelope_max_mm();
    let x_mm = f64::from(env_max.x - env_min.x);
    let y_mm = f64::from(env_max.y - env_min.y);
    let longest = x_mm.max(y_mm);
    if longest <= 0.0 {
        return None;
    }

    let mm_per_px = longest / f64::from(MAX_CAP_SIDE_PX);
    let res_x = ((x_mm / mm_per_px).round() as u32).max(1);
    let res_y = ((y_mm / mm_per_px).round() as u32).max(1);

    let resolution = PixelResolution::new(res_x, res_y).ok()?;
    let raster_volume = RasterBuildVolume::new(x_mm, y_mm, 1.0).ok()?;
    // Mirror is a printer-output flip; the on-screen cap must match the
    // un-mirrored world mesh, so it is never applied here.
    let pixel_map = PixelMap::new(raster_volume, resolution, Mirror { x: false, y: false });

    // The world contour is centred on the build origin; shift it into
    // the corner-origin raster frame before filling.
    let shifted = layer.translated_xy(-f64::from(env_min.x), -f64::from(env_min.y));
    let raster = rasterize_layer_binary(&shifted, &pixel_map);

    let mut coverage = Vec::with_capacity((res_x as usize) * (res_y as usize));
    for run in raster.runs() {
        coverage.extend(std::iter::repeat_n(run.value, run.length as usize));
    }

    Some(CapRaster {
        coverage,
        width: res_x,
        height: res_y,
    })
}

#[cfg(test)]
mod slice_cap_tests {
    use super::*;
    use sc_sl1cer::contour::{ClosedRing, Polygon};
    use sc_sl1cer::geom2d::Point2D;

    fn square_layer(half_mm: f64) -> ContourLayer {
        let outer = ClosedRing::new(vec![
            Point2D::new(-half_mm, -half_mm),
            Point2D::new(half_mm, -half_mm),
            Point2D::new(half_mm, half_mm),
            Point2D::new(-half_mm, half_mm),
        ]);
        let polygon = Polygon::new(outer, vec![]).expect("a CCW square is a valid polygon");
        ContourLayer::new(0.05, vec![polygon])
    }

    #[test]
    fn cap_raster_is_sized_and_covered_for_a_centred_square() {
        let bv = BuildVolume::new(80.0, 80.0, 80.0);
        let raster = build_cap_raster(&square_layer(20.0), &bv).expect("a cap raster");
        assert_eq!(raster.width, MAX_CAP_SIDE_PX);
        assert_eq!(raster.height, MAX_CAP_SIDE_PX);
        assert_eq!(
            raster.coverage.len(),
            (raster.width as usize) * (raster.height as usize)
        );
        // The build centre lies inside the square, its corners outside.
        let centre =
            (raster.height as usize / 2) * raster.width as usize + raster.width as usize / 2;
        assert_eq!(raster.coverage[centre], 255);
        assert_eq!(raster.coverage[0], 0);
    }

    #[test]
    fn an_empty_layer_yields_all_zero_coverage() {
        let bv = BuildVolume::new(80.0, 80.0, 80.0);
        let raster = build_cap_raster(&ContourLayer::new(0.05, vec![]), &bv).expect("a cap raster");
        assert!(raster.coverage.iter().all(|&c| c == 0));
    }
}
