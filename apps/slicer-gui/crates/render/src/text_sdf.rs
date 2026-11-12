/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The writing on the plate as distance fields (#545): the front label, the
//! rulers' numbers and the plates' names keep sharp edges however close the
//! camera comes, where a raster of coverage blurs into its texels.
//!
//! Each texel holds how far its centre is from the nearest edge of the ink,
//! inside positive, and the label shader draws the edge where that crosses
//! zero, as wide as one pixel of the screen. A glyph is drawn by swash at
//! [`SUPERSAMPLE`] times the size asked for, its inside and outside measured
//! by an exact Euclidean distance transform (Felzenszwalb and Huttenlocher,
//! "Distance Transforms of Sampled Functions", 2012), the distances refined
//! on the edge by the drawn coverage, and sampled back at the texels asked
//! for. Each glyph is measured once per size and kept.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use swash::FontRef;
use swash::scale::{Render, ScaleContext, Source};

/// How far, in texels, the field reaches either side of an edge; past it a
/// texel reads as wholly inside or wholly outside. The label shader decodes
/// with the same number.
pub(crate) const SPREAD: f32 = 6.0;

/// The texels a line of writing keeps around its ink, so its edges fade out
/// within its own part of a raster.
pub(crate) const PAD: u32 = 7;

/// How many times finer than the texels a glyph is drawn to be measured.
const SUPERSAMPLE: usize = 4;

/// The places a glyph can start at within a texel, in steps of a quarter.
const PHASES: usize = 4;

/// Past this many glyphs kept, the store starts over.
const MAX_KEPT: usize = 512;

/// One glyph as a distance field, for one place within a texel.
pub(crate) struct GlyphField {
    /// The field's bottom-left texel, from the pen's texel and the baseline.
    left: i32,
    bottom: i32,
    width: usize,
    height: usize,
    /// Signed distances in texels, inside positive, rows bottom-to-top.
    distances: Vec<f32>,
    /// The ink, `[min_x, min_y, max_x, max_y]` in texels from the pen's
    /// texel and the baseline.
    pub(crate) ink: [f32; 4],
}

type Measured = Option<Arc<[GlyphField; PHASES]>>;
/// The glyphs measured: by font, glyph and size.
type Kept = Mutex<HashMap<(usize, u16, u32), Measured>>;

fn kept() -> &'static Kept {
    static KEPT: OnceLock<Kept> = OnceLock::new();
    KEPT.get_or_init(Default::default)
}

/// Glyph `id` of `font` at `px` per em, its pen at `pen_x` texels along the
/// line: the texel its pen is counted from, and its field. `None` for a glyph
/// with no ink, such as a space.
pub(crate) fn place(
    font: &'static [u8],
    id: u16,
    px: f32,
    pen_x: f32,
) -> Option<(i32, Arc<[GlyphField; PHASES]>, usize)> {
    let key = (font.as_ptr() as usize, id, px.to_bits());
    let measured = {
        let found = kept().lock().ok().and_then(|kept| kept.get(&key).cloned());
        match found {
            Some(measured) => measured,
            None => {
                let measured = measure(font, id, px).map(Arc::new);
                if let Ok(mut kept) = kept().lock() {
                    if kept.len() >= MAX_KEPT {
                        kept.clear();
                    }
                    kept.insert(key, measured.clone());
                }
                measured
            }
        }
    }?;
    let steps = (pen_x * PHASES as f32).round() as i32;
    let texel = steps.div_euclid(PHASES as i32);
    let phase = steps.rem_euclid(PHASES as i32) as usize;
    Some((texel, measured, phase))
}

