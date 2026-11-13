/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! One support drawn on its own for the Supports tab (#564): the size a
//! preset places, built from the numbers the engine reads (tip, cone, break
//! point, trunk, join cone, foot) the way the engine draws them, and turned
//! by the person. The drawing is small enough to make on the UI thread at
//! every change, so a value typed shows at once.
//!
//! Each part is a surface of revolution about the support's axis, drawn
//! with a depth buffer at twice the size and halved, so its edges are
//! smooth. The model's surface the tip goes into is drawn over it, see
//! through, and the dimension lines are drawn in; their numbers are left to
//! the panel, which lays out text better.

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

/// A number of the drawing: where its label goes, in logical pixels from
/// the drawing's top left, which part it measures and what it reads.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Callout {
    pub(crate) x: f32,
    pub(crate) y: f32,
    /// The label ends at `x` instead of starting there.
    pub(crate) right: bool,
    pub(crate) part: Part,
    /// What goes before the number ("Ø " for a diameter), and the number in
    /// mm: the panel writes it as its fields do.
    pub(crate) prefix: &'static str,
    pub(crate) value: f64,
}

/// The drawing and its numbers.
pub(crate) struct Drawing {
    pub(crate) pixels: SharedPixelBuffer<Rgba8Pixel>,
    pub(crate) callouts: Vec<Callout>,
}

/// The drawing's size in the panel, logical pixels: the viewer's inside.
pub(crate) const SHAPE_WIDTH: f32 = 306.0;
pub(crate) const SHAPE_HEIGHT: f32 = 198.0;

/// How long the trunk is drawn, mm: any length does, this one leaves room
/// for the tip and the foot to read.
const TRUNK_SHOWN_MM: f32 = 5.0;
/// Segments around the axis.
const SEGMENTS: usize = 40;

const BACKGROUND: [f32; 3] = [12.0, 14.0, 16.0];
const SUPPORT: [f32; 3] = [118.0, 136.0, 154.0];
const HOT: [f32; 3] = [226.0, 167.0, 80.0];
const LINE: [f32; 3] = [105.0, 115.0, 123.0];
const SURFACE: [f32; 3] = [194.0, 201.0, 207.0];

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

