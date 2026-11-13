/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! One support drawn on its own for the Supports tab (#564, #567): the size
//! a preset places, built from the numbers the engine reads (tip, cone,
//! break point, trunk, join cone, foot) the way the engine draws them. The
//! drawings are small enough to make on the UI thread at every change, so a
//! value typed shows at once.
//!
//! There are four: the support in 3D, turned by the person, standing on a
//! 1 mm grid; the same support as a plan, cut down its axis at its real
//! scale with the trunk broken; and two lenses, the tip and the foot close
//! up, for what is too small to see in the whole (how far the tip sinks,
//! the break point, the join cone, the foot's thickness).
//!
//! All are drawn at twice the size and halved, so their edges are smooth.
//! Their texts are left to the panel, which lays out text better: each
//! drawing hands back where its numbers and its words go.

use sc_geom::df_supports::SupportSize;
use slint::{Rgba8Pixel, SharedPixelBuffer};

/// A part of the support, as the panel names them: what lights up when a
/// field of it is pointed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Part {
    None = 0,
    Tip = 1,
    Trunk = 2,
    Foot = 3,
}

impl Part {
    pub(crate) fn from_code(code: i32) -> Part {
        match code {
            1 => Part::Tip,
            2 => Part::Trunk,
            3 => Part::Foot,
            _ => Part::None,
        }
    }
}

/// Where the drawing is seen from: its turn about the support's axis and
/// how far above it the eye stands, in radians.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct View {
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
}

impl Default for View {
    fn default() -> Self {
        View {
            yaw: 0.6,
            pitch: 0.42,
        }
    }
}

impl View {
    /// The view turned by a drag of `dx`, `dy` logical pixels.
    pub(crate) fn dragged(self, dx: f32, dy: f32) -> View {
        View {
            yaw: self.yaw + dx * 0.012,
            pitch: (self.pitch + dy * 0.008).clamp(-0.2, 1.3),
        }
    }
}

/// Where a text of the drawing stands about its point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Anchor {
    Start = 0,
    End = 1,
    Middle = 2,
}

/// The words a drawing carries, which the panel writes in the person's
/// language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Word {
    None = 0,
    Model = 1,
    Plate = 2,
    Tip = 3,
    FootHalf = 4,
    Sinks = 5,
    Break = 6,
    Cone = 7,
    Thick = 8,
    /// With the number the size ships with.
    Ships = 9,
    OneMm = 10,
    Grid = 11,
}

/// A text of the drawing: where it goes, in logical pixels from the
/// drawing's top left (its middle in height), which part it is about and
/// what it reads.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Callout {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) anchor: Anchor,
    pub(crate) part: Part,
    /// A number in its box, as a field; else a plain word.
    pub(crate) chip: bool,
    pub(crate) word: Word,
    /// What goes before the number ("Ø " for a diameter), and the number in
    /// mm: the panel writes it as its fields do.
    pub(crate) prefix: &'static str,
    pub(crate) value: f64,
    /// The field the number is, as the panel's fields are numbered, so it
    /// can be dragged where it stands; -1 for none.
    pub(crate) field: i32,
}

/// A drawing and its texts.
pub(crate) struct Drawing {
    pub(crate) pixels: SharedPixelBuffer<Rgba8Pixel>,
    pub(crate) callouts: Vec<Callout>,
}

/// The main drawing's size in the panel, logical pixels: the viewer's
/// inside. The 3D view and the plan share it, so switching moves nothing.
pub(crate) const SHAPE_WIDTH: f32 = 306.0;
pub(crate) const SHAPE_HEIGHT: f32 = 240.0;
/// Each lens's size: two side by side under the main drawing.
pub(crate) const LENS_WIDTH: f32 = 149.0;
pub(crate) const LENS_HEIGHT: f32 = 110.0;

/// How long the trunk is drawn in 3D, mm: any length does, this one leaves
/// room for the tip and the foot to read.
const TRUNK_SHOWN_MM: f32 = 3.0;
/// Segments around the axis.
const SEGMENTS: usize = 40;
/// The most a millimetre is drawn, logical pixels: 3D, plan. A support that
/// does not fit at it is drawn smaller; the 1 mm grid tells either way.
const MM_3D: f32 = 27.0;
const MM_PLAN: f32 = 32.0;

const BACKGROUND: [f32; 3] = [12.0, 14.0, 16.0];
const SUPPORT: [f32; 3] = [118.0, 136.0, 154.0];
const HOT: [f32; 3] = [226.0, 167.0, 80.0];
const LINE: [f32; 3] = [105.0, 115.0, 123.0];
const SURFACE: [f32; 3] = [194.0, 201.0, 207.0];
/// The outline of a part in section, the 1 mm grid, hatching (the model,
/// the plate), their edges, and what ships, dashed.
const OUTLINE: [f32; 3] = [143.0, 163.0, 182.0];
const GRID: [f32; 3] = [22.0, 26.0, 30.0];
/// The plate's grid under the 3D drawing, and its lines through the axis.
const FLOOR: [f32; 3] = [26.0, 31.0, 36.0];
const FLOOR_AXIS: [f32; 3] = [38.0, 45.0, 51.0];
const HATCH: [f32; 3] = [40.0, 47.0, 54.0];
const EDGE: [f32; 3] = [86.0, 97.0, 107.0];
const SHIPS: [f32; 3] = [190.0, 197.0, 203.0];

/// One triangle: its corners, the light at each, and its part.
struct Tri {
    v: [[f32; 3]; 3],
    light: [f32; 3],
    part: Part,
}

/// One corner of a triangle: where it is, its normal, and its part.
type Corner = ([f32; 3], [f32; 3], Part);

/// The support of `size` as triangles, in mm with z up from the plate, and
/// the height of its tip's contact.
fn geometry(size: &SupportSize) -> (Vec<Corner>, f32, f32) {
    let foot_r = (size.base_diameter_mm / 2.0) as f32;
    let foot_h = size.base_thickness_mm as f32;
    let trunk_r = (size.trunk_diameter_mm / 2.0) as f32;
    let tip_r = (size.tip_diameter_mm / 2.0) as f32;
    let join = size.join_cone_mm as f32;
    let cone_from = foot_h + join.max(0.0) + TRUNK_SHOWN_MM;
    let cone_to = cone_from + size.tip_length_mm as f32;
    // The tip's sphere is centred on the contact, past a neck of the tip's
    // width as long as the break point.
    let contact = cone_to + size.break_point_mm as f32;
    let mut out = Vec::new();
    // Each profile runs bottom to top, outward first, with the outward
    // normal (radial, axial) of every point.
    let flat = |a: [f32; 2], b: [f32; 2]| {
        let (dr, dz) = (b[0] - a[0], b[1] - a[1]);
        let l = dr.hypot(dz).max(1e-6);
        [dz / l, -dr / l]
    };
    let mut segment = |a: [f32; 2], b: [f32; 2], part: Part| {
        let n = flat(a, b);
        lathe(&mut out, &[(a, n), (b, n)], part);
    };
    segment([0.0, 0.0], [foot_r, 0.0], Part::Foot);
    segment([foot_r, 0.0], [foot_r, foot_h], Part::Foot);
    segment([foot_r, foot_h], [0.0, foot_h], Part::Foot);
    if join > 0.0 {
        // The engine's join cone: 45° from the trunk out (#360).
        segment(
            [trunk_r + join, foot_h],
            [trunk_r, foot_h + join],
            Part::Foot,
        );
    }
    segment([trunk_r, foot_h], [trunk_r, cone_from], Part::Trunk);
    segment([trunk_r, cone_from], [tip_r, cone_to], Part::Tip);
    if contact > cone_to {
        segment([tip_r, cone_to], [tip_r, contact], Part::Tip);
    }
    let ball: Vec<([f32; 2], [f32; 2])> = (0..=12)
        .map(|i| {
            let a = -std::f32::consts::FRAC_PI_2 + i as f32 / 12.0 * std::f32::consts::PI;
            (
                [tip_r * a.cos(), contact + tip_r * a.sin()],
                [a.cos(), a.sin()],
            )
        })
        .collect();
    lathe(&mut out, &ball, Part::Tip);
    (out, contact, contact + tip_r)
}

