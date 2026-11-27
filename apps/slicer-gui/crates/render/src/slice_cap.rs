/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The selected layer's cap, in two forms.
//!
//! The 3D cut is closed by [`CapShape`]: the layer's polygons as
//! triangles, so its edges are the contours' own at any zoom and meet the
//! cut walls of the mesh (#776). The slice-view panel shows [`CapRaster`]:
//! the layer rasterised at a modest display resolution (the longest
//! build-area side maps to at most [`MAX_CAP_SIDE_PX`] texels). The
//! contours stay in the printer frame (XY centred on the build origin).
//!
//! A review finding's pixels on the layer (#409) can be highlighted: the
//! cut draws its rectangles over the cap, and [`highlight_cap`] marks every
//! texel they reach in the panel's raster, both in [`HIGHLIGHT_RGB`].

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
/// order, `0` or `255` (empty for none). The slice-view panel shows it.
pub struct CapRaster {
    pub coverage: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub highlight: Vec<u8>,
}

impl CapRaster {
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

/// The cap that closes the 3D cut (#776): triangles in plate-0 world
/// millimetres, three vertices each, `[x, y, highlight]` with `highlight`
/// `0` for the layer and `1` for a review finding's pixels (#409), drawn
/// over it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CapShape {
    pub vertices: Vec<[f32; 3]>,
}

impl CapShape {
    /// How many vertices the cap draws.
    pub fn len(&self) -> usize {
        self.vertices.len()
    }

    /// Whether the cap draws nothing: an empty layer.
    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }
}

/// The layer's polygons as triangles, each polygon with its holes, then the
/// highlighted finding's `rects` (`[x0, y0, x1, y1]` in printer-frame mm,
/// centred on the plate, as the contours) over them. The polygons are
/// triangulated in parallel; a polygon earcut cannot read draws nothing.
pub fn build_cap_shape(
    layer: &ContourLayer,
    rects: &[[f64; 4]],
    build_volume: &BuildVolume,
) -> CapShape {
    use rayon::prelude::*;

    let centre = (build_volume.envelope_min_mm() + build_volume.envelope_max_mm()) * 0.5;
    let (cx, cy) = (f64::from(centre.x), f64::from(centre.y));
    let parts: Vec<Vec<[f32; 3]>> = layer
        .polygons()
        .par_iter()
        .map(|polygon| {
            let rings: Vec<Vec<[f64; 2]>> = std::iter::once(polygon.outer())
                .chain(polygon.holes())
                .map(|ring| {
                    let mut points: Vec<[f64; 2]> =
                        ring.vertices().iter().map(|p| [p.x, p.y]).collect();
                    // A ring written closed repeats its first point.
                    if points.len() > 1 && points.first() == points.last() {
                        points.pop();
                    }
                    points
                })
                .collect();
            let mut out = Vec::new();
            if rings.len() > STRIP_HOLES {
                for strip in strips_of(&rings) {
                    triangulate(&strip, [cx, cy], &mut out);
                }
            } else {
                triangulate(&rings, [cx, cy], &mut out);
            }
            out
        })
        .collect();
    let mut vertices: Vec<[f32; 3]> = Vec::with_capacity(parts.iter().map(Vec::len).sum());
    for part in parts {
        vertices.extend(part);
    }
    for &[x0, y0, x1, y1] in rects {
        let (x0, y0, x1, y1) = (
            (x0 + cx) as f32,
            (y0 + cy) as f32,
            (x1 + cx) as f32,
            (y1 + cy) as f32,
        );
        vertices.extend([
            [x0, y0, 1.0],
            [x1, y0, 1.0],
            [x1, y1, 1.0],
            [x0, y0, 1.0],
            [x1, y1, 1.0],
            [x0, y1, 1.0],
        ]);
    }
    CapShape { vertices }
}

/// Past this many holes a polygon is cut into strips across X before it is
/// triangulated: earcut bridges each hole to the outline by a search along
/// the whole outline, so a line raft's hundreds of holes would cost it a
/// quarter of a second (#776).
const STRIP_HOLES: usize = 32;

