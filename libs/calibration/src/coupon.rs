/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The XY coupon: printed flat on the plate, with no supports and no raft.
//!
//! Layout, in mm (X right, Y back, Z up):
//! - a base, 66 × 34 × 2, whose right end is a point: it shows +X. Every
//!   feature but the column stands on it, so its walls are read clear of the
//!   elephant's foot;
//! - a bar of blind holes, 0.6 to 4 mm: the thickest drill bit that goes in
//!   each one says how much they close;
//! - square blocks of 4, 8 and 16 mm, read in X and in Y: their sizes
//!   separate the scale from the offset of the walls;
//! - pins of 0.5, 1 and 2 mm: the thinnest that stands whole;
//! - two combs whose slits, 0.2 to 0.6 mm, run across X and across Y: the
//!   thinnest still open;
//! - a column of 8 × 8 × 5 on the plate beside the base: its width at the
//!   bottom against its width near the top is the elephant's foot.
//!
//! Each feature is a closed shell of its own; shells that sit on the base
//! start inside it ([`SINK_MM`]) and never share a vertex with it, so the
//! piece reads the same once written to STL and welded back. There are few
//! of them, and few pins: the piece must not read as a pre-supported model
//! when it is imported.

/// The base's thickness.
pub const BASE_MM: f32 = 2.0;
/// How far features start inside the base, so the shells overlap.
pub const SINK_MM: f32 = 0.2;
/// The top of the blocks, holes and slits.
pub const TOP_MM: f32 = BASE_MM + 3.0;
/// The base's size, its point included.
pub const BASE_SIZE_MM: [f32; 2] = [66.0, 34.0];
/// The base's outline, seen from above: a point at +X.
pub const BASE_OUTLINE: [[f32; 2]; 5] = [
    [0.0, 0.0],
    [61.0, 0.0],
    [66.0, 17.0],
    [61.0, 34.0],
    [0.0, 34.0],
];

/// The blocks' sides, in the order the reading steps ask for them.
pub const BLOCKS_MM: [f32; 3] = [16.0, 8.0, 4.0];
/// The blind holes' diameters, left to right.
pub const HOLES_MM: [f32; 7] = [0.6, 0.8, 1.0, 1.5, 2.0, 3.0, 4.0];
/// The pins' diameters, left to right.
pub const PINS_MM: [f32; 3] = [0.5, 1.0, 2.0];
/// The slits' widths, thinnest first.
pub const SLITS_MM: [f32; 4] = [0.2, 0.3, 0.4, 0.6];
/// The column's side and height.
pub const COLUMN_MM: [f32; 2] = [8.0, 5.0];

/// Segments of a circle.
const SEGMENTS: usize = 48;
/// A comb's tooth: its width across the slits and its length along them.
const TOOTH_MM: [f32; 2] = [2.0, 6.0];
/// The back of a comb, holding its teeth together.
const SPINE_MM: f32 = 2.0;
/// The bar of holes: lower-left corner and size.
pub const BAR_AT: [f32; 2] = [1.0, 26.0];
pub const BAR_SIZE: [f32; 2] = [42.0, 6.0];

/// A triangle mesh: vertices and the faces that index them, wound
/// counter-clockwise seen from outside.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub vertices: Vec<[f32; 3]>,
    pub faces: Vec<[u32; 3]>,
}