/// Draws glyph `id` finely and measures it, once for each place within a
/// texel it can start at.
fn measure(font: &[u8], id: u16, px: f32) -> Option<[GlyphField; PHASES]> {
    let font = FontRef::from_index(font, 0)?;
    let mut context = ScaleContext::new();
    let fine = SUPERSAMPLE as f32;
    let mut scaler = context.builder(font).size(px * fine).hint(false).build();
    let image = Render::new(&[Source::Outline]).render(&mut scaler, id)?;
    let place = image.placement;
    if place.width == 0 || place.height == 0 {
        return None;
    }
    let (gw, gh) = (place.width as usize, place.height as usize);
    // Room around the ink for the whole spread, in fine pixels.
    let margin = ((SPREAD + 1.0) * fine).ceil() as usize;
    let (w, h) = (gw + 2 * margin, gh + 2 * margin);
    let mut coverage = vec![0u8; w * h];
    for row in 0..gh {
        coverage[(row + margin) * w + margin..(row + margin) * w + margin + gw]
            .copy_from_slice(&image.data[row * gw..(row + 1) * gw]);
    }
    let fine_field = signed_field(&coverage, w, h);

    // The fine field's corner, in fine pixels from the pen, y up.
    let field_left = place.left - margin as i32;
    let field_top = place.top + margin as i32;
    let fields = std::array::from_fn(|phase| {
        let shift = phase as f32 / PHASES as f32;
        let left = (field_left as f32 / fine + shift).floor() as i32;
        let right = ((field_left + w as i32) as f32 / fine + shift).ceil() as i32;
        let bottom = ((field_top - h as i32) as f32 / fine).floor() as i32;
        let top = (field_top as f32 / fine).ceil() as i32;
        let (width, height) = ((right - left) as usize, (top - bottom) as usize);
        let mut distances = Vec::with_capacity(width * height);
        for ty in 0..height {
            for tx in 0..width {
                // The texel's centre in fine pixels of the field, rows
                // top-down.
                let x = ((left + tx as i32) as f32 + 0.5 - shift) * fine - field_left as f32 - 0.5;
                let y = field_top as f32 - ((bottom + ty as i32) as f32 + 0.5) * fine - 0.5;
                distances.push(bilinear(&fine_field, w, h, x, y) / fine);
            }
        }
        GlyphField {
            left,
            bottom,
            width,
            height,
            distances,
            ink: [
                place.left as f32 / fine + shift,
                (place.top - place.height as i32) as f32 / fine,
                (place.left + place.width as i32) as f32 / fine + shift,
                place.top as f32 / fine,
            ],
        }
    });
    Some(fields)
}

/// The signed distance of every pixel's centre from the edge of the ink
/// `coverage` draws, in pixels, inside positive: where the coverage crosses
/// one half. Near the edge, from the contour that crossing traces between
/// the pixels' centres (marching squares), which follows the edge within the
/// pixel; further off, by the exact Euclidean distance transform of the
/// pixels in and out.
fn signed_field(coverage: &[u8], w: usize, h: usize) -> Vec<f32> {
    let a: Vec<f32> = coverage.iter().map(|&c| c as f32 / 255.0 - 0.5).collect();
    let inside: Vec<bool> = a.iter().map(|&v| v >= 0.0).collect();
    let to_inside = squared_distances(&inside, w, h, true);
    let to_outside = squared_distances(&inside, w, h, false);
    let mut field: Vec<f32> = (0..w * h)
        .map(|i| {
            if inside[i] {
                to_outside[i].sqrt() as f32 - 0.5
            } else {
                0.5 - to_inside[i].sqrt() as f32
            }
        })
        .collect();
    let contour = contour(&a, w, h);
    // How far from the edge, in pixels, the contour is read.
    const NEAR: usize = 3;
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if field[i].abs() > NEAR as f32 {
                continue;
            }
            let mut nearest = f32::MAX;
            for cy in y.saturating_sub(NEAR)..(y + NEAR).min(h - 1) {
                for cx in x.saturating_sub(NEAR)..(x + NEAR).min(w - 1) {
                    for segment in &contour[cy * (w - 1) + cx] {
                        nearest = nearest.min(to_segment(x as f32, y as f32, *segment));
                    }
                }
            }
            if nearest < f32::MAX {
                field[i] = if inside[i] { nearest } else { -nearest };
            }
        }
    }
    field
}