/// The surface swept by `profile` (points with their outward normals)
/// about the z axis, as triangles with a normal at each corner.
fn lathe(out: &mut Vec<([f32; 3], [f32; 3], Part)>, profile: &[([f32; 2], [f32; 2])], part: Part) {
    let at = |(p, n): ([f32; 2], [f32; 2]), k: usize| {
        let a = k as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
        let (s, c) = a.sin_cos();
        ([p[0] * c, p[0] * s, p[1]], [n[0] * c, n[0] * s, n[1]])
    };
    for pair in profile.windows(2) {
        for k in 0..SEGMENTS {
            let corners = [
                at(pair[0], k),
                at(pair[1], k),
                at(pair[1], k + 1),
                at(pair[0], k + 1),
            ];
            for [i, j, l] in [[0, 1, 2], [0, 2, 3]] {
                out.push((corners[i].0, corners[i].1, part));
                out.push((corners[j].0, corners[j].1, part));
                out.push((corners[l].0, corners[l].1, part));
            }
        }
    }
}

/// Buffer pixels drawn per physical pixel, each way.
const SS: f32 = 2.0;

/// What a drawing is made on: its colours at twice the size, halved at the
/// end so every edge is smooth. Its own shapes take logical pixels.
struct Canvas {
    color: Vec<[f32; 3]>,
    /// The buffer's size, and the picture's.
    bw: usize,
    bh: usize,
    pw: usize,
    ph: usize,
    /// Buffer pixels per logical pixel.
    k: f32,
}

impl Canvas {
    fn new(width: f32, height: f32, scale: f32) -> Self {
        let pw = ((width * scale).round() as usize).max(1);
        let ph = ((height * scale).round() as usize).max(1);
        let (bw, bh) = (pw * 2, ph * 2);
        Canvas {
            color: vec![BACKGROUND; bw * bh],
            bw,
            bh,
            pw,
            ph,
            k: SS * scale,
        }
    }

    /// A line one physical pixel wide.
    fn line(&mut self, a: [f32; 2], b: [f32; 2], col: [f32; 3]) {
        let k = self.k;
        line(
            &mut self.color,
            self.bw,
            self.bh,
            [a[0] * k, a[1] * k, 0.0],
            [b[0] * k, b[1] * k, 0.0],
            col,
            1.0,
            SS,
        );
    }

    fn dashed(&mut self, a: [f32; 2], b: [f32; 2], col: [f32; 3]) {
        let len = (b[0] - a[0]).hypot(b[1] - a[1]);
        if len < 1e-3 {
            return;
        }
        let at = |d: f32| {
            [
                a[0] + (b[0] - a[0]) * d / len,
                a[1] + (b[1] - a[1]) * d / len,
            ]
        };
        let mut d = 0.0;
        while d < len {
            self.line(at(d), at((d + 3.0).min(len)), col);
            d += 6.0;
        }
    }

    fn path(&mut self, points: &[[f32; 2]], col: [f32; 3]) {
        for pair in points.windows(2) {
            self.line(pair[0], pair[1], col);
        }
    }

    fn outline(&mut self, points: &[[f32; 2]], col: [f32; 3]) {
        self.path(points, col);
        if let (Some(first), Some(last)) = (points.first(), points.last()) {
            self.line(*last, *first, col);
        }
    }

    /// The inside of the polygon `points`, `alpha` of `col` over what is
    /// there.
    fn fill(&mut self, points: &[[f32; 2]], col: [f32; 3], alpha: f32) {
        let k = self.k;
        let p: Vec<[f32; 2]> = points.iter().map(|q| [q[0] * k, q[1] * k]).collect();
        let lo = p.iter().map(|q| q[1]).fold(f32::INFINITY, f32::min);
        let hi = p.iter().map(|q| q[1]).fold(f32::NEG_INFINITY, f32::max);
        let y0 = lo.floor().max(0.0) as usize;
        let y1 = (hi.ceil().max(0.0) as usize).min(self.bh.saturating_sub(1));
        let mut xs: Vec<f32> = Vec::new();
        for y in y0..=y1 {
            let yc = y as f32 + 0.5;
            xs.clear();
            for i in 0..p.len() {
                let (a, b) = (p[i], p[(i + 1) % p.len()]);
                if (a[1] <= yc) != (b[1] <= yc) {
                    xs.push(a[0] + (yc - a[1]) / (b[1] - a[1]) * (b[0] - a[0]));
                }
            }
            xs.sort_by(f32::total_cmp);
            for pair in xs.as_chunks::<2>().0 {
                let x0 = (pair[0] - 0.5).ceil().max(0.0) as usize;
                let x1 = (pair[1] - 0.5).floor();
                if x1 < 0.0 {
                    continue;
                }
                let x1 = (x1 as usize).min(self.bw - 1);
                for x in x0..=x1 {
                    let c = &mut self.color[y * self.bw + x];
                    for j in 0..3 {
                        c[j] += (col[j] - c[j]) * alpha;
                    }
                }
            }
        }
    }