fn signed_area(ring: &[[f32; 2]]) -> f32 {
    let n = ring.len();
    (0..n)
        .map(|i| {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f32>()
        / 2.0
}

/// `ring` counter-clockwise seen from above.
fn ccw(mut ring: Vec<[f32; 2]>) -> Vec<[f32; 2]> {
    if signed_area(&ring) < 0.0 {
        ring.reverse();
    }
    ring
}

impl Mesh {
    /// Adds a solid over the `outline` with `holes` cut through it, from
    /// `z0` up to `z1`. The outline goes counter-clockwise seen from above,
    /// the holes clockwise.
    fn extrude(&mut self, outline: &[[f32; 2]], holes: &[Vec<[f32; 2]>], z0: f32, z1: f32) {
        let mut flat: Vec<f64> = Vec::new();
        let mut hole_starts = Vec::new();
        let mut rings: Vec<&[[f32; 2]]> = vec![outline];
        rings.extend(holes.iter().map(Vec::as_slice));
        for (k, ring) in rings.iter().enumerate() {
            if k > 0 {
                hole_starts.push(flat.len() / 2);
            }
            for p in ring.iter() {
                flat.push(f64::from(p[0]));
                flat.push(f64::from(p[1]));
            }
        }
        let points = flat.len() / 2;
        let first = self.vertices.len() as u32;
        for z in [z0, z1] {
            for p in flat.chunks(2) {
                self.vertices.push([p[0] as f32, p[1] as f32, z]);
            }
        }
        let (bottom, top) = (first, first + points as u32);
        let triangles = earcutr::earcut(&flat, &hole_starts, 2).unwrap_or_default();
        for t in triangles.chunks(3) {
            let (pa, pb, pc) = (
                [flat[2 * t[0]], flat[2 * t[0] + 1]],
                [flat[2 * t[1]], flat[2 * t[1] + 1]],
                [flat[2 * t[2]], flat[2 * t[2] + 1]],
            );
            let up = (pb[0] - pa[0]) * (pc[1] - pa[1]) - (pb[1] - pa[1]) * (pc[0] - pa[0]) > 0.0;
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| i as u32);
            let (b, c) = if up { (b, c) } else { (c, b) };
            self.faces.push([top + a, top + b, top + c]);
            self.faces.push([bottom + a, bottom + c, bottom + b]);
        }
        let mut start = 0u32;
        for ring in &rings {
            let n = ring.len() as u32;
            for i in 0..n {
                let (i, j) = (start + i, start + (i + 1) % n);
                self.faces.push([bottom + i, bottom + j, top + j]);
                self.faces.push([bottom + i, top + j, top + i]);
            }
            start += n;
        }
    }

    fn cuboid(&mut self, min: [f32; 2], max: [f32; 2], z0: f32, z1: f32) {
        let outline = [
            [min[0], min[1]],
            [max[0], min[1]],
            [max[0], max[1]],
            [min[0], max[1]],
        ];
        self.extrude(&outline, &[], z0, z1);
    }

    /// Whether every edge of the mesh is shared by exactly two faces wound
    /// against each other.
    pub fn is_closed(&self) -> bool {
        let mut edges = std::collections::HashMap::<(u32, u32), (i32, u32)>::new();
        for f in &self.faces {
            for k in 0..3 {
                let (a, b) = (f[k], f[(k + 1) % 3]);
                let e = edges.entry((a.min(b), a.max(b))).or_default();
                e.0 += if a < b { 1 } else { -1 };
                e.1 += 1;
            }
        }
        edges.values().all(|&(s, u)| s == 0 && u == 2)
    }

    /// How many shells the mesh holds: faces joined through their vertices.
    pub fn shells(&self) -> usize {
        fn root(parent: &mut [u32], mut v: u32) -> u32 {
            while parent[v as usize] != v {
                parent[v as usize] = parent[parent[v as usize] as usize];
                v = parent[v as usize];
            }
            v
        }
        let mut parent: Vec<u32> = (0..self.vertices.len() as u32).collect();
        for f in &self.faces {
            let a = root(&mut parent, f[0]);
            for &v in &f[1..] {
                let b = root(&mut parent, v);
                parent[b as usize] = a;
            }
        }
        let mut roots = std::collections::HashSet::new();
        for f in &self.faces {
            roots.insert(root(&mut parent, f[0]));
        }
        roots.len()
    }
}

/// A part of the coupon a step of the reading asks about, where it lies on
/// the base (mm, seen from above).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Feature {
    /// A square block: its lower-left corner and side.
    Block { at: [f32; 2], side: f32 },
    /// A blind hole: its centre and diameter.
    Hole { at: [f32; 2], diameter: f32 },
    /// A pin: its centre and diameter.
    Pin { at: [f32; 2], diameter: f32 },
    /// A slit between two teeth: its lower-left corner, its size.
    Slit { at: [f32; 2], size: [f32; 2] },
    /// A comb's tooth, or its back.
    Tooth { at: [f32; 2], size: [f32; 2] },
    /// The column on the plate: its lower-left corner and side.
    Column { at: [f32; 2], side: f32 },
}

/// The XY coupon: its mesh and where each feature lies.
#[derive(Clone, Debug)]
pub struct Coupon {
    pub mesh: Mesh,
    pub blocks: Vec<Feature>,
    pub holes: Vec<Feature>,
    pub pins: Vec<Feature>,
    /// Slits across X (the gap runs along Y), thinnest first.
    pub slits_x: Vec<Feature>,
    /// Slits across Y (the gap runs along X), thinnest first.
    pub slits_y: Vec<Feature>,
    pub teeth: Vec<Feature>,
    pub column: Feature,
}

fn circle(at: [f32; 2], radius: f32) -> Vec<[f32; 2]> {
    (0..SEGMENTS)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / SEGMENTS as f32;
            [at[0] + radius * a.cos(), at[1] + radius * a.sin()]
        })
        .collect()
}

/// A rectangle's corner and size.
type Rect = ([f32; 2], [f32; 2]);