/// Draws the support of `size` seen from `view` in `width` × `height`
/// logical pixels at `scale` physical pixels each, with `hot` lit.
pub(crate) fn draw(
    size: &SupportSize,
    view: View,
    width: f32,
    height: f32,
    scale: f32,
    hot: Part,
) -> Drawing {
    const SS: f32 = 2.0;
    let pw = ((width * scale).round() as usize).max(1);
    let ph = ((height * scale).round() as usize).max(1);
    let (bw, bh) = (pw * 2, ph * 2);
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
    // room left and right for the numbers.
    let surface_half = foot_r.max(1.6) * 0.95;
    let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    let mut reach = |v: [f32; 3]| {
        let e = turn([v[0], v[1], v[2] - mid]);
        for k in 0..2 {
            lo[k] = lo[k].min(e[k]);
            hi[k] = hi[k].max(e[k]);
        }
    };
    for c in &corners {
        reach(c.0);
    }
    for (x, y) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        reach([x * surface_half, y * surface_half, contact]);
    }
    let span = [(hi[0] - lo[0]).max(1e-3), (hi[1] - lo[1]).max(1e-3)];
    let mm = (bw as f32 * 0.66 / span[0]).min(bh as f32 * 0.86 / span[1]);
    let centre = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
    let (ox, oy) = (bw as f32 / 2.0, bh as f32 / 2.0);
    let screen = |v: [f32; 3]| {
        let e = turn([v[0], v[1], v[2] - mid]);
        [
            ox + (e[0] - centre[0]) * mm,
            oy - (e[1] - centre[1]) * mm,
            e[2],
        ]
    };
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

    let mut color = vec![BACKGROUND; bw * bh];
    let mut depth = vec![f32::NEG_INFINITY; bw * bh];
    for t in &tris {
        let base = if t.part == hot { HOT } else { SUPPORT };
        fill(&mut color, &mut depth, bw, bh, t, base);
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
            &mut color,
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
            &mut color,
            bw,
            bh,
            quad[i],
            quad[(i + 1) % 4],
            SURFACE,
            0.35,
            SS,
        );
    }

    // Dimension lines, and where their numbers go.
    let mut callouts = Vec::new();
    let tip_r = (size.tip_diameter_mm / 2.0) as f32;
    let trunk_r = (size.trunk_diameter_mm / 2.0) as f32;
    let cone_to = contact - size.break_point_mm as f32;
    let cone_from = cone_to - size.tip_length_mm as f32;
    let mut dim =
        |part: Part, a: [f32; 3], b: [f32; 3], right: bool, prefix: &'static str, value: f64| {
            let (pa, pb) = (screen(a), screen(b));
            let col = if part == hot { HOT } else { LINE };
            line(&mut color, bw, bh, pa, pb, col, 1.0, SS);
            let x = if right {
                pa[0].max(pb[0]) + 8.0 * SS
            } else {
                pa[0].min(pb[0]) - 8.0 * SS
            };
            let y = (pa[1] + pb[1]) / 2.0;
            callouts.push(Callout {
                x: x / (SS * scale),
                y: y / (SS * scale),
                right: !right,
                part,
                prefix,
                value,
            });
        };
    dim(
        Part::Tip,
        [-tip_r, 0.0, contact],
        [tip_r, 0.0, contact],
        true,
        "Ø ",
        size.tip_diameter_mm,
    );
    dim(
        Part::Tip,
        [trunk_r + 0.35, 0.0, cone_from],
        [trunk_r + 0.35, 0.0, cone_to],
        true,
        "",
        size.tip_length_mm,
    );
    let trunk_at = cone_from - TRUNK_SHOWN_MM * 0.45;
    dim(
        Part::Trunk,
        [-trunk_r, 0.0, trunk_at],
        [trunk_r, 0.0, trunk_at],
        false,
        "Ø ",
        size.trunk_diameter_mm,
    );
    dim(
        Part::Foot,
        [-foot_r, 0.0, -0.25],
        [foot_r, 0.0, -0.25],
        false,
        "Ø ",
        size.base_diameter_mm,
    );

    let mut pixels = SharedPixelBuffer::<Rgba8Pixel>::new(pw as u32, ph as u32);
    let out = pixels.make_mut_slice();
    for y in 0..ph {
        for x in 0..pw {
            let mut sum = [0.0f32; 3];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let c = color[(y * 2 + dy) * bw + x * 2 + dx];
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
    Drawing { pixels, callouts }
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

    /// The drawing has the size asked for, shows the support, and numbers
    /// the four basic measures where their parts are.
    #[test]
    fn a_support_is_drawn_with_its_four_numbers() {
        let d = draw(&small(), View::default(), 300.0, 200.0, 1.5, Part::None);
        assert_eq!((d.pixels.width(), d.pixels.height()), (450, 300));
        let drawn = d
            .pixels
            .as_slice()
            .iter()
            .filter(|p| p.r > 40 || p.g > 40 || p.b > 40)
            .count();
        assert!(drawn > 4000, "the support covers the picture: {drawn}");
        let numbers: Vec<(&str, f64)> = d.callouts.iter().map(|c| (c.prefix, c.value)).collect();
        assert_eq!(numbers, [("Ø ", 0.24), ("", 2.3), ("Ø ", 1.1), ("Ø ", 5.0)]);
        for c in &d.callouts {
            assert!(
                c.x > 0.0 && c.x < 300.0 && c.y > 0.0 && c.y < 200.0,
                "{c:?}"
            );
        }
        // The tip's numbers stand above the trunk's, the foot's lowest.
        assert!(d.callouts[0].y < d.callouts[2].y && d.callouts[2].y < d.callouts[3].y);
    }

    /// Pointing at a part lights it: amber where it was grey.
    #[test]
    fn the_part_pointed_at_lights_up() {
        let plain = draw(&small(), View::default(), 300.0, 200.0, 1.0, Part::None);
        let foot = draw(&small(), View::default(), 300.0, 200.0, 1.0, Part::Foot);
        let tip = draw(&small(), View::default(), 300.0, 200.0, 1.0, Part::Tip);
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
        let a = draw(&small(), View::default(), 300.0, 200.0, 1.0, Part::None);
        let b = draw(&wide, View::default(), 300.0, 200.0, 1.0, Part::None);
        assert_ne!(a.pixels.as_slice(), b.pixels.as_slice());
        assert_eq!(b.callouts[3].value, 8.0);
    }

    /// Dragging turns the view, and the eye never goes past straight above.
    #[test]
    fn a_drag_turns_the_view_within_its_bounds() {
        let v = View::default().dragged(100.0, 0.0);
        assert!((v.yaw - (0.6 + 1.2)).abs() < 1e-5);
        assert_eq!(View::default().dragged(0.0, 10_000.0).pitch, 1.3);
        assert_eq!(View::default().dragged(0.0, -10_000.0).pitch, -0.2);
    }

    /// With `BREADSLICER_SHAPE_SHOT` naming a folder, writes the drawing of
    /// each part lit, to look at.
    #[test]
    fn the_pictures_of_the_shape_are_written_when_asked() {
        let Some(dir) = std::env::var_os("BREADSLICER_SHAPE_SHOT") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        let _ = std::fs::create_dir_all(&dir);
        let mut neck = small();
        neck.break_point_mm = 0.4;
        neck.penetration_mm = 0.1;
        for (name, size, hot) in [
            ("plain", small(), Part::None),
            ("tip", small(), Part::Tip),
            ("trunk", small(), Part::Trunk),
            ("foot", small(), Part::Foot),
            ("neck", neck, Part::Tip),
        ] {
            let d = draw(&size, View::default(), 308.0, 210.0, 2.0, hot);
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
        }
    }
}