    fn rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, col: [f32; 3], alpha: f32) {
        self.fill(&[[x0, y0], [x1, y0], [x1, y1], [x0, y1]], col, alpha);
    }

    fn circle(c: [f32; 2], r: f32) -> Vec<[f32; 2]> {
        (0..40)
            .map(|i| {
                let a = i as f32 / 40.0 * std::f32::consts::TAU;
                [c[0] + r * a.cos(), c[1] + r * a.sin()]
            })
            .collect()
    }

    /// Lines at 45°, 6 pixels apart, inside the rectangle: what is cut
    /// through (the model, the plate).
    fn hatch(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) {
        let h = y1 - y0;
        let mut c = x0;
        while c < x1 + h {
            let ya = y0.max(y0 + c - x1);
            let yb = y1.min(y0 + c - x0);
            if ya < yb {
                self.line([c - (ya - y0), ya], [c - (yb - y0), yb], HATCH);
            }
            c += 6.0;
        }
    }

    /// A dimension line with a slash at each end.
    fn dimension(&mut self, a: [f32; 2], b: [f32; 2], col: [f32; 3]) {
        self.line(a, b, col);
        for p in [a, b] {
            self.line([p[0] - 3.0, p[1] + 3.0], [p[0] + 3.0, p[1] - 3.0], col);
        }
    }

    /// A part in section: filled, with its outline, lit when pointed at.
    fn part(&mut self, points: &[[f32; 2]], lit: bool) {
        if lit {
            self.fill(points, HOT, 0.30);
            self.outline(points, HOT);
        } else {
            self.fill(points, SUPPORT, 0.40);
            self.outline(points, OUTLINE);
        }
    }

    fn finish(self) -> SharedPixelBuffer<Rgba8Pixel> {
        let (pw, ph, bw) = (self.pw, self.ph, self.bw);
        let mut pixels = SharedPixelBuffer::<Rgba8Pixel>::new(pw as u32, ph as u32);
        let out = pixels.make_mut_slice();
        for y in 0..ph {
            for x in 0..pw {
                let mut sum = [0.0f32; 3];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let c = self.color[(y * 2 + dy) * bw + x * 2 + dx];
                    for k in 0..3 {
                        sum[k] += c[k];
                    }
                }
                out[y * pw + x] = Rgba8Pixel {
                    r: (sum[0] / 4.0).round().clamp(0.0, 255.0) as u8,
                    g: (sum[1] / 4.0).round().clamp(0.0, 255.0) as u8,
                    b: (sum[2] / 4.0).round().clamp(0.0, 255.0) as u8,
                    a: 255,
                };
            }
        }
        pixels
    }
}

fn dim_colour(part: Part, hot: Part) -> [f32; 3] {
    if part == hot { HOT } else { LINE }
}

fn chip(
    x: f32,
    y: f32,
    anchor: Anchor,
    part: Part,
    prefix: &'static str,
    value: f64,
    field: i32,
) -> Callout {
    Callout {
        x,
        y,
        anchor,
        part,
        chip: true,
        word: Word::None,
        prefix,
        value,
        field,
    }
}

fn note(x: f32, y: f32, anchor: Anchor, word: Word, value: f64) -> Callout {
    Callout {
        x,
        y,
        anchor,
        part: Part::None,
        chip: false,
        word,
        prefix: "",
        value,
        field: -1,
    }
}

/// A text that says nothing: it holds the place of one a drawing does not
/// always carry, so a drawing's texts stay as many while a number is
/// dragged (the panel keeps each where it is, and the drag with it).
fn blank() -> Callout {
    note(0.0, 0.0, Anchor::Start, Word::None, 0.0)
}

/// Whether two numbers of a size differ enough to show.
fn differs(a: f64, b: f64) -> bool {
    (a - b).abs() > 0.0005
}