/// A comb in its own frame: teeth side by side along `u`, the slits between
/// them open at `v = 0`, held by a back past `v = TOOTH_MM[1]`. Returns its
/// outline, its slits and its teeth (the back last).
fn comb() -> (Vec<[f32; 2]>, Vec<Rect>, Vec<Rect>) {
    let [tooth, length] = TOOTH_MM;
    let mut outline = vec![[0.0, 0.0]];
    let mut slits = Vec::new();
    let mut teeth = Vec::new();
    let mut u = 0.0;
    for (k, &gap) in SLITS_MM.iter().chain(std::iter::once(&0.0)).enumerate() {
        teeth.push(([u, 0.0], [tooth, length]));
        u += tooth;
        outline.push([u, 0.0]);
        if k < SLITS_MM.len() {
            outline.push([u, length]);
            outline.push([u + gap, length]);
            outline.push([u + gap, 0.0]);
            slits.push(([u, 0.0], [gap, length]));
            u += gap;
        }
    }
    teeth.push(([0.0, length], [u, SPINE_MM]));
    outline.push([u, length + SPINE_MM]);
    outline.push([0.0, length + SPINE_MM]);
    (outline, slits, teeth)
}

impl Coupon {
    /// The XY coupon.
    pub fn xy() -> Self {
        let mut mesh = Mesh::default();
        let (z0, z1) = (BASE_MM - SINK_MM, TOP_MM);
        mesh.extrude(&BASE_OUTLINE, &[], 0.0, BASE_MM);

        // The bar of holes, in one piece.
        let mut holes = Vec::new();
        let mut cuts = Vec::new();
        for (k, &d) in HOLES_MM.iter().enumerate() {
            let at = [4.0 + 6.0 * k as f32, 29.0];
            let mut ring = circle(at, d / 2.0);
            ring.reverse();
            cuts.push(ring);
            holes.push(Feature::Hole { at, diameter: d });
        }
        let [bx, by] = BAR_AT;
        let [bw, bh] = BAR_SIZE;
        mesh.extrude(
            &[[bx, by], [bx + bw, by], [bx + bw, by + bh], [bx, by + bh]],
            &cuts,
            z0,
            z1,
        );

        let blocks: Vec<Feature> = BLOCKS_MM
            .iter()
            .map(|&side| {
                let at = match side as u32 {
                    16 => [44.0, 9.0],
                    8 => [33.0, 15.0],
                    _ => [26.0, 19.0],
                };
                Feature::Block { at, side }
            })
            .collect();
        for block in &blocks {
            if let Feature::Block { at, side } = *block {
                mesh.cuboid(at, [at[0] + side, at[1] + side], z0, z1);
            }
        }

        let mut pins = Vec::new();
        for (k, &d) in PINS_MM.iter().enumerate() {
            let at = [5.0 + 6.0 * k as f32, 13.0];
            mesh.extrude(&circle(at, d / 2.0), &[], z0, BASE_MM + 4.0);
            pins.push(Feature::Pin { at, diameter: d });
        }

        let (outline, slits, comb_teeth) = comb();
        let mut teeth = Vec::new();
        // Along X: the teeth side by side, the slits open towards the front.
        let along_x = |p: [f32; 2]| [2.0 + p[0], 2.0 + p[1]];
        mesh.extrude(
            &ccw(outline.iter().map(|&p| along_x(p)).collect()),
            &[],
            z0,
            z1,
        );
        let slits_x = slits
            .iter()
            .map(|&(at, size)| Feature::Slit {
                at: along_x(at),
                size,
            })
            .collect();
        teeth.extend(comb_teeth.iter().map(|&(at, size)| Feature::Tooth {
            at: along_x(at),
            size,
        }));
        // Along Y: stacked, open towards the left, a millimetre lower so the
        // two combs tell apart.
        let along_y = |p: [f32; 2]| [19.0 + p[1], 2.0 + p[0]];
        mesh.extrude(
            &ccw(outline.iter().map(|&p| along_y(p)).collect()),
            &[],
            z0,
            z1 - 1.0,
        );
        let slits_y = slits
            .iter()
            .map(|&(at, size)| Feature::Slit {
                at: along_y(at),
                size: [size[1], size[0]],
            })
            .collect();
        teeth.extend(comb_teeth.iter().map(|&(at, size)| Feature::Tooth {
            at: along_y(at),
            size: [size[1], size[0]],
        }));

        let [side, height] = COLUMN_MM;
        let at = [69.0, 13.0];
        let column = Feature::Column { at, side };
        mesh.cuboid(at, [at[0] + side, at[1] + side], 0.0, height);

        Coupon {
            mesh,
            blocks,
            holes,
            pins,
            slits_x,
            slits_y,
            teeth,
            column,
        }
    }

