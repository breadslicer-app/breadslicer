/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The shapes of the plate checklist (#740): what an object leaves on the
//! plate, and its outline seen from above, as smooth closed outlines in the
//! plate's millimetres.
//!
//! The triangles are drawn on a grid of [`CELL_MM`] cells, the holes filled,
//! and the grid's border traced, simplified and rounded: a drawing to tick
//! parts off, not a measure.

/// The side of a grid cell, mm.
pub(crate) const CELL_MM: f32 = 0.2;

/// How high above an object's lowest point its footprint is read, mm: what
/// lies there stands on the plate (a raft, a base, the feet).
pub(crate) const BASE_BAND_MM: f32 = 1.0;

/// A triangle, its corners in the plate's millimetres.
pub(crate) type Triangle = [[f32; 3]; 3];

/// A closed outline, its points in the plate's millimetres.
pub(crate) type Outline = Vec<[f32; 2]>;

/// What an object is drawn as: its outlines and where its number goes.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Shape {
    pub outlines: Vec<Outline>,
    /// The point deepest inside the shape, mm.
    pub label_at: Option<[f32; 2]>,
    /// The area the shape takes, mm².
    pub area_mm2: f32,
}

impl Shape {
    pub(crate) fn is_empty(&self) -> bool {
        self.outlines.is_empty()
    }
}

/// The cells a set of triangles covers, on a grid of [`CELL_MM`].
#[derive(Clone, Debug)]
pub(crate) struct Mask {
    x0: f32,
    y0: f32,
    w: usize,
    h: usize,
    cells: Vec<bool>,
}

impl Mask {
    /// An empty grid that holds `min`–`max`, with a free cell all round.
    fn over(min: [f32; 2], max: [f32; 2]) -> Self {
        let x0 = min[0] - 2.0 * CELL_MM;
        let y0 = min[1] - 2.0 * CELL_MM;
        let w = (((max[0] - x0) / CELL_MM).ceil() as usize + 3).max(3);
        let h = (((max[1] - y0) / CELL_MM).ceil() as usize + 3).max(3);
        Self {
            x0,
            y0,
            w,
            h,
            cells: vec![false; w * h],
        }
    }

    fn at(&self, x: usize, y: usize) -> bool {
        self.cells[y * self.w + x]
    }

    fn set(&mut self, x: usize, y: usize) {
        self.cells[y * self.w + x] = true;
    }

    /// Covers the cells whose middle the flat triangle `t` holds, and the
    /// cell of its middle (a triangle smaller than a cell still marks one).
    fn cover(&mut self, t: [[f32; 2]; 3]) {
        let to_cell = |p: [f32; 2]| [(p[0] - self.x0) / CELL_MM, (p[1] - self.y0) / CELL_MM];
        let [a, b, c] = t.map(to_cell);
        let mid = [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0];
        if mid[0] >= 0.0 && mid[1] >= 0.0 {
            let (mx, my) = (mid[0] as usize, mid[1] as usize);
            if mx < self.w && my < self.h {
                self.set(mx, my);
            }
        }
        let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        if area.abs() < 1e-9 {
            return;
        }
        let lo_x = a[0].min(b[0]).min(c[0]).floor().max(0.0) as usize;
        let hi_x = (a[0].max(b[0]).max(c[0]).ceil() as usize).min(self.w);
        let lo_y = a[1].min(b[1]).min(c[1]).floor().max(0.0) as usize;
        let hi_y = (a[1].max(b[1]).max(c[1]).ceil() as usize).min(self.h);
        let edge = |p: [f32; 2], q: [f32; 2], x: f32, y: f32| {
            (q[0] - p[0]) * (y - p[1]) - (q[1] - p[1]) * (x - p[0])
        };
        for y in lo_y..hi_y {
            let cy = y as f32 + 0.5;
            for x in lo_x..hi_x {
                let cx = x as f32 + 0.5;
                let (e0, e1, e2) = (edge(a, b, cx, cy), edge(b, c, cx, cy), edge(c, a, cx, cy));
                let inside = if area > 0.0 {
                    e0 >= 0.0 && e1 >= 0.0 && e2 >= 0.0
                } else {
                    e0 <= 0.0 && e1 <= 0.0 && e2 <= 0.0
                };
                if inside {
                    self.set(x, y);
                }
            }
        }
    }