/// Draws the support of `size` in 3D, seen from `view`, in `width` ×
/// `height` logical pixels at `scale` physical pixels each, with `hot` lit:
/// standing on a 1 mm grid, its tip in the model's surface. Where its trunk
/// is not `factory`'s, that one shows dashed.
pub(crate) fn draw(
    size: &SupportSize,
    factory: Option<&SupportSize>,
    view: View,
    width: f32,
    height: f32,
    scale: f32,
    hot: Part,
) -> Drawing {
    let mut cv = Canvas::new(width, height, scale);
    let (bw, bh, k) = (cv.bw, cv.bh, cv.k);
    let (corners, contact, top) = geometry(size);
    let (sy, cy) = view.yaw.sin_cos();
    let (sp, cp) = view.pitch.sin_cos();
    let mid = top / 2.0;
    // The eye's frame: x right, y up, z towards the eye.
    let turn = |v: [f32; 3]| {
        let x = v[0] * cy - v[1] * sy;
        let y = v[0] * sy + v[1] * cy;
        [x, y * sp + v[2] * cp, -y * cp + v[2] * sp]
    };
    let foot_r = (size.base_diameter_mm / 2.0) as f32;
    // Fitted to what it covers as seen, the model's surface included, with
    // room left and right for the numbers; never larger than its real
    // scale.
    let surface_half = foot_r.max(1.6) * 0.95;
    let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    let mut reach = |v: [f32; 3]| {
        let e = turn([v[0], v[1], v[2] - mid]);
        for j in 0..2 {
            lo[j] = lo[j].min(e[j]);
            hi[j] = hi[j].max(e[j]);
        }
    };
    for c in &corners {
        reach(c.0);
    }
    for (x, y) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        reach([x * surface_half, y * surface_half, contact]);
    }
    let span = [(hi[0] - lo[0]).max(1e-3), (hi[1] - lo[1]).max(1e-3)];
    let mm = (bw as f32 * 0.66 / span[0])
        .min(bh as f32 * 0.84 / span[1])
        .min(MM_3D * k);
    let centre = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
    let (ox, oy) = (bw as f32 / 2.0, bh as f32 / 2.0 + 4.0 * k);
    let screen = |v: [f32; 3]| {
        let e = turn([v[0], v[1], v[2] - mid]);
        [
            ox + (e[0] - centre[0]) * mm,
            oy - (e[1] - centre[1]) * mm,
            e[2],
        ]
    };

    // The plate under it, a 1 mm grid.
    let n = (foot_r.ceil() as i32 + 2).clamp(3, 24);
    for i in -n..=n {
        let col = if i == 0 { FLOOR_AXIS } else { FLOOR };
        let (a, b) = (i as f32, n as f32);
        for (from, to) in [([a, -b, 0.0], [a, b, 0.0]), ([-b, a, 0.0], [b, a, 0.0])] {
            line(
                &mut cv.color,
                bw,
                bh,
                screen(from),
                screen(to),
                col,
                1.0,
                SS,
            );
        }
    }

    let light = {
        let l = [-0.35f32, 0.6, 0.75];
        let n = (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).sqrt();
        [l[0] / n, l[1] / n, l[2] / n]
    };
    let shade = |n: [f32; 3]| {
        let e = turn(n);
        0.42 + 0.62 * (e[0] * light[0] + e[1] * light[1] + e[2] * light[2]).max(0.0)
    };
    let tris: Vec<Tri> = corners
        .as_chunks::<3>()
        .0
        .iter()
        .map(|c| Tri {
            v: [screen(c[0].0), screen(c[1].0), screen(c[2].0)],
            light: [shade(c[0].1), shade(c[1].1), shade(c[2].1)],
            part: c[0].2,
        })
        .collect();
    let mut depth = vec![f32::NEG_INFINITY; bw * bh];
    for t in &tris {
        let base = if t.part == hot { HOT } else { SUPPORT };
        fill(&mut cv.color, &mut depth, bw, bh, t, base);
    }

    let tip_r = (size.tip_diameter_mm / 2.0) as f32;
    let trunk_r = (size.trunk_diameter_mm / 2.0) as f32;
    let cone_to = contact - size.break_point_mm as f32;
    let cone_from = cone_to - size.tip_length_mm as f32;
    let trunk_from = cone_from - TRUNK_SHOWN_MM;
    // A point of the outline as seen: `r` from the axis at height `z`, on
    // the left (-1) or the right (1), in logical pixels.
    let side = |r: f32, z: f32, sign: f32| {
        let (s, c) = (-view.yaw).sin_cos();
        let p = screen([sign * r * c, sign * r * s, z]);
        [p[0] / k, p[1] / k]
    };

    // The trunk the size ships with, dashed over the one drawn.
    let ships = factory.filter(|f| differs(f.trunk_diameter_mm, size.trunk_diameter_mm));
    if let Some(f) = ships {
        let r = (f.trunk_diameter_mm / 2.0) as f32;
        for sign in [-1.0, 1.0] {
            cv.dashed(
                side(r, trunk_from + 0.1, sign),
                side(r, cone_from - 0.1, sign),
                SHIPS,
            );
        }
    }

    // The model's surface where the tip touches it, see through: the tip's
    // ball sits half in it.
    let surface = (contact - size.penetration_mm as f32).max(0.0);
    let half = surface_half;
    let quad = [
        screen([-half, -half, surface]),
        screen([half, -half, surface]),
        screen([half, half, surface]),
        screen([-half, half, surface]),
    ];
    for [a, b, c] in [[0, 1, 2], [0, 2, 3]] {
        blend_tri(
            &mut cv.color,
            &depth,
            bw,
            bh,
            [quad[a], quad[b], quad[c]],
            SURFACE,
            0.13,
        );
    }
    for i in 0..4 {
        line(
            &mut cv.color,
            bw,
            bh,
            quad[i],
            quad[(i + 1) % 4],
            SURFACE,
            0.35,
            SS,
        );
    }

    // The numbers, each on a line from its part.
    let mut callouts = Vec::new();
    let a = side(tip_r, contact, 1.0);
    // Clear of the 2D / 3D switch at the top right.
    let at = [(a[0] + 34.0).min(width - 52.0), (a[1] - 12.0).max(32.0)];
    cv.line(a, at, dim_colour(Part::Tip, hot));
    callouts.push(chip(
        at[0],
        at[1],
        Anchor::Start,
        Part::Tip,
        "Ø ",
        size.tip_diameter_mm,
        0,
    ));
    let (t0, t1) = (side(trunk_r, cone_from, 1.0), side(tip_r, cone_to, 1.0));
    let xr = (t0[0] + 14.0).min(width - 46.0);
    cv.line([xr, t0[1]], [xr, t1[1] + 4.0], dim_colour(Part::Tip, hot));
    callouts.push(chip(
        xr + 7.0,
        (t0[1] + t1[1]) / 2.0 + 4.0,
        Anchor::Start,
        Part::Tip,
        "",
        size.tip_length_mm,
        2,
    ));
    let zt = trunk_from + TRUNK_SHOWN_MM * 0.45;
    let (l, r) = (side(trunk_r, zt, -1.0), side(trunk_r, zt, 1.0));
    let xe = (l[0] - 14.0).max(52.0);
    cv.line(l, r, dim_colour(Part::Trunk, hot));
    cv.line(l, [xe, l[1]], dim_colour(Part::Trunk, hot));
    callouts.push(chip(
        xe,
        l[1],
        Anchor::End,
        Part::Trunk,
        "Ø ",
        size.trunk_diameter_mm,
        1,
    ));
    callouts.push(ships.map_or_else(blank, |f| {
        note(
            xe,
            l[1] + 17.0,
            Anchor::End,
            Word::Ships,
            f.trunk_diameter_mm,
        )
    }));
    let fl = side(foot_r, 0.0, -1.0);
    let fa = [(fl[0] - 10.0).max(44.0), (fl[1] + 13.0).min(height - 12.0)];
    cv.line(fl, fa, dim_colour(Part::Foot, hot));
    callouts.push(chip(
        fa[0],
        fa[1],
        Anchor::End,
        Part::Foot,
        "Ø ",
        size.base_diameter_mm,
        3,
    ));
    // The model's name at the corner of its surface furthest left.
    let corner = quad
        .iter()
        .min_by(|a, b| a[0].total_cmp(&b[0]))
        .map_or([0.0, 0.0], |p| [p[0] / k, p[1] / k]);
    callouts.push(note(
        (corner[0] + 2.0).max(6.0),
        (corner[1] - 8.0).max(22.0),
        Anchor::Start,
        Word::Model,
        0.0,
    ));
    callouts.push(note(
        width - 7.0,
        height - 9.0,
        Anchor::End,
        Word::Grid,
        0.0,
    ));

    Drawing {
        pixels: cv.finish(),
        callouts,
    }
}