    /// The coupon's footprint on the plate: lower-left and upper-right corners.
    pub fn footprint(&self) -> ([f32; 2], [f32; 2]) {
        let mut min = [f32::MAX; 2];
        let mut max = [f32::MIN; 2];
        for v in &self.mesh.vertices {
            for k in 0..2 {
                min[k] = min[k].min(v[k]);
                max[k] = max[k].max(v[k]);
            }
        }
        (min, max)
    }

    /// The coupon as a binary STL, its name in the header.
    pub fn to_stl(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(84 + 50 * self.mesh.faces.len());
        let name = b"Breadslicer XY calibration coupon";
        let mut header = [b' '; 80];
        header[..name.len()].copy_from_slice(name);
        out.extend_from_slice(&header);
        out.extend_from_slice(&(self.mesh.faces.len() as u32).to_le_bytes());
        for f in &self.mesh.faces {
            let [a, b, c] = f.map(|i| self.mesh.vertices[i as usize]);
            let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let n = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2])
                .sqrt()
                .max(f32::MIN_POSITIVE);
            for value in n.map(|c| c / len).into_iter().chain(a).chain(b).chain(c) {
                out.extend_from_slice(&value.to_le_bytes());
            }
            out.extend_from_slice(&[0, 0]);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_coupon_is_closed_and_stands_on_the_plate() {
        let coupon = Coupon::xy();
        assert!(coupon.mesh.is_closed());
        let lowest = coupon
            .mesh
            .vertices
            .iter()
            .map(|v| v[2])
            .fold(f32::MAX, f32::min);
        assert_eq!(lowest, 0.0);
        let (min, max) = coupon.footprint();
        assert_eq!(min, [0.0, 0.0]);
        assert_eq!(max, [77.0, 34.0]);
    }

    #[test]
    fn the_coupon_holds_few_shells() {
        // A pre-supported model is read from 12 shells up.
        assert_eq!(Coupon::xy().mesh.shells(), 11);
    }

    #[test]
    fn no_two_shells_share_a_vertex() {
        // Welded back from an STL, a shared vertex would join two shells
        // into an edge of four faces.
        let coupon = Coupon::xy();
        let mut seen = std::collections::HashSet::new();
        for v in &coupon.mesh.vertices {
            assert!(seen.insert(v.map(f32::to_bits)), "{v:?} twice");
        }
    }

    #[test]
    fn the_slits_have_their_widths() {
        let coupon = Coupon::xy();
        for slits in [&coupon.slits_x, &coupon.slits_y] {
            let widths: Vec<f32> = slits
                .iter()
                .map(|s| match s {
                    Feature::Slit { size, .. } => size[0].min(size[1]),
                    _ => unreachable!(),
                })
                .collect();
            assert_eq!(widths, SLITS_MM);
        }
    }

    #[test]
    fn the_features_stay_apart_and_on_the_base() {
        let coupon = Coupon::xy();
        // Each feature's box, and which comb it belongs to (0 none).
        let mut boxes: Vec<(Rect, u8)> = Vec::new();
        for f in coupon.blocks.iter().chain(&coupon.pins) {
            boxes.push(match *f {
                Feature::Block { at, side } => ((at, [side, side]), 0),
                Feature::Pin { at, diameter } => {
                    let r = diameter / 2.0;
                    (([at[0] - r, at[1] - r], [diameter, diameter]), 0)
                }
                _ => unreachable!(),
            });
        }
        let half = coupon.teeth.len() / 2;
        for (k, f) in coupon.teeth.iter().enumerate() {
            if let Feature::Tooth { at, size } = *f {
                boxes.push(((at, size), 1 + u8::from(k >= half)));
            }
        }
        boxes.push(((BAR_AT, BAR_SIZE), 0));
        for ((at, size), _) in &boxes {
            assert!(at[0] >= 0.0 && at[1] >= 0.0);
            assert!(at[0] + size[0] <= 61.0 && at[1] + size[1] <= 34.0);
        }
        for (i, &((a, sa), ca)) in boxes.iter().enumerate() {
            for &((b, sb), cb) in &boxes[i + 1..] {
                if ca != 0 && ca == cb {
                    continue;
                }
                let apart = (a[0] - b[0] - sb[0])
                    .max(b[0] - a[0] - sa[0])
                    .max(a[1] - b[1] - sb[1])
                    .max(b[1] - a[1] - sa[1]);
                assert!(apart >= 1.0, "{a:?} and {b:?} are {apart} apart");
            }
        }
    }

    #[test]
    fn the_stl_holds_every_face() {
        let coupon = Coupon::xy();
        let stl = coupon.to_stl();
        assert_eq!(stl.len(), 84 + 50 * coupon.mesh.faces.len());
        let count = u32::from_le_bytes(stl[80..84].try_into().unwrap());
        assert_eq!(count as usize, coupon.mesh.faces.len());
    }
}