/// The contour where `a` crosses zero, as segments between the centres of
/// the pixels: those in each cell of four centres, cell `x, y` the one from
/// pixel `x, y` to pixel `x + 1, y + 1`.
fn contour(a: &[f32], w: usize, h: usize) -> Vec<Vec<[f32; 4]>> {
    let mut cells = vec![Vec::new(); (w - 1) * (h - 1)];
    for y in 0..h - 1 {
        for x in 0..w - 1 {
            // The corners in turn round the cell, and each side's crossing.
            let corners = [(x, y), (x + 1, y), (x + 1, y + 1), (x, y + 1)];
            let value = |(cx, cy): (usize, usize)| a[cy * w + cx];
            let mut crossings = [None; 4];
            for side in 0..4 {
                let (p, q) = (corners[side], corners[(side + 1) % 4]);
                let (vp, vq) = (value(p), value(q));
                if (vp >= 0.0) != (vq >= 0.0) {
                    let t = vp / (vp - vq);
                    crossings[side] = Some([
                        p.0 as f32 + t * (q.0 as f32 - p.0 as f32),
                        p.1 as f32 + t * (q.1 as f32 - p.1 as f32),
                    ]);
                }
            }
            let found: Vec<[f32; 2]> = crossings.iter().flatten().copied().collect();
            let cell = &mut cells[y * (w - 1) + x];
            match found.len() {
                2 => cell.push([found[0][0], found[0][1], found[1][0], found[1][1]]),
                4 => {
                    // A saddle: the centre's side says which corners the
                    // contour cuts off.
                    let centre = corners.iter().map(|&c| value(c)).sum::<f32>() >= 0.0;
                    let [s0, s1, s2, s3] = crossings.map(|c| c.unwrap_or_default());
                    let pairs = if centre == (value(corners[0]) >= 0.0) {
                        [(s0, s1), (s2, s3)]
                    } else {
                        [(s3, s0), (s1, s2)]
                    };
                    for (p, q) in pairs {
                        cell.push([p[0], p[1], q[0], q[1]]);
                    }
                }
                _ => {}
            }
        }
    }
    cells
}