/// Draws the support of `size` as a plan: cut down its axis, at its real
/// scale on a 1 mm grid, the trunk broken so the tip and the foot both fit,
/// with each number on its dimension line. Where its trunk or its foot is
/// not `factory`'s, that one shows dashed.
pub(crate) fn draw_plan(
    size: &SupportSize,
    factory: Option<&SupportSize>,
    width: f32,
    height: f32,
    scale: f32,
    hot: Part,
) -> Drawing {
    let mut cv = Canvas::new(width, height, scale);
    let mut callouts = Vec::new();
    let tip_d = size.tip_diameter_mm as f32;
    let trunk_d = size.trunk_diameter_mm as f32;
    let foot_d = size.base_diameter_mm as f32;
    let (len, brk) = (size.tip_length_mm as f32, size.break_point_mm as f32);
    let (thick, join) = (
        size.base_thickness_mm as f32,
        (size.join_cone_mm as f32).max(0.0),
    );
    let pen = size.penetration_mm as f32;
    let cx = (width * 0.47).round();
    let (y_s, y_p) = (32.0, height - 34.0);
    // Its real scale, or what fits: across with room for the numbers, and
    // down with two stubs of trunk about the break.
    let across = foot_d.max(trunk_d + 2.0 * join).max(0.1);
    let down = (len + brk + thick + join).max(0.1);
    let s = MM_PLAN
        .min((width - 140.0) / across)
        .min((y_p - y_s - 56.0) / down)
        .max(1.0);
    let (rt, big_rt, rb, jw) = (
        tip_d / 2.0 * s,
        trunk_d / 2.0 * s,
        foot_d / 2.0 * s,
        join * s,
    );
    // The ball's centre (the contact), the neck's end, the cone's.
    let y_b = y_s - pen * s;
    let y_n = y_b + brk * s;
    let y_c = y_n + len * s;
    let (b1, b2) = (y_c + 26.0, y_c + 38.0);
    let y_d = y_p - thick * s;
    let y_j = y_d - jw;

    // The grid, a line through the axis and one on the plate.
    if s >= 6.0 {
        let mut x = cx % s;
        while x < width {
            cv.line([x, 0.0], [x, height], GRID);
            x += s;
        }
        let mut y = y_p % s;
        while y < height {
            cv.line([0.0, y], [width, y], GRID);
            y += s;
        }
    }
    // The model above, the plate below.
    cv.rect(0.0, 0.0, width, y_s, BACKGROUND, 1.0);
    cv.hatch(0.0, 0.0, width, y_s);
    cv.line([0.0, y_s], [width, y_s], EDGE);
    cv.hatch(0.0, y_p, width, y_p + 9.0);
    cv.line([0.0, y_p], [width, y_p], EDGE);

    // The foot: its join cone and its disk.
    let foot_lit = hot == Part::Foot;
    if jw > 0.0 {
        cv.part(
            &[
                [cx - big_rt, y_j],
                [cx + big_rt, y_j],
                [cx + big_rt + jw, y_d],
                [cx - big_rt - jw, y_d],
            ],
            foot_lit,
        );
    }
    cv.part(
        &[
            [cx - rb, y_d],
            [cx + rb, y_d],
            [cx + rb, y_p],
            [cx - rb, y_p],
        ],
        foot_lit,
    );
    // The trunk, broken in two.
    let trunk_lit = hot == Part::Trunk;
    let (fill, alpha, edge) = if trunk_lit {
        (HOT, 0.30, HOT)
    } else {
        (SUPPORT, 0.40, OUTLINE)
    };
    for (from, to) in [(y_c, b1), (b2, y_j)] {
        cv.rect(cx - big_rt, from, cx + big_rt, to, fill, alpha);
        cv.line([cx - big_rt, from], [cx - big_rt, to], edge);
        cv.line([cx + big_rt, from], [cx + big_rt, to], edge);
    }
    for y in [b1, b2] {
        let mut points = Vec::new();
        let (mut x, mut up) = (cx - big_rt - 6.0, false);
        while x <= cx + big_rt + 6.0 {
            points.push([x, if up { y - 2.5 } else { y + 2.5 }]);
            up = !up;
            x += 4.0;
        }
        cv.path(&points, OUTLINE);
    }
    // What ships, dashed.
    if let Some(f) = factory {
        if differs(f.trunk_diameter_mm, size.trunk_diameter_mm) {
            let g = (f.trunk_diameter_mm / 2.0) as f32 * s;
            for x in [cx - g, cx + g] {
                cv.dashed([x, y_c + 8.0], [x, b1 - 4.0], SHIPS);
                cv.dashed([x, b2 + 4.0], [x, y_j - 2.0], SHIPS);
            }
        }
        if differs(f.base_diameter_mm, size.base_diameter_mm) {
            let g = (f.base_diameter_mm / 2.0) as f32 * s;
            for x in [cx - g, cx + g] {
                cv.dashed([x, y_d - 8.0], [x, y_p + 6.0], SHIPS);
            }
        }
    }
    // The tip: cone, neck and ball.
    let tip_lit = hot == Part::Tip;
    cv.part(
        &[
            [cx - big_rt, y_c],
            [cx - rt, y_n],
            [cx - rt, y_b],
            [cx + rt, y_b],
            [cx + rt, y_n],
            [cx + big_rt, y_c],
        ],
        tip_lit,
    );
    cv.part(&Canvas::circle([cx, y_b], rt), tip_lit);

    // The numbers.
    let col = |part: Part| dim_colour(part, hot);
    let at = [(cx + 50.0).min(width - 60.0), y_s - 16.0];
    cv.path(
        &[[cx + rt + 2.0, y_b], [at[0] - 14.0, at[1]], at],
        col(Part::Tip),
    );
    callouts.push(chip(
        at[0],
        at[1],
        Anchor::Start,
        Part::Tip,
        "Ø ",
        size.tip_diameter_mm,
        0,
    ));
    let xl = (cx + big_rt + 16.0).min(width - 48.0);
    cv.dimension([xl, y_n], [xl, y_c], col(Part::Tip));
    callouts.push(chip(
        xl + 8.0,
        (y_n + y_c) / 2.0,
        Anchor::Start,
        Part::Tip,
        "",
        size.tip_length_mm,
        2,
    ));
    let yt = (b2 + y_j) / 2.0 + 2.0;
    let xe = (cx - big_rt - 16.0).max(52.0);
    cv.dimension([cx - big_rt, yt], [cx + big_rt, yt], col(Part::Trunk));
    cv.line([cx - big_rt, yt], [xe, yt], col(Part::Trunk));
    callouts.push(chip(
        xe,
        yt,
        Anchor::End,
        Part::Trunk,
        "Ø ",
        size.trunk_diameter_mm,
        1,
    ));
    let ships = factory.filter(|f| differs(f.trunk_diameter_mm, size.trunk_diameter_mm));
    callouts.push(ships.map_or_else(blank, |f| {
        note(xe, yt + 17.0, Anchor::End, Word::Ships, f.trunk_diameter_mm)
    }));
    cv.dimension(
        [cx - rb, y_p + 19.0],
        [cx + rb, y_p + 19.0],
        col(Part::Foot),
    );
    callouts.push(chip(
        cx,
        y_p + 19.0,
        Anchor::Middle,
        Part::Foot,
        "Ø ",
        size.base_diameter_mm,
        3,
    ));
    callouts.push(note(8.0, 10.0, Anchor::Start, Word::Model, 0.0));
    callouts.push(note(width - 8.0, y_p - 8.0, Anchor::End, Word::Plate, 0.0));
    // One millimetre, as drawn: under the model's surface, at the left.
    let bar = y_s + 20.0;
    cv.path(
        &[
            [10.0, bar - 5.0],
            [10.0, bar],
            [10.0 + s, bar],
            [10.0 + s, bar - 5.0],
        ],
        SHIPS,
    );
    callouts.push(note(16.0 + s, bar - 3.0, Anchor::Start, Word::OneMm, 0.0));

    Drawing {
        pixels: cv.finish(),
        callouts,
    }
}