/// Triangulates one polygon, `rings[0]` its outline and the rest its holes,
/// into `out`, moved by `centre` onto the plate. A ring of fewer than three
/// points is left out; an outline earcut cannot read draws nothing.
fn triangulate(rings: &[Vec<[f64; 2]>], centre: [f64; 2], out: &mut Vec<[f32; 3]>) {
    if rings.first().is_none_or(|outer| outer.len() < 3) {
        return;
    }
    let mut coords = Vec::new();
    let mut holes = Vec::new();
    for (k, ring) in rings.iter().enumerate() {
        if ring.len() < 3 {
            continue;
        }
        if k > 0 {
            holes.push(coords.len() / 2);
        }
        for p in ring {
            coords.extend_from_slice(p);
        }
    }
    let triangles = earcutr::earcut(&coords, &holes, 2).unwrap_or_default();
    out.extend(triangles.iter().map(|&i| {
        [
            (coords[2 * i] + centre[0]) as f32,
            (coords[2 * i + 1] + centre[1]) as f32,
            0.0,
        ]
    }));
}

/// A polygon with many holes cut into strips across X, about
/// [`STRIP_HOLES`] holes each: every strip is the polygon clipped to it, as
/// outlines with their holes. Each clip reads only the rings that reach its
/// strip.
fn strips_of(rings: &[Vec<[f64; 2]>]) -> Vec<Vec<Vec<[f64; 2]>>> {
    use i_overlay::core::fill_rule::FillRule;
    use i_overlay::core::overlay_rule::OverlayRule;
    use i_overlay::float::overlay::FloatOverlay;
    use rayon::prelude::*;

    let span = |ring: &Vec<[f64; 2]>| {
        ring.iter().fold([f64::MAX, f64::MIN], |[lo, hi], p| {
            [lo.min(p[0]), hi.max(p[0])]
        })
    };
    let spans: Vec<[f64; 2]> = rings.iter().map(span).collect();
    let [x0, x1] = spans[0];
    let [y0, y1] = rings[0].iter().fold([f64::MAX, f64::MIN], |[lo, hi], p| {
        [lo.min(p[1]), hi.max(p[1])]
    });
    let count = rings.len().div_ceil(STRIP_HOLES).clamp(2, 256);
    let width = (x1 - x0) / count as f64;
    (0..count)
        .into_par_iter()
        .flat_map_iter(|k| {
            let (a, b) = (x0 + width * k as f64, x0 + width * (k + 1) as f64);
            // The last strip ends past the outline, so nothing on its edge
            // is lost.
            let b = if k + 1 == count { x1 + width } else { b };
            let a = if k == 0 { x0 - width } else { a };
            let subject: Vec<Vec<[f64; 2]>> = rings
                .iter()
                .zip(&spans)
                .enumerate()
                .filter(|&(i, (_, s))| i == 0 || (s[1] >= a && s[0] <= b))
                .map(|(_, (ring, _))| ring.clone())
                .collect();
            let (ylo, yhi) = (y0 - 1.0, y1 + 1.0);
            let clip = vec![vec![[a, ylo], [b, ylo], [b, yhi], [a, yhi]]];
            FloatOverlay::with_subj_and_clip(&subject, &clip)
                .overlay(OverlayRule::Intersect, FillRule::NonZero)
        })
        .collect()
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
    }

    fn ring(points: &[(f64, f64)]) -> ClosedRing {
        ClosedRing::new(points.iter().map(|&(x, y)| Point2D::new(x, y)).collect())
    }

    fn area(vertices: &[[f32; 3]]) -> f64 {
        vertices
            .chunks(3)
            .map(|t| {
                let (a, b, c) = (t[0], t[1], t[2]);
                0.5 * f64::from((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])).abs()
            })
            .sum()
    }

    /// #776: the cap is the layer's polygons themselves, holes left open,
    /// every corner on a contour point (a round pin stays round at any zoom),
    /// set on the plate where the contours' frame is centred.
    #[test]
    fn the_cap_is_the_polygons_with_their_holes_on_the_plate() {
        // A plate from 0 to 100 in X and 0 to 60 in Y: its centre is (50, 30).
        let bv = BuildVolume::new(100.0, 60.0, 80.0);
        let centre = (bv.envelope_min_mm() + bv.envelope_max_mm()) * 0.5;
        let square = Polygon::new(
            ring(&[(-10.0, -10.0), (10.0, -10.0), (10.0, 10.0), (-10.0, 10.0)]),
            vec![ring(&[(-2.0, -2.0), (-2.0, 2.0), (2.0, 2.0), (2.0, -2.0)])],
        )
        .expect("a square with a hole");
        let n = 48;
        let pin: Vec<(f64, f64)> = (0..n)
            .map(|k| {
                let a = std::f64::consts::TAU * f64::from(k) / f64::from(n);
                (20.0 + 0.25 * a.cos(), 0.25 * a.sin())
            })
            .collect();
        let pin = Polygon::new(ring(&pin), vec![]).expect("a round pin");
        let layer = ContourLayer::new(0.05, vec![square, pin]);

        let shape = build_cap_shape(&layer, &[], &bv);
        assert_eq!(shape.len() % 3, 0);
        let pin_area =
            0.5 * f64::from(n) * 0.25 * 0.25 * (std::f64::consts::TAU / f64::from(n)).sin();
        assert!((area(&shape.vertices) - (400.0 - 16.0 + pin_area)).abs() < 1e-3);
        assert!(shape.vertices.iter().all(|v| v[2] == 0.0));
        // Every corner is a contour point, moved onto the plate.
        let on_contour = |v: &[f32; 3]| {
            let (x, y) = (f64::from(v[0] - centre.x), f64::from(v[1] - centre.y));
            let on_square =
                [-10.0, 10.0, -2.0, 2.0].contains(&x.round()) && (x - x.round()).abs() < 1e-4;
            let on_pin = ((x - 20.0).hypot(y) - 0.25).abs() < 1e-4;
            on_square || on_pin
        };
        assert!(shape.vertices.iter().all(on_contour));
    }

    /// #409 on the cut: a finding's rectangles come after the layer, marked.
    #[test]
    fn a_findings_rectangles_draw_over_the_layer() {
        let bv = BuildVolume::new(80.0, 80.0, 80.0);
        let layer = square_layer(20.0);
        let plain = build_cap_shape(&layer, &[], &bv);
        let marked = build_cap_shape(&layer, &[[1.0, 2.0, 1.5, 2.25]], &bv);
        assert_eq!(&marked.vertices[..plain.len()], &plain.vertices[..]);
        let rect = &marked.vertices[plain.len()..];
        assert_eq!(rect.len(), 6);
        assert!(rect.iter().all(|v| v[2] == 1.0));
        assert!((area(rect) - 0.125).abs() < 1e-6);
        assert!(build_cap_shape(&ContourLayer::new(0.05, vec![]), &[], &bv).is_empty());
    }

    /// #776: a polygon with more holes than [`STRIP_HOLES`] is cut into
    /// strips, and its cap keeps the same area: no hole filled, nothing lost
    /// on a strip's edge.
    #[test]
    fn a_polygon_of_many_holes_keeps_its_area_in_strips() {
        let bv = BuildVolume::new(223.0, 126.0, 200.0);
        let outer = ring(&[(-50.0, -30.0), (50.0, -30.0), (50.0, 30.0), (-50.0, 30.0)]);
        // A 20 x 10 lattice of 2 x 2 holes, some across where strips meet.
        let holes: Vec<ClosedRing> = (0..200)
            .map(|k| {
                let (x, y) = (
                    -47.0 + 4.7 * f64::from(k % 20),
                    -27.0 + 5.5 * f64::from(k / 20),
                );
                ring(&[(x, y), (x, y + 2.0), (x + 2.0, y + 2.0), (x + 2.0, y)])
            })
            .collect();
        let layer = ContourLayer::new(0.05, vec![Polygon::new(outer, holes).expect("a lattice")]);
        let shape = build_cap_shape(&layer, &[], &bv);
        assert!((area(&shape.vertices) - (6000.0 - 200.0 * 4.0)).abs() < 1e-2);
    }
}