/// How far `x`, `y` is from the segment `[x0, y0, x1, y1]`.
fn to_segment(x: f32, y: f32, [x0, y0, x1, y1]: [f32; 4]) -> f32 {
    let (dx, dy) = (x1 - x0, y1 - y0);
    let length = dx * dx + dy * dy;
    let t = if length > 0.0 {
        (((x - x0) * dx + (y - y0) * dy) / length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (x - x0 - t * dx).hypot(y - y0 - t * dy)
}

const FAR: f64 = 1e20;

/// The squared distance of every pixel to the nearest one whose `inside` is
/// `to`.
fn squared_distances(inside: &[bool], w: usize, h: usize, to: bool) -> Vec<f64> {
    let mut grid: Vec<f64> = inside
        .iter()
        .map(|&i| if i == to { 0.0 } else { FAR })
        .collect();
    let n = w.max(h);
    let (mut f, mut d) = (vec![0.0; n], vec![0.0; n]);
    let (mut v, mut z) = (vec![0usize; n], vec![0.0; n + 1]);
    for x in 0..w {
        for y in 0..h {
            f[y] = grid[y * w + x];
        }
        transform(&f[..h], &mut d[..h], &mut v, &mut z);
        for y in 0..h {
            grid[y * w + x] = d[y];
        }
    }
    for y in 0..h {
        f[..w].copy_from_slice(&grid[y * w..(y + 1) * w]);
        transform(&f[..w], &mut d[..w], &mut v, &mut z);
        grid[y * w..(y + 1) * w].copy_from_slice(&d[..w]);
    }
    grid
}

/// The one-dimensional squared distance transform of `f` into `d`: the
/// lower envelope of the parabolas rooted at each sample.
fn transform(f: &[f64], d: &mut [f64], v: &mut [usize], z: &mut [f64]) {
    let n = f.len();
    if n == 0 {
        return;
    }
    let mut k = 0;
    v[0] = 0;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    for q in 1..n {
        let mut s = parabolas_meet(f, v[k], q);
        while s <= z[k] {
            k -= 1;
            s = parabolas_meet(f, v[k], q);
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f64::INFINITY;
    }
    k = 0;
    for (q, out) in d.iter_mut().enumerate() {
        let qf = q as f64;
        while z[k + 1] < qf {
            k += 1;
        }
        let p = v[k] as f64;
        *out = (qf - p) * (qf - p) + f[v[k]];
    }
}

/// Where the parabolas rooted at samples `p` and `q` of `f` cross.
fn parabolas_meet(f: &[f64], p: usize, q: usize) -> f64 {
    let (pf, qf) = (p as f64, q as f64);
    ((f[q] + qf * qf) - (f[p] + pf * pf)) / (2.0 * (qf - pf))
}

/// `field` read between its pixels at `x`, `y`, held at its edge.
fn bilinear(field: &[f32], w: usize, h: usize, x: f32, y: f32) -> f32 {
    let x = x.clamp(0.0, (w - 1) as f32);
    let y = y.clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let at = |x: usize, y: usize| field[y * w + x];
    let top = at(x0, y0) * (1.0 - fx) + at(x1, y0) * fx;
    let bottom = at(x0, y1) * (1.0 - fx) + at(x1, y1) * fx;
    top * (1.0 - fy) + bottom * fy
}

/// A distance field being drawn into: what lies on it is the union of what
/// is drawn, rows bottom-to-top.
pub(crate) struct Canvas {
    pub(crate) width: u32,
    pub(crate) height: u32,
    distances: Vec<f32>,
}

impl Canvas {
    /// A canvas with nothing on it.
    pub(crate) fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            distances: vec![-SPREAD; (width * height) as usize],
        }
    }

    /// Adds `glyph`'s field, in place `phase` within its texel, its pen at
    /// texel `x` and its baseline at row `y`.
    pub(crate) fn glyph(&mut self, fields: &[GlyphField; PHASES], phase: usize, x: i32, y: i32) {
        let field = &fields[phase];
        for row in 0..field.height {
            let cy = y + field.bottom + row as i32;
            if cy < 0 || cy >= self.height as i32 {
                continue;
            }
            for col in 0..field.width {
                let cx = x + field.left + col as i32;
                if cx < 0 || cx >= self.width as i32 {
                    continue;
                }
                let at = (cy as u32 * self.width + cx as u32) as usize;
                let d = field.distances[row * field.width + col];
                if d > self.distances[at] {
                    self.distances[at] = d;
                }
            }
        }
    }

    /// Adds the rectangle `[min_x, min_y, max_x, max_y]`, in texels.
    pub(crate) fn rect(&mut self, rect: [f32; 4]) {
        self.shape(|x, y| rect_distance(rect, x, y), rect);
    }

    /// Adds a frame `thickness` texels wide inside the rectangle `outer`.
    pub(crate) fn frame(&mut self, outer: [f32; 4], thickness: f32) {
        let [x0, y0, x1, y1] = outer;
        let inner = [
            x0 + thickness,
            y0 + thickness,
            x1 - thickness,
            y1 - thickness,
        ];
        self.shape(
            |x, y| rect_distance(outer, x, y).min(-rect_distance(inner, x, y)),
            outer,
        );
    }

    /// Adds the shape `distance` measures, lying within `bounds`.
    fn shape(&mut self, distance: impl Fn(f32, f32) -> f32, bounds: [f32; 4]) {
        let [x0, y0, x1, y1] = bounds;
        let span = |from: f32, to: f32, size: u32| {
            let first = (from - SPREAD).floor().max(0.0) as u32;
            let last = ((to + SPREAD).ceil().max(0.0) as u32).min(size);
            first..last
        };
        let (cols, rows) = (span(x0, x1, self.width), span(y0, y1, self.height));
        for y in rows {
            for x in cols.clone() {
                let d = distance(x as f32 + 0.5, y as f32 + 0.5);
                let at = (y * self.width + x) as usize;
                if d > self.distances[at] {
                    self.distances[at] = d;
                }
            }
        }
    }

    /// The texel at `x`, `y` as a byte: 128 on an edge, 255 a whole spread
    /// inside, 0 a whole spread outside.
    pub(crate) fn byte(&self, x: u32, y: u32) -> u8 {
        encode(self.distances[(y * self.width + x) as usize])
    }

    /// Writes the canvas into channel `channel` of `out`, a raster `channels`
    /// bytes a texel and `out_width` texels wide, from texel `at`, keeping
    /// whichever is further in where they overlap.
    pub(crate) fn write(
        &self,
        out: &mut [u8],
        out_width: u32,
        channels: u32,
        channel: u32,
        at: (u32, u32),
    ) {
        for y in 0..self.height {
            for x in 0..self.width {
                let texel = ((at.1 + y) * out_width + at.0 + x) * channels + channel;
                let byte = &mut out[texel as usize];
                *byte = (*byte).max(self.byte(x, y));
            }
        }
    }
}

/// How far `x`, `y` is inside the rectangle, negative outside.
fn rect_distance([x0, y0, x1, y1]: [f32; 4], x: f32, y: f32) -> f32 {
    let dx = (x0 - x).max(x - x1);
    let dy = (y0 - y).max(y - y1);
    if dx <= 0.0 && dy <= 0.0 {
        -dx.max(dy)
    } else {
        -(dx.max(0.0).hypot(dy.max(0.0)))
    }
}

/// A distance as the byte the label shader decodes.
fn encode(d: f32) -> u8 {
    ((0.5 + d / (2.0 * SPREAD)).clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    static FONT: &[u8] = include_bytes!("../../../assets/fonts/NotoSans-Variable.ttf");

    #[test]
    fn the_transform_measures_true_distances() {
        // One feature in the middle of a 9 x 9 grid.
        let mut inside = vec![false; 81];
        inside[40] = true;
        let d = squared_distances(&inside, 9, 9, true);
        assert_eq!(d[40], 0.0);
        assert_eq!(d[0], 32.0, "4² + 4² to the corner");
        assert_eq!(d[44], 16.0);
        assert_eq!(d[13], 9.0 + 0.0, "three rows up, same column");
    }

    #[test]
    fn a_rectangle_is_inside_positive_and_outside_negative() {
        let r = [2.0, 2.0, 6.0, 4.0];
        assert!((rect_distance(r, 4.0, 3.0) - 1.0).abs() < 1e-6);
        assert!((rect_distance(r, 8.0, 3.0) + 2.0).abs() < 1e-6);
        assert!((rect_distance(r, 9.0, 8.0) + 5.0).abs() < 1e-6, "3, 4, 5");
    }

    #[test]
    fn an_edge_reads_halfway_and_the_spread_reaches_both_ends() {
        assert_eq!(encode(0.0), 128);
        assert_eq!(encode(SPREAD), 255);
        assert_eq!(encode(-SPREAD), 0);
    }

    #[test]
    fn a_glyphs_edge_lies_where_its_drawn_ink_ends() {
        // An "l" is a bar: its field reads inside across the stem and
        // outside past it, the edge within a tenth of a texel of where a
        // straight drawing at the same size puts it.
        let font = FontRef::from_index(FONT, 0).unwrap();
        let id = font.charmap().map('l');
        let px = 60.0;
        let (texel, fields, phase) = place(FONT, id, px, 0.0).expect("an l has ink");
        assert_eq!((texel, phase), (0, 0));
        let field = &fields[0];
        let mut context = ScaleContext::new();
        let mut scaler = context.builder(font).size(px).hint(false).build();
        let plain = Render::new(&[Source::Outline])
            .render(&mut scaler, id)
            .unwrap();
        // Half way up the stem, the plain drawing's covered width.
        let row = plain.placement.height as usize / 2;
        let covered: f32 = (0..plain.placement.width as usize)
            .map(|c| plain.data[row * plain.placement.width as usize + c] as f32 / 255.0)
            .sum();
        // The field's width there: where it crosses zero either side.
        let fy = (plain.placement.top - row as i32) - 1 - field.bottom;
        let line: Vec<f32> = (0..field.width)
            .map(|c| field.distances[fy as usize * field.width + c])
            .collect();
        let inside_from = line.iter().position(|&d| d > 0.0).unwrap();
        let inside_to = line.iter().rposition(|&d| d > 0.0).unwrap();
        let cross = |a: usize, b: usize| {
            // Where the line between two texels crosses zero.
            a as f32 + line[a] / (line[a] - line[b]) * (b as f32 - a as f32)
        };
        let width = cross(inside_to, inside_to + 1) - cross(inside_from - 1, inside_from);
        assert!(
            (width - covered).abs() < 0.05,
            "field {width:.3} texels wide, drawing {covered:.3}"
        );
    }
}