/// Draws the tip of `size` close up, cut down its axis: the ball in the
/// model's surface, how far it sinks, and the neck where it breaks off.
pub(crate) fn draw_tip_lens(
    size: &SupportSize,
    width: f32,
    height: f32,
    scale: f32,
    hot: Part,
) -> Drawing {
    let mut cv = Canvas::new(width, height, scale);
    let mut callouts = Vec::new();
    let tip_d = (size.tip_diameter_mm as f32).max(0.01);
    let (len, brk) = (
        (size.tip_length_mm as f32).max(0.1),
        size.break_point_mm as f32,
    );
    let pen = size.penetration_mm as f32;
    // The ball 18 pixels in radius, at most 150 pixels a millimetre.
    let z = 150.0f32.min(18.0 / (tip_d / 2.0));
    let (cx, y_s) = ((width / 2.0).round(), 36.0);
    let r = tip_d / 2.0 * z;
    let y_b = y_s - pen * z;
    let y_end = height + 4.0;
    let y_n = (y_b + brk * z).min(y_end);
    let slope = ((size.trunk_diameter_mm as f32 - tip_d) / 2.0 / len).max(0.0);
    let rb = r + slope * (y_end - y_n);

    cv.hatch(0.0, 0.0, width, y_s);
    cv.line([0.0, y_s], [width, y_s], EDGE);
    let lit = hot == Part::Tip;
    cv.part(
        &[
            [cx - r, y_b],
            [cx + r, y_b],
            [cx + r, y_n],
            [cx + rb, y_end],
            [cx - rb, y_end],
            [cx - r, y_n],
        ],
        lit,
    );
    cv.part(&Canvas::circle([cx, y_b], r), lit);

    let col = dim_colour(Part::Tip, hot);
    callouts.push(note(5.0, 9.0, Anchor::Start, Word::Tip, 0.0));
    // How far the tip goes into the model: its radius and what is asked on
    // top of it.
    let sink = size.tip_diameter_mm / 2.0 + size.penetration_mm;
    let x = (cx + r + 8.0).min(width - 44.0);
    let top = (y_b - r).max(4.0);
    cv.dimension([x, top], [x, y_s], col);
    let cy = ((top + y_s) / 2.0 - 4.0).max(24.0);
    callouts.push(chip(x + 6.0, cy, Anchor::Start, Part::Tip, "", sink, -1));
    callouts.push(note(x + 25.0, cy - 14.0, Anchor::Middle, Word::Sinks, 0.0));
    let yd = y_s + 14.0;
    let xl = (cx - r - 6.0).max(48.0);
    cv.line([cx - r, yd], [xl, yd], col);
    callouts.push(chip(
        xl,
        yd,
        Anchor::End,
        Part::Tip,
        "Ø ",
        size.tip_diameter_mm,
        0,
    ));
    if brk > 0.0 {
        let from = y_s.max(y_b) + 3.0;
        let to = y_n.min(height - 6.0);
        if to > from + 4.0 {
            cv.dimension([x, from], [x, to], col);
        }
        let cy = ((y_s + to) / 2.0 + 8.0).clamp(yd + 20.0, height - 24.0);
        callouts.push(chip(
            x + 6.0,
            cy,
            Anchor::Start,
            Part::Tip,
            "",
            size.break_point_mm,
            5,
        ));
        callouts.push(note(x + 25.0, cy + 15.0, Anchor::Middle, Word::Break, 0.0));
    } else {
        callouts.extend([blank(), blank()]);
    }

    Drawing {
        pixels: cv.finish(),
        callouts,
    }
}

/// Draws the foot of `size` close up, half of it cut down the axis: the
/// disk on the plate, its thickness, and the cone that joins the trunk.
pub(crate) fn draw_foot_lens(
    size: &SupportSize,
    width: f32,
    height: f32,
    scale: f32,
    hot: Part,
) -> Drawing {
    let mut cv = Canvas::new(width, height, scale);
    let mut callouts = Vec::new();
    let foot_r = (size.base_diameter_mm as f32 / 2.0).max(0.05);
    let trunk_r = size.trunk_diameter_mm as f32 / 2.0;
    let (thick, join) = (
        size.base_thickness_mm as f32,
        (size.join_cone_mm as f32).max(0.0),
    );
    let (ax, y_p) = (14.0, height - 18.0);
    // Half the foot across the lens, the disk and the cone no taller than
    // half of it.
    let z = 44.0f32
        .min((width - ax - 20.0) / foot_r)
        .min(52.0 / (thick + join).max(0.05));
    let (rt, rb, jw) = (trunk_r * z, foot_r * z, join * z);
    let y_d = y_p - thick * z;
    let y_j = y_d - jw;

    cv.hatch(0.0, y_p, width, height);
    cv.line([0.0, y_p], [width, y_p], EDGE);
    cv.part(
        &[[ax, -2.0], [ax + rt, -2.0], [ax + rt, y_j], [ax, y_j]],
        hot == Part::Trunk,
    );
    let lit = hot == Part::Foot;
    if jw > 0.0 {
        cv.part(
            &[[ax, y_j], [ax + rt, y_j], [ax + rt + jw, y_d], [ax, y_d]],
            lit,
        );
    }
    cv.part(&[[ax, y_d], [ax + rb, y_d], [ax + rb, y_p], [ax, y_p]], lit);
    // The axis.
    cv.dashed([ax, 0.0], [ax, height], SHIPS);

    let col = dim_colour(Part::Foot, hot);
    callouts.push(note(width - 5.0, 9.0, Anchor::End, Word::FootHalf, 0.0));
    if jw > 0.0 {
        let xj = (ax + rt + jw + 8.0).min(width - 60.0);
        let chip_x = (ax + rt + 9.0).max(58.0).min(width - 86.0);
        cv.dimension([xj, y_j], [xj, y_d], col);
        cv.path(
            &[
                [xj, y_j - 2.0],
                [xj, 46.0],
                [xj.clamp(chip_x + 4.0, chip_x + 30.0), 46.0],
            ],
            col,
        );
        callouts.push(chip(
            chip_x,
            38.0,
            Anchor::Start,
            Part::Foot,
            "",
            size.join_cone_mm,
            7,
        ));
        callouts.push(note(chip_x + 19.0, 24.0, Anchor::Middle, Word::Cone, 0.0));
    } else {
        callouts.extend([blank(), blank()]);
    }
    let xg = (ax + rb + 8.0).min(width - 8.0);
    let chip_x = (xg + 12.0).min(width - 3.0);
    cv.dimension([xg, y_d], [xg, y_p], col);
    cv.line([xg, y_d - 2.0], [xg, 70.0], col);
    callouts.push(chip(
        chip_x,
        62.0,
        Anchor::End,
        Part::Foot,
        "",
        size.base_thickness_mm,
        6,
    ));
    callouts.push(note(chip_x - 19.0, 48.0, Anchor::Middle, Word::Thick, 0.0));

    Drawing {
        pixels: cv.finish(),
        callouts,
    }
}

/// The barycentric weights of `p` in the screen triangle `v`, or `None`
/// outside it.
fn inside(v: &[[f32; 3]; 3], p: [f32; 2]) -> Option<[f32; 3]> {
    let area =
        (v[1][0] - v[0][0]) * (v[2][1] - v[0][1]) - (v[2][0] - v[0][0]) * (v[1][1] - v[0][1]);
    if area.abs() < 1e-9 {
        return None;
    }
    let w =
        |a: [f32; 3], b: [f32; 3]| (b[0] - a[0]) * (p[1] - a[1]) - (p[0] - a[0]) * (b[1] - a[1]);
    let w0 = w(v[1], v[2]) / area;
    let w1 = w(v[2], v[0]) / area;
    let w2 = w(v[0], v[1]) / area;
    (w0 >= -1e-6 && w1 >= -1e-6 && w2 >= -1e-6).then_some([w0, w1, w2])
}