    /// Grows the covered cells by one (a cell with a covered neighbour).
    fn dilated(&self) -> Self {
        let mut out = self.clone();
        for y in 0..self.h {
            for x in 0..self.w {
                if self.at(x, y) {
                    continue;
                }
                let near = neighbours(x, y, self.w, self.h).any(|(nx, ny)| self.at(nx, ny));
                if near {
                    out.set(x, y);
                }
            }
        }
        out
    }

    /// Shrinks the covered cells by one (a cell with a free neighbour).
    fn eroded(&self) -> Self {
        let mut out = self.clone();
        for y in 0..self.h {
            for x in 0..self.w {
                if !self.at(x, y) {
                    continue;
                }
                let edge = x == 0 || y == 0 || x + 1 == self.w || y + 1 == self.h;
                if edge || neighbours(x, y, self.w, self.h).any(|(nx, ny)| !self.at(nx, ny)) {
                    out.cells[y * self.w + x] = false;
                }
            }
        }
        out
    }

    /// The same cells with every hole filled: a free cell no path of free
    /// cells joins to the border is covered.
    fn filled(&self) -> Self {
        let mut outside = vec![false; self.cells.len()];
        let mut stack = Vec::new();
        for x in 0..self.w {
            stack.push((x, 0));
            stack.push((x, self.h - 1));
        }
        for y in 0..self.h {
            stack.push((0, y));
            stack.push((self.w - 1, y));
        }
        while let Some((x, y)) = stack.pop() {
            let i = y * self.w + x;
            if outside[i] || self.cells[i] {
                continue;
            }
            outside[i] = true;
            stack.extend(neighbours(x, y, self.w, self.h));
        }
        Self {
            cells: outside.iter().map(|o| !o).collect(),
            ..self.clone()
        }
    }

    fn count(&self) -> usize {
        self.cells.iter().filter(|&&c| c).count()
    }

    /// The covered cell farthest from any free one, as a point in mm.
    fn deepest(&self) -> Option<[f32; 2]> {
        // Two passes of the 3–4 chamfer distance.
        let (w, h) = (self.w, self.h);
        let mut d = vec![0u32; w * h];
        for y in 0..h {
            for x in 0..w {
                if !self.at(x, y) {
                    continue;
                }
                let mut best = u32::MAX / 2;
                if x > 0 && y > 0 {
                    best = best.min(d[(y - 1) * w + x - 1] + 4);
                }
                if y > 0 {
                    best = best.min(d[(y - 1) * w + x] + 3);
                    if x + 1 < w {
                        best = best.min(d[(y - 1) * w + x + 1] + 4);
                    }
                }
                if x > 0 {
                    best = best.min(d[y * w + x - 1] + 3);
                }
                d[y * w + x] = best;
            }
        }
        for y in (0..h).rev() {
            for x in (0..w).rev() {
                if !self.at(x, y) {
                    continue;
                }
                let mut best = d[y * w + x];
                if x + 1 < w && y + 1 < h {
                    best = best.min(d[(y + 1) * w + x + 1] + 4);
                }
                if y + 1 < h {
                    best = best.min(d[(y + 1) * w + x] + 3);
                    if x > 0 {
                        best = best.min(d[(y + 1) * w + x - 1] + 4);
                    }
                }
                if x + 1 < w {
                    best = best.min(d[y * w + x + 1] + 3);
                }
                d[y * w + x] = best;
            }
        }
        // Of the deepest cells (within a cell of the deepest), the one
        // nearest the middle of them all: the middle of a long shape, not
        // an end of it.
        let deepest = *d.iter().max()?;
        if deepest == 0 {
            return None;
        }
        let near = |v: u32| v + 3 >= deepest;
        let (mut sx, mut sy, mut n) = (0.0f64, 0.0f64, 0.0f64);
        for y in 0..h {
            for x in 0..w {
                if near(d[y * w + x]) {
                    sx += x as f64;
                    sy += y as f64;
                    n += 1.0;
                }
            }
        }
        let (mx, my) = (sx / n, sy / n);
        let mut best = None::<(f64, usize, usize)>;
        for y in 0..h {
            for x in 0..w {
                if near(d[y * w + x]) {
                    let dd = (x as f64 - mx).powi(2) + (y as f64 - my).powi(2);
                    if best.is_none_or(|(b, _, _)| dd < b) {
                        best = Some((dd, x, y));
                    }
                }
            }
        }
        best.map(|(_, x, y)| {
            [
                self.x0 + (x as f32 + 0.5) * CELL_MM,
                self.y0 + (y as f32 + 0.5) * CELL_MM,
            ]
        })
    }

