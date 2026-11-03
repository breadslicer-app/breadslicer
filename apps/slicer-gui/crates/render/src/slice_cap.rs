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
//! contour stays in the printer frame (XY centred on the build
//! origin): that is the frame [`PixelMap`] projects from.
//!
//! A review finding's pixels on the layer (#409) can be highlighted:
//! [`highlight_cap`] marks every texel they reach, and the cut and the
//! slice-view panel draw those texels in [`HIGHLIGHT_RGB`].

use sc_print_primitives::{Mirror, PixelResolution, PositiveFiniteMm};
use sc_sl1cer::contour::ContourLayer;
use sc_sl1cer::raster::pixel_map::PixelMap;
use sc_sl1cer::raster::rasterize_layer_binary;
use slint::{Rgba8Pixel, SharedPixelBuffer};

use crate::build_volume::BuildVolume;

/// The longest build-area side maps to at most this many texels. A cap
/// is a preview a few hundred screen pixels wide, so a modest bound
/// keeps the per-layer rasterise cheap and inline.
const MAX_CAP_SIDE_PX: u32 = 1024;

/// A rasterised layer cap: single-channel coverage (`0` or `255`) in
/// row-major, bottom-to-top order (row 0 is the minimum-Y edge), sized
/// `width` by `height`, and the highlighted texels (#409) in the same
/// order, `0` or `255` (empty for none). Uploaded as an RG8 texture.
pub struct CapRaster {
    pub coverage: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub highlight: Vec<u8>,
}

impl CapRaster {
    /// Coverage and highlight interleaved, texel by texel: the RG8
    /// texture's bytes.
    pub fn texture_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.coverage.len() * 2);
        for (i, &c) in self.coverage.iter().enumerate() {
            out.push(c);
            out.push(self.highlight.get(i).copied().unwrap_or(0));
        }
        out
    }

    /// Whether texel `(x, y)` (bottom-to-top rows) is highlighted.
    pub fn is_highlighted(&self, x: u32, y: u32) -> bool {
        let i = y as usize * self.width as usize + x as usize;
        self.highlight.get(i).is_some_and(|&h| h != 0)
    }
}

/// The highlight of a review finding's pixels (#409), on the cut and in
/// the slice-view panel: a magenta no model, support or warning colour
/// uses.
pub const HIGHLIGHT_RGB: [u8; 3] = [230, 60, 170];

/// Marks every texel of `raster` that one of `rects` reaches (each
/// `[x0, y0, x1, y1]` in printer-frame mm, centred on the build origin, as
/// the layer's contours) as highlighted and covered: a part narrower than
/// a texel still shows as one.
pub fn highlight_cap(raster: &mut CapRaster, rects: &[[f64; 4]], build_volume: &BuildVolume) {
    let env_min = build_volume.envelope_min_mm();
    let env_max = build_volume.envelope_max_mm();
    let (x_mm, y_mm) = (
        f64::from(env_max.x - env_min.x),
        f64::from(env_max.y - env_min.y),
    );
    if raster.width == 0 || raster.height == 0 || x_mm <= 0.0 || y_mm <= 0.0 {
        return;
    }
    let (w, h) = (raster.width as usize, raster.height as usize);
    if raster.highlight.len() != w * h {
        raster.highlight = vec![0; w * h];
    }
    let (sx, sy) = (
        f64::from(raster.width) / x_mm,
        f64::from(raster.height) / y_mm,
    );
    // Texel columns and rows from the minimum corner of the build area.
    let span = |lo: f64, hi: f64, scale: f64, half: f64, count: usize| {
        // A rectangle edge on a texel edge reaches no further.
        let a = ((lo + half) * scale + 1e-6).floor().max(0.0) as usize;
        let b = ((hi + half) * scale - 1e-6).ceil().max(0.0) as usize;
        (
            a.min(count),
            b.min(count).max(a.min(count) + usize::from(a < count)),
        )
    };
    for &[x0, y0, x1, y1] in rects {
        let (c0, c1) = span(x0, x1, sx, x_mm / 2.0, w);
        let (r0, r1) = span(y0, y1, sy, y_mm / 2.0, h);
        for row in r0..r1 {
            for col in c0..c1 {
                let i = row * w + col;
                raster.highlight[i] = 255;
                raster.coverage[i] = 255;
            }
        }
    }
}