/// The pixel box a screen triangle covers, inside the buffer.
fn bounds(v: &[[f32; 3]; 3], w: usize, h: usize) -> (usize, usize, usize, usize) {
    let lo = |k: usize| v.iter().map(|p| p[k]).fold(f32::INFINITY, f32::min);
    let hi = |k: usize| v.iter().map(|p| p[k]).fold(f32::NEG_INFINITY, f32::max);
    let x0 = lo(0).floor().max(0.0) as usize;
    let y0 = lo(1).floor().max(0.0) as usize;
    let x1 = (hi(0).ceil().max(0.0) as usize).min(w.saturating_sub(1));
    let y1 = (hi(1).ceil().max(0.0) as usize).min(h.saturating_sub(1));
    (x0, y0, x1, y1)
}

fn fill(color: &mut [[f32; 3]], depth: &mut [f32], w: usize, h: usize, t: &Tri, base: [f32; 3]) {
    let (x0, y0, x1, y1) = bounds(&t.v, w, h);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let Some(b) = inside(&t.v, [x as f32 + 0.5, y as f32 + 0.5]) else {
                continue;
            };
            let z = b[0] * t.v[0][2] + b[1] * t.v[1][2] + b[2] * t.v[2][2];
            let i = y * w + x;
            if z <= depth[i] {
                continue;
            }
            depth[i] = z;
            let k = b[0] * t.light[0] + b[1] * t.light[1] + b[2] * t.light[2];
            color[i] = [base[0] * k, base[1] * k, base[2] * k];
        }
    }
}

fn blend_tri(
    color: &mut [[f32; 3]],
    depth: &[f32],
    w: usize,
    h: usize,
    v: [[f32; 3]; 3],
    over: [f32; 3],
    alpha: f32,
) {
    let (x0, y0, x1, y1) = bounds(&v, w, h);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let Some(b) = inside(&v, [x as f32 + 0.5, y as f32 + 0.5]) else {
                continue;
            };
            let z = b[0] * v[0][2] + b[1] * v[1][2] + b[2] * v[2][2];
            let i = y * w + x;
            if z < depth[i] {
                continue;
            }
            for k in 0..3 {
                color[i][k] += (over[k] - color[i][k]) * alpha;
            }
        }
    }
}