    /// The border of the covered cells as closed loops, counterclockwise
    /// round what they hold, in cell corners.
    fn loops(&self) -> Vec<Vec<(i32, i32)>> {
        use std::collections::HashMap;
        // Each covered cell's side that faces a free one, its covered cell
        // on its left.
        let mut from: HashMap<(i32, i32), Vec<(i32, i32)>> = HashMap::new();
        let covered = |x: i32, y: i32| {
            x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h && {
                self.at(x as usize, y as usize)
            }
        };
        for y in 0..self.h as i32 {
            for x in 0..self.w as i32 {
                if !covered(x, y) {
                    continue;
                }
                if !covered(x, y - 1) {
                    from.entry((x, y)).or_default().push((x + 1, y));
                }
                if !covered(x + 1, y) {
                    from.entry((x + 1, y)).or_default().push((x + 1, y + 1));
                }
                if !covered(x, y + 1) {
                    from.entry((x + 1, y + 1)).or_default().push((x, y + 1));
                }
                if !covered(x - 1, y) {
                    from.entry((x, y + 1)).or_default().push((x, y));
                }
            }
        }
        let mut loops = Vec::new();
        let mut starts: Vec<(i32, i32)> = from.keys().copied().collect();
        starts.sort_unstable();
        for start in starts {
            while from.get(&start).is_some_and(|v| !v.is_empty()) {
                let mut ring = vec![start];
                let mut at = start;
                let mut came = (0, 0);
                loop {
                    let Some(outs) = from.get_mut(&at) else {
                        break;
                    };
                    if outs.is_empty() {
                        break;
                    }
                    // Where two cells touch at a corner, turn left: each
                    // stays a loop of its own.
                    let pick = if outs.len() == 1 {
                        0
                    } else {
                        let left = (-came.1, came.0);
                        outs.iter()
                            .position(|&n| (n.0 - at.0, n.1 - at.1) == left)
                            .unwrap_or(0)
                    };
                    let next = outs.swap_remove(pick);
                    came = (next.0 - at.0, next.1 - at.1);
                    at = next;
                    if at == start {
                        break;
                    }
                    ring.push(at);
                }
                if ring.len() >= 4 {
                    loops.push(ring);
                }
            }
        }
        loops
    }
}

fn neighbours(x: usize, y: usize, w: usize, h: usize) -> impl Iterator<Item = (usize, usize)> {
    [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)]
        .into_iter()
        .filter_map(move |(dx, dy)| {
            let (nx, ny) = (x as i32 + dx, y as i32 + dy);
            (nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h)
                .then_some((nx as usize, ny as usize))
        })
}

/// The part of `t` at or below `top`, flat: the corners of the triangle cut
/// by that plane, seen from above (none when all of it is above).
fn below(t: &Triangle, top: f32) -> Vec<[f32; 2]> {
    let mut out = Vec::with_capacity(4);
    for i in 0..3 {
        let (p, q) = (t[i], t[(i + 1) % 3]);
        let (pin, qin) = (p[2] <= top, q[2] <= top);
        if pin {
            out.push([p[0], p[1]]);
        }
        if pin != qin {
            let s = (top - p[2]) / (q[2] - p[2]);
            out.push([p[0] + (q[0] - p[0]) * s, p[1] + (q[1] - p[1]) * s]);
        }
    }
    out
}

/// The shape `triangles` take seen from above, or, with `band` set, the
/// shape of what of them lies within that many mm of their lowest point.
pub(crate) fn shape_of(triangles: &[Triangle], band: Option<f32>) -> Shape {
    let Some(lowest) = triangles
        .iter()
        .flat_map(|t| t.iter().map(|p| p[2]))
        .reduce(f32::min)
    else {
        return Shape::default();
    };
    let top = band.map(|b| lowest + b);
    let mut flat: Vec<[[f32; 2]; 3]> = Vec::new();
    for t in triangles {
        match top {
            None => flat.push(t.map(|p| [p[0], p[1]])),
            Some(top) => {
                let poly = below(t, top);
                for i in 1..poly.len().saturating_sub(1) {
                    flat.push([poly[0], poly[i], poly[i + 1]]);
                }
            }
        }
    }
    let Some((min, max)) = flat
        .iter()
        .flatten()
        .fold(None::<([f32; 2], [f32; 2])>, |acc, p| {
            Some(match acc {
                None => (*p, *p),
                Some((lo, hi)) => (
                    [f32::min(lo[0], p[0]), f32::min(lo[1], p[1])],
                    [f32::max(hi[0], p[0]), f32::max(hi[1], p[1])],
                ),
            })
        })
    else {
        return Shape::default();
    };
    let mut mask = Mask::over(min, max);
    for t in &flat {
        mask.cover(*t);
    }
    shape_of_mask(&mask.dilated().filled().eroded())
}