// The size only: the coverage runs to millions of bytes.
impl std::fmt::Debug for CapRaster {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CapRaster")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

/// Rasterise `layer` to a coverage bitmap spanning the build area. The
/// resolution scales with the build dimensions so texels stay roughly
/// square. Returns `None` if the build area is degenerate.
pub fn build_cap_raster(layer: &ContourLayer, build_volume: &BuildVolume) -> Option<CapRaster> {
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
    let plate_x = PositiveFiniteMm::new(x_mm).ok()?;
    let plate_y = PositiveFiniteMm::new(y_mm).ok()?;
    // Mirror is a printer-output flip; the on-screen cap must match the
    // un-mirrored world mesh, so it is never applied here.
    let pixel_map = PixelMap::new(plate_x, plate_y, resolution, Mirror { x: false, y: false });

    let raster = rasterize_layer_binary(layer, &pixel_map);

    let mut coverage = Vec::with_capacity((res_x as usize) * (res_y as usize));
    for run in raster.runs() {
        coverage.extend(std::iter::repeat_n(run.value, run.length as usize));
    }

    Some(CapRaster {
        coverage,
        width: res_x,
        height: res_y,
        highlight: Vec::new(),
    })
}

/// RGB fill for covered texels in the slice-view panel: the mesh base
/// tint, matching the 3D cap so the panel and the viewport cut read the
/// same colour. Uncovered texels reuse this RGB with a zero alpha so the
/// panel's bilinear scaling does not fringe covered edges toward a
/// different hue.
const SLICE_VIEW_FILL_RGB: [u8; 3] = [191, 199, 217];

/// Expand a [`CapRaster`] into a straight-alpha RGBA buffer for the
/// slice-view panel. The cap stores coverage bottom-to-top (row 0 is the
/// minimum-Y edge) to match the GPU texture upload; the panel shows the
/// plate from above, so the rows are flipped to top-down here. Covered
/// texels become an opaque fill; uncovered texels become fully
/// transparent so the panel surface shows through.
pub fn cap_raster_to_rgba(raster: &CapRaster) -> SharedPixelBuffer<Rgba8Pixel> {
    let width = raster.width as usize;
    let height = raster.height as usize;
    let [r, g, b] = SLICE_VIEW_FILL_RGB;

    let [hr, hg, hb] = HIGHLIGHT_RGB;
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(raster.width, raster.height);
    let pixels = buffer.make_mut_slice();
    for out_row in 0..height {
        // Flip vertically: output row 0 (top) reads the cap's last row.
        let src_row = height - 1 - out_row;
        for col in 0..width {
            let i = src_row * width + col;
            let covered = raster.coverage[i] != 0;
            let a = if covered { 255 } else { 0 };
            pixels[out_row * width + col] = if raster.highlight.get(i).is_some_and(|&h| h != 0) {
                Rgba8Pixel {
                    r: hr,
                    g: hg,
                    b: hb,
                    a: 255,
                }
            } else {
                Rgba8Pixel { r, g, b, a }
            };
        }
    }
    buffer
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

    #[test]
    fn cap_raster_to_rgba_flips_rows_and_fills_coverage() {
        // Bottom-to-top coverage: row 0 (the minimum-Y edge) is solid,
        // row 1 is empty. The panel views the plate from above, so the
        // expansion flips the rows: output row 0 (the top) comes from the
        // cap's last row and stays transparent; output row 1 (the bottom)
        // is the opaque fill.
        let raster = CapRaster {
            coverage: vec![255, 255, 0, 0],
            width: 2,
            height: 2,
            highlight: Vec::new(),
        };
        let rgba = cap_raster_to_rgba(&raster);
        let pixels = rgba.as_slice();

        assert_eq!(pixels[0].a, 0);
        assert_eq!(pixels[1].a, 0);
        assert_eq!(pixels[2].a, 255);
        assert_eq!(pixels[3].a, 255);
        assert_eq!((pixels[2].r, pixels[2].g, pixels[2].b), (191, 199, 217));
    }

    #[test]
    fn cap_raster_to_rgba_preserves_dimensions() {
        let raster = CapRaster {
            coverage: vec![0; 12],
            width: 4,
            height: 3,
            highlight: Vec::new(),
        };
        let rgba = cap_raster_to_rgba(&raster);
        assert_eq!(rgba.width(), 4);
        assert_eq!(rgba.height(), 3);
    }

    /// #409: a finding's rectangles highlight exactly the texels they
    /// reach, covered, and the panel draws them in the highlight colour.
    #[test]
    fn a_highlight_marks_exactly_the_texels_its_rectangles_reach() {
        // 80 x 80 mm at 1024 texels: 0.078125 mm a texel.
        let bv = BuildVolume::new(80.0, 80.0, 80.0);
        let mut raster = build_cap_raster(&square_layer(20.0), &bv).expect("a cap raster");
        let texel = 80.0 / 1024.0;
        // Texels 520..522 across and 600..601 up, exactly, and a speck
        // narrower than a texel inside texel (100, 100).
        let rect = [
            520.0 * texel - 40.0,
            600.0 * texel - 40.0,
            522.0 * texel - 40.0,
            601.0 * texel - 40.0,
        ];
        let speck = [
            100.2 * texel - 40.0,
            100.3 * texel - 40.0,
            100.4 * texel - 40.0,
            100.5 * texel - 40.0,
        ];
        highlight_cap(&mut raster, &[rect, speck], &bv);
        let marked: Vec<(u32, u32)> = (0..raster.height)
            .flat_map(|y| (0..raster.width).map(move |x| (x, y)))
            .filter(|&(x, y)| raster.is_highlighted(x, y))
            .collect();
        assert_eq!(marked, [(100, 100), (520, 600), (521, 600)]);
        // A speck outside the cut still shows: it is covered.
        assert_eq!(raster.coverage[100 * 1024 + 100], 255);
        let rgba = cap_raster_to_rgba(&raster);
        // The panel flips rows: texel (520, 600) is row 1023 - 600 there.
        let p = rgba.as_slice()[(1023 - 600) * 1024 + 520];
        assert_eq!([p.r, p.g, p.b, p.a], [230, 60, 170, 255]);
        let bytes = raster.texture_bytes();
        assert_eq!(bytes.len(), 2 * 1024 * 1024);
        assert_eq!(&bytes[2 * (600 * 1024 + 520)..][..2], &[255, 255]);
    }
}