/// A line `width` buffer pixels wide, over everything.
#[allow(clippy::too_many_arguments)]
fn line(
    color: &mut [[f32; 3]],
    w: usize,
    h: usize,
    a: [f32; 3],
    b: [f32; 3],
    col: [f32; 3],
    alpha: f32,
    width: f32,
) {
    let len = (b[0] - a[0]).hypot(b[1] - a[1]);
    let steps = (len.ceil() as usize).max(1);
    for s in 0..=steps {
        let t = s as f32 / steps as f32;
        let (cx, cy) = (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t);
        let r = (width / 2.0).max(0.5);
        let (x0, x1) = (
            (cx - r).floor().max(0.0) as usize,
            ((cx + r).ceil() as usize).min(w - 1),
        );
        let (y0, y1) = (
            (cy - r).floor().max(0.0) as usize,
            ((cy + r).ceil() as usize).min(h - 1),
        );
        for y in y0..=y1 {
            for x in x0..=x1 {
                if (x as f32 + 0.5 - cx).hypot(y as f32 + 0.5 - cy) <= r {
                    let i = y * w + x;
                    for k in 0..3 {
                        color[i][k] += (col[k] - color[i][k]) * alpha;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small() -> SupportSize {
        sc_geom::df_supports::zone_presets::minis_30()
            .rules
            .sizes
            .into_iter()
            .find(|s| s.id == "small")
            .expect("the small size ships")
    }

    fn lit(d: &Drawing, colour: [u8; 3]) -> usize {
        d.pixels
            .as_slice()
            .iter()
            .filter(|p| {
                (i32::from(p.r) - i32::from(colour[0])).abs() < 60
                    && (i32::from(p.g) - i32::from(colour[1])).abs() < 60
                    && (i32::from(p.b) - i32::from(colour[2])).abs() < 60
                    && p.r > 120
                    && p.r > p.b + 40
            })
            .count()
    }

    /// The numbers of a drawing in their boxes: what goes before each, the
    /// number and its field.
    fn chips(d: &Drawing) -> Vec<(&'static str, f64, i32)> {
        d.callouts
            .iter()
            .filter(|c| c.chip)
            .map(|c| (c.prefix, c.value, c.field))
            .collect()
    }

    /// The texts of a drawing that say something.
    fn shown(d: &Drawing) -> Vec<&Callout> {
        d.callouts
            .iter()
            .filter(|c| c.chip || c.word != Word::None)
            .collect()
    }

    fn words(d: &Drawing) -> Vec<Word> {
        d.callouts
            .iter()
            .filter(|c| !c.chip && c.word != Word::None)
            .map(|c| c.word)
            .collect()
    }

    fn solid(size: &SupportSize, hot: Part) -> Drawing {
        draw(size, None, View::default(), 300.0, 240.0, 1.0, hot)
    }

    const FOUR: [(&str, f64, i32); 4] = [
        ("Ø ", 0.24, 0),
        ("", 2.3, 2),
        ("Ø ", 1.1, 1),
        ("Ø ", 5.0, 3),
    ];

    /// The 3D drawing has the size asked for, shows the support, and
    /// numbers the four basic measures where their parts are, each the
    /// field it is.
    #[test]
    fn a_support_is_drawn_with_its_four_numbers() {
        let d = draw(
            &small(),
            None,
            View::default(),
            300.0,
            240.0,
            1.5,
            Part::None,
        );
        assert_eq!((d.pixels.width(), d.pixels.height()), (450, 360));
        let drawn = d
            .pixels
            .as_slice()
            .iter()
            .filter(|p| p.r > 60 || p.g > 60 || p.b > 60)
            .count();
        assert!(drawn > 4000, "the support covers the picture: {drawn}");
        assert_eq!(chips(&d), FOUR);
        for c in shown(&d) {
            assert!(
                c.x > 0.0 && c.x < 300.0 && c.y > 0.0 && c.y < 240.0,
                "{c:?}"
            );
        }
        // The tip's numbers stand above the trunk's, the foot's lowest.
        let at: Vec<f32> = d.callouts.iter().filter(|c| c.chip).map(|c| c.y).collect();
        assert!(at[0] < at[2] && at[2] < at[3]);
        assert_eq!(words(&d), [Word::Model, Word::Grid]);
    }

    /// Pointing at a part lights it: amber where it was grey.
    #[test]
    fn the_part_pointed_at_lights_up() {
        let plain = solid(&small(), Part::None);
        let foot = solid(&small(), Part::Foot);
        let tip = solid(&small(), Part::Tip);
        assert_eq!(lit(&plain, [226, 167, 80]), 0);
        assert!(
            lit(&foot, [226, 167, 80]) > lit(&tip, [226, 167, 80]),
            "the foot is the bigger part"
        );
        assert!(lit(&tip, [226, 167, 80]) > 30);
    }

    /// A value changed changes the drawing: a wider foot reaches further.
    #[test]
    fn a_wider_foot_is_drawn_wider() {
        let mut wide = small();
        wide.base_diameter_mm = 8.0;
        let a = solid(&small(), Part::None);
        let b = solid(&wide, Part::None);
        assert_ne!(a.pixels.as_slice(), b.pixels.as_slice());
        assert_eq!(chips(&b)[3].1, 8.0);
    }

    /// A trunk that is not the one the size ships with shows that one, and
    /// says its number; the same trunk says nothing.
    #[test]
    fn a_changed_trunk_shows_the_one_that_ships() {
        let mut thick = small();
        thick.trunk_diameter_mm = 1.6;
        for d in [
            draw(
                &thick,
                Some(&small()),
                View::default(),
                300.0,
                240.0,
                1.0,
                Part::None,
            ),
            draw_plan(&thick, Some(&small()), 306.0, 240.0, 1.0, Part::None),
        ] {
            let ships: Vec<f64> = d
                .callouts
                .iter()
                .filter(|c| c.word == Word::Ships)
                .map(|c| c.value)
                .collect();
            assert_eq!(ships, [1.1]);
        }
        let same = draw_plan(&small(), Some(&small()), 306.0, 240.0, 1.0, Part::None);
        assert!(!words(&same).contains(&Word::Ships));
        assert_ne!(
            draw_plan(&thick, Some(&small()), 306.0, 240.0, 1.0, Part::None)
                .pixels
                .as_slice(),
            draw_plan(&thick, None, 306.0, 240.0, 1.0, Part::None)
                .pixels
                .as_slice(),
            "dashed where it ships"
        );
    }

    /// The plan carries the same four numbers, lights the part pointed at,
    /// and draws a millimetre as wide whatever the trunk; a foot too wide
    /// for it is drawn smaller, its numbers still inside.
    #[test]
    fn the_plan_is_at_its_real_scale_until_it_does_not_fit() {
        let d = draw_plan(&small(), None, 306.0, 240.0, 1.0, Part::None);
        assert_eq!(chips(&d), FOUR);
        assert_eq!(words(&d), [Word::Model, Word::Plate, Word::OneMm]);
        assert_eq!(lit(&d, [226, 167, 80]), 0);
        assert!(
            lit(
                &draw_plan(&small(), None, 306.0, 240.0, 1.0, Part::Trunk),
                [226, 167, 80]
            ) > 30
        );
        // The trunk's number stands left of the trunk: further left for a
        // thicker one, by half the difference at 32 pixels a millimetre.
        let trunk_x = |d: &Drawing| d.callouts.iter().find(|c| c.field == 1).unwrap().x;
        let mut thick = small();
        thick.trunk_diameter_mm = 2.1;
        let wide = draw_plan(&thick, None, 306.0, 240.0, 1.0, Part::None);
        assert!((trunk_x(&d) - trunk_x(&wide) - 0.5 * 32.0).abs() < 0.01);
        let mut huge = small();
        huge.base_diameter_mm = 24.0;
        let far = draw_plan(&huge, None, 306.0, 240.0, 1.0, Part::None);
        for c in shown(&far) {
            assert!(
                c.x > 0.0 && c.x < 306.0 && c.y > 0.0 && c.y < 240.0,
                "{c:?}"
            );
        }
    }

    /// The tip's lens says how far the tip sinks (its radius and what is
    /// asked on top) and its width; the neck's length only when there is
    /// one. The foot's says its thickness and its join cone.
    #[test]
    fn the_lenses_number_what_is_too_small_to_see() {
        let tip = draw_tip_lens(&small(), 149.0, 110.0, 1.0, Part::None);
        assert_eq!(chips(&tip), [("", 0.12, -1), ("Ø ", 0.24, 0)]);
        assert_eq!(words(&tip), [Word::Tip, Word::Sinks]);
        let mut neck = small();
        neck.break_point_mm = 0.3;
        neck.penetration_mm = 0.1;
        let tip = draw_tip_lens(&neck, 149.0, 110.0, 1.0, Part::Tip);
        let numbers = chips(&tip);
        assert!((numbers[0].1 - 0.22).abs() < 1e-9);
        assert_eq!(numbers[2], ("", 0.3, 5));
        assert!(words(&tip).contains(&Word::Break));
        assert!(lit(&tip, [226, 167, 80]) > 30);

        let foot = draw_foot_lens(&small(), 149.0, 110.0, 1.0, Part::None);
        assert_eq!(chips(&foot), [("", 0.4, 7), ("", 0.25, 6)]);
        assert_eq!(words(&foot), [Word::FootHalf, Word::Cone, Word::Thick]);
        for c in shown(&tip).into_iter().chain(shown(&foot)) {
            assert!(
                c.x > 0.0 && c.x < 149.0 && c.y > 0.0 && c.y < 110.0,
                "{c:?}"
            );
        }
    }

    /// Dragging turns the view, and the eye never goes past straight above.
    #[test]
    fn a_drag_turns_the_view_within_its_bounds() {
        let v = View::default().dragged(100.0, 0.0);
        assert!((v.yaw - (0.6 + 1.2)).abs() < 1e-5);
        assert_eq!(View::default().dragged(0.0, 10_000.0).pitch, 1.3);
        assert_eq!(View::default().dragged(0.0, -10_000.0).pitch, -0.2);
    }

    /// With `BREADSLICER_SHAPE_SHOT` naming a folder, writes each drawing,
    /// to look at.
    #[test]
    fn the_pictures_of_the_shape_are_written_when_asked() {
        let Some(dir) = std::env::var_os("BREADSLICER_SHAPE_SHOT") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        let _ = std::fs::create_dir_all(&dir);
        let mut neck = small();
        neck.break_point_mm = 0.3;
        neck.trunk_diameter_mm = 1.6;
        let factory = small();
        let save = |name: &str, d: Drawing| {
            let (w, h) = (d.pixels.width(), d.pixels.height());
            let bytes: Vec<u8> = d
                .pixels
                .as_slice()
                .iter()
                .flat_map(|p| [p.r, p.g, p.b, p.a])
                .collect();
            image::RgbaImage::from_raw(w, h, bytes)
                .expect("the buffer is the size")
                .save(dir.join(format!("shape-{name}.png")))
                .expect("written");
        };
        let (w, h) = (SHAPE_WIDTH, SHAPE_HEIGHT);
        save(
            "3d",
            draw(
                &small(),
                Some(&factory),
                View::default(),
                w,
                h,
                2.0,
                Part::None,
            ),
        );
        save(
            "3d-trunk",
            draw(
                &neck,
                Some(&factory),
                View::default(),
                w,
                h,
                2.0,
                Part::Trunk,
            ),
        );
        save(
            "plan",
            draw_plan(&small(), Some(&factory), w, h, 2.0, Part::None),
        );
        save(
            "plan-trunk",
            draw_plan(&neck, Some(&factory), w, h, 2.0, Part::Trunk),
        );
        save(
            "lens-tip",
            draw_tip_lens(&neck, LENS_WIDTH, LENS_HEIGHT, 2.0, Part::None),
        );
        save(
            "lens-foot",
            draw_foot_lens(&neck, LENS_WIDTH, LENS_HEIGHT, 2.0, Part::Foot),
        );
    }
}