fn shape_of_mask(mask: &Mask) -> Shape {
    let cells = mask.count();
    if cells == 0 {
        return Shape::default();
    }
    let to_mm = |(x, y): (i32, i32)| [mask.x0 + x as f32 * CELL_MM, mask.y0 + y as f32 * CELL_MM];
    let outlines = mask
        .loops()
        .into_iter()
        .map(|ring| {
            let points: Vec<[f32; 2]> = ring.into_iter().map(to_mm).collect();
            smooth(&simplify(&points, CELL_MM * 0.75))
        })
        .filter(|o| o.len() >= 3)
        .collect();
    Shape {
        outlines,
        label_at: mask.deepest(),
        area_mm2: cells as f32 * CELL_MM * CELL_MM,
    }
}

/// `ring` (closed) with the points within `tolerance` of the line between
/// their neighbours left out (Douglas–Peucker, on each half).
fn simplify(ring: &[[f32; 2]], tolerance: f32) -> Vec<[f32; 2]> {
    if ring.len() < 4 {
        return ring.to_vec();
    }
    // Split at the point farthest from the first, so both halves are open.
    let far = (1..ring.len())
        .max_by(|&a, &b| {
            dist2(ring[0], ring[a])
                .partial_cmp(&dist2(ring[0], ring[b]))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(ring.len() / 2);
    let mut first: Vec<[f32; 2]> = ring[..=far].to_vec();
    let mut second: Vec<[f32; 2]> = ring[far..].to_vec();
    second.push(ring[0]);
    first = douglas_peucker(&first, tolerance);
    second = douglas_peucker(&second, tolerance);
    first.pop();
    second.pop();
    first.extend(second);
    first
}

fn douglas_peucker(line: &[[f32; 2]], tolerance: f32) -> Vec<[f32; 2]> {
    if line.len() < 3 {
        return line.to_vec();
    }
    let mut keep = vec![false; line.len()];
    keep[0] = true;
    keep[line.len() - 1] = true;
    let mut stack = vec![(0, line.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        let mut worst = (0.0f32, a);
        for i in a + 1..b {
            let d = off_line(line[i], line[a], line[b]);
            if d > worst.0 {
                worst = (d, i);
            }
        }
        if worst.0 > tolerance {
            keep[worst.1] = true;
            stack.push((a, worst.1));
            stack.push((worst.1, b));
        }
    }
    line.iter()
        .zip(keep)
        .filter_map(|(p, k)| k.then_some(*p))
        .collect()
}

fn dist2(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)
}

fn off_line(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-9 {
        return dist2(p, a).sqrt();
    }
    ((p[0] - a[0]) * dy - (p[1] - a[1]) * dx).abs() / len
}

/// `ring` (closed) with its corners rounded: two passes of Chaikin's
/// corner cutting.
fn smooth(ring: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let mut ring = ring.to_vec();
    for _ in 0..2 {
        if ring.len() < 3 {
            break;
        }
        let n = ring.len();
        let mut out = Vec::with_capacity(n * 2);
        for i in 0..n {
            let (p, q) = (ring[i], ring[(i + 1) % n]);
            out.push([0.75 * p[0] + 0.25 * q[0], 0.75 * p[1] + 0.25 * q[1]]);
            out.push([0.25 * p[0] + 0.75 * q[0], 0.25 * p[1] + 0.75 * q[1]]);
        }
        ring = out;
    }
    ring
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box from `lo` to `hi`, as its 12 triangles.
    fn cube(lo: [f32; 3], hi: [f32; 3]) -> Vec<Triangle> {
        let c = |i: usize| {
            [
                if i & 1 == 0 { lo[0] } else { hi[0] },
                if i & 2 == 0 { lo[1] } else { hi[1] },
                if i & 4 == 0 { lo[2] } else { hi[2] },
            ]
        };
        let quads = [
            [0, 1, 3, 2],
            [4, 6, 7, 5],
            [0, 4, 5, 1],
            [2, 3, 7, 6],
            [0, 2, 6, 4],
            [1, 5, 7, 3],
        ];
        quads
            .iter()
            .flat_map(|q| [[c(q[0]), c(q[1]), c(q[2])], [c(q[0]), c(q[2]), c(q[3])]])
            .collect()
    }

    fn area(outline: &Outline) -> f32 {
        let n = outline.len();
        (0..n)
            .map(|i| {
                let (p, q) = (outline[i], outline[(i + 1) % n]);
                p[0] * q[1] - q[0] * p[1]
            })
            .sum::<f32>()
            / 2.0
    }

    /// A box seen from above is its square, counterclockwise, with its
    /// number in the middle.
    #[test]
    fn a_box_from_above_is_its_square() {
        let shape = shape_of(&cube([10.0, 20.0, 0.0], [30.0, 30.0, 5.0]), None);
        assert_eq!(shape.outlines.len(), 1);
        let drawn = area(&shape.outlines[0]);
        // The corners come out rounded.
        assert!((drawn - 200.0).abs() < 10.0, "drawn {drawn}");
        assert!((shape.area_mm2 - 200.0).abs() < 6.0, "{}", shape.area_mm2);
        let at = shape.label_at.unwrap();
        assert!(
            (at[0] - 20.0).abs() < 1.0 && (at[1] - 25.0).abs() < 1.0,
            "{at:?}"
        );
    }

    /// The footprint reads only what stands on the plate: a mushroom (thin
    /// stem under a wide cap) leaves its stem's square, from above its cap.
    #[test]
    fn the_footprint_is_what_stands_on_the_plate() {
        let mut mushroom = cube([-1.0, -1.0, 0.0], [1.0, 1.0, 10.0]);
        mushroom.extend(cube([-5.0, -5.0, 10.0], [5.0, 5.0, 12.0]));
        let foot = shape_of(&mushroom, Some(BASE_BAND_MM));
        let top = shape_of(&mushroom, None);
        assert!((foot.area_mm2 - 4.0).abs() < 1.5, "{}", foot.area_mm2);
        assert!((top.area_mm2 - 100.0).abs() < 4.0, "{}", top.area_mm2);
    }

    /// Two boxes apart are two outlines; a ring keeps no hole.
    #[test]
    fn parts_apart_are_outlines_of_their_own_and_holes_fill() {
        let mut two = cube([0.0, 0.0, 0.0], [5.0, 5.0, 1.0]);
        two.extend(cube([10.0, 0.0, 0.0], [15.0, 5.0, 1.0]));
        assert_eq!(shape_of(&two, None).outlines.len(), 2);
        let mut ring = cube([0.0, 0.0, 0.0], [10.0, 2.0, 1.0]);
        ring.extend(cube([0.0, 8.0, 0.0], [10.0, 10.0, 1.0]));
        ring.extend(cube([0.0, 2.0, 0.0], [2.0, 8.0, 1.0]));
        ring.extend(cube([8.0, 2.0, 0.0], [10.0, 8.0, 1.0]));
        let shape = shape_of(&ring, None);
        assert_eq!(shape.outlines.len(), 1);
        assert!((shape.area_mm2 - 100.0).abs() < 4.0, "{}", shape.area_mm2);
    }

    /// A model made of many triangles smaller than a cell still reads whole.
    #[test]
    fn tiny_triangles_still_read_whole() {
        let mut tris = Vec::new();
        let step = 0.05;
        for i in 0..100 {
            for j in 0..100 {
                let (x, y) = (i as f32 * step, j as f32 * step);
                tris.push([[x, y, 0.0], [x + step, y, 0.0], [x + step, y + step, 0.0]]);
                tris.push([[x, y, 0.0], [x + step, y + step, 0.0], [x, y + step, 0.0]]);
            }
        }
        let shape = shape_of(&tris, None);
        assert_eq!(shape.outlines.len(), 1);
        assert!((shape.area_mm2 - 25.0).abs() < 2.0, "{}", shape.area_mm2);
    }

    #[test]
    fn nothing_is_an_empty_shape() {
        assert!(shape_of(&[], None).is_empty());
    }
}
