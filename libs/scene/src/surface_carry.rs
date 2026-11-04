/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Paint that goes with a model when its mesh changes (#183): Repair can
//! rewrite many faces and Hollow adds a cavity, and the person's paint has to
//! land on the same surface of the new mesh.
//!
//! Each face of the new mesh takes the value of the nearest face of the old
//! one that looks the same way: nearest by the distance from its centroid to
//! the old triangle, with a similar normal, so the two sides of a thin wall
//! stay apart and the walls of a new cavity take nothing. A face flipped in
//! place, as Repair flips an inside-out one, still matches the face it lies on.
//!
//! Only the old faces around the paint are looked at: they go in a sparse grid
//! over the painted box, its cells about the size of the faces, and a new face
//! outside that box is left bare without looking further. [`CarriedTags`] says
//! how much painted area the new mesh kept: the caller tells the person what
//! was lost, never drops it unseen.

use std::sync::Arc;

use rayon::prelude::*;
use sc_mesh_core::indexed_mesh::IndexedMesh;

use crate::surface::{SurfaceTags, UNPAINTED};

/// How far apart two face normals may look and still be the same surface:
/// the cosine of 60°.
const SIMILAR_NORMAL: f32 = 0.5;
/// A cell's side, in mean face edges: a few faces to a cell.
const CELL_EDGES: f64 = 1.5;
/// A face whose box reaches over more cells than this goes only in the cells
/// it crosses: a flat base's long triangles would fill their whole box.
const MAX_BOX_CELLS: usize = 64;
/// How far from the old surface a new face may lie and still be it: this
/// share of the painted region's size, and never less than the floor, mm. A
/// repair moves vertices by far less; a surface further away is another one.
const SAME_SURFACE_SHARE: f32 = 0.01;
const SAME_SURFACE_FLOOR_MM: f32 = 0.25;
/// A new face this close to an old one, as a share of that distance, lies on
/// it: nothing else needs looking at.
const ON_THE_SURFACE_SHARE: f32 = 0.02;

type P = [f32; 3];

fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: P, b: P) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: P, b: P) -> P {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// The corners of `face` of `mesh`, when they are in range.
fn corners(mesh: &IndexedMesh, face: usize) -> Option<[P; 3]> {
    let vertices = mesh.vertices();
    let f = mesh.faces().get(face)?;
    Some([
        vertices.get(usize::from(f.vertices[0]))?.coords(),
        vertices.get(usize::from(f.vertices[1]))?.coords(),
        vertices.get(usize::from(f.vertices[2]))?.coords(),
    ])
}

fn unit_normal(t: &[P; 3]) -> Option<P> {
    let n = cross(sub(t[1], t[0]), sub(t[2], t[0]));
    let len = dot(n, n).sqrt();
    (len > 0.0 && len.is_finite()).then(|| [n[0] / len, n[1] / len, n[2] / len])
}

fn centroid(t: &[P; 3]) -> P {
    [
        (t[0][0] + t[1][0] + t[2][0]) / 3.0,
        (t[0][1] + t[1][1] + t[2][1]) / 3.0,
        (t[0][2] + t[1][2] + t[2][2]) / 3.0,
    ]
}

/// The squared distance from `p` to the triangle `t`, by its closest point
/// (Ericson, Real-Time Collision Detection, 5.1.5).
fn distance_sq(p: P, t: &[P; 3]) -> f32 {
    let (a, b, c) = (t[0], t[1], t[2]);
    let (ab, ac, ap) = (sub(b, a), sub(c, a), sub(p, a));
    let len_sq = |q: P| dot(sub(p, q), sub(p, q));
    let lerp = |from: P, along: P, s: f32| {
        [
            from[0] + along[0] * s,
            from[1] + along[1] * s,
            from[2] + along[2] * s,
        ]
    };
    let (d1, d2) = (dot(ab, ap), dot(ac, ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return len_sq(a);
    }
    let bp = sub(p, b);
    let (d3, d4) = (dot(ab, bp), dot(ac, bp));
    if d3 >= 0.0 && d4 <= d3 {
        return len_sq(b);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return len_sq(lerp(a, ab, d1 / (d1 - d3)));
    }
    let cp = sub(p, c);
    let (d5, d6) = (dot(ab, cp), dot(ac, cp));
    if d6 >= 0.0 && d5 <= d6 {
        return len_sq(c);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return len_sq(lerp(a, ac, d2 / (d2 - d6)));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return len_sq(lerp(b, sub(c, b), (d4 - d3) / ((d4 - d3) + (d5 - d6))));
    }
    let denom = 1.0 / (va + vb + vc);
    let on = [
        a[0] + ab[0] * vb * denom + ac[0] * vc * denom,
        a[1] + ab[1] * vb * denom + ac[1] * vc * denom,
        a[2] + ab[2] * vb * denom + ac[2] * vc * denom,
    ];
    len_sq(on)
}

/// An old face in the grid.
struct Candidate {
    triangle: [P; 3],
    normal: P,
    value: u8,
}

/// Old faces by the cells their boxes reach: `(cell, face)` pairs in cell
/// order, so a model's millions of tiny faces get cells of their size without
/// a table of all the empty ones around them.
struct Grid {
    lo: P,
    hi: P,
    cell: f32,
    /// The squared distance past which a new face is another surface.
    same_surface_sq: f32,
    dims: [usize; 3],
    entries: Vec<(u64, u32)>,
    faces: Vec<Candidate>,
}

impl Grid {
    fn cell_of(&self, p: P) -> [usize; 3] {
        std::array::from_fn(|k| {
            let at = ((p[k] - self.lo[k]) / self.cell) as isize;
            at.clamp(0, self.dims[k] as isize - 1) as usize
        })
    }

    fn key(&self, c: [usize; 3]) -> u64 {
        (c[2] as u64 * self.dims[1] as u64 + c[1] as u64) * self.dims[0] as u64 + c[0] as u64
    }

    fn holds(&self, p: P) -> bool {
        (0..3).all(|k| p[k] >= self.lo[k] && p[k] <= self.hi[k])
    }

    /// The cell range the box of `t` reaches.
    fn reach(&self, t: &[P; 3]) -> ([usize; 3], [usize; 3]) {
        let lo: P = std::array::from_fn(|k| t[0][k].min(t[1][k]).min(t[2][k]));
        let hi: P = std::array::from_fn(|k| t[0][k].max(t[1][k]).max(t[2][k]));
        (self.cell_of(lo), self.cell_of(hi))
    }

    fn build(lo: P, hi: P, faces: Vec<Candidate>) -> Self {
        let size: P = std::array::from_fn(|k| (hi[k] - lo[k]).max(1e-6));
        let edges: f64 = faces
            .par_iter()
            .map(|f| {
                let e = sub(f.triangle[1], f.triangle[0]);
                f64::from(dot(e, e).sqrt())
            })
            .sum();
        let mean_edge = (edges / faces.len().max(1) as f64 * CELL_EDGES) as f32;
        // No axis gets more cells than a key can count.
        let cell = mean_edge.max(size[0].max(size[1]).max(size[2]) / 1_000_000.0);
        let dims = std::array::from_fn(|k| ((size[k] / cell).ceil() as usize).max(1));
        let reach = (dot(size, size).sqrt() * SAME_SURFACE_SHARE).max(SAME_SURFACE_FLOOR_MM);
        let mut grid = Self {
            lo,
            hi,
            cell,
            same_surface_sq: reach * reach,
            dims,
            entries: Vec::new(),
            faces,
        };
        let mut entries: Vec<(u64, u32)> = (0..grid.faces.len())
            .into_par_iter()
            .with_min_len(1 << 10)
            .flat_map_iter(|face| {
                let (a, b) = grid.reach(&grid.faces[face].triangle);
                grid.cells_of(face, &a, &b)
                    .into_iter()
                    .map(move |cell| (cell, face as u32))
            })
            .collect();
        entries.par_sort_unstable();
        grid.entries = entries;
        grid
    }

    /// The keys of the cells of the box `a`..`b` that face `face` reaches.
    fn cells_of(&self, face: usize, a: &[usize; 3], b: &[usize; 3]) -> Vec<u64> {
        let mut cells = Vec::new();
        if box_cells(a, b) > MAX_BOX_CELLS {
            self.split_box(&self.faces[face].triangle, *a, *b, &mut cells);
            return cells;
        }
        for z in a[2]..=b[2] {
            for y in a[1]..=b[1] {
                for x in a[0]..=b[0] {
                    cells.push(self.key([x, y, z]));
                }
            }
        }
        cells
    }

    /// Halves the box `a`..`b` of cells down to the ones `triangle` can
    /// reach: a box is left out when its centre is further from the triangle
    /// than its half diagonal, so a long thin face costs what it crosses, not
    /// the whole of its bounding box.
    fn split_box(&self, triangle: &[P; 3], a: [usize; 3], b: [usize; 3], cells: &mut Vec<u64>) {
        let size: [f32; 3] = std::array::from_fn(|k| (b[k] - a[k] + 1) as f32 * self.cell);
        let centre: P =
            std::array::from_fn(|k| self.lo[k] + a[k] as f32 * self.cell + size[k] / 2.0);
        if distance_sq(centre, triangle) > dot(size, size) / 4.0 * 1.01 {
            return;
        }
        let longest = (0..3).max_by_key(|&k| b[k] - a[k]).unwrap_or(0);
        if a[longest] == b[longest] {
            cells.push(self.key(a));
            return;
        }
        let middle = (a[longest] + b[longest]) / 2;
        let (mut left, mut right) = (b, a);
        left[longest] = middle;
        right[longest] = middle + 1;
        self.split_box(triangle, a, left, cells);
        self.split_box(triangle, right, b, cells);
    }

    /// The squared distance from `p` to the box of cell `c`.
    fn cell_distance_sq(&self, c: [usize; 3], p: P) -> f32 {
        (0..3)
            .map(|k| {
                let lo = self.lo[k] + c[k] as f32 * self.cell;
                let d = (lo - p[k]).max(p[k] - (lo + self.cell)).max(0.0);
                d * d
            })
            .sum()
    }

    fn cell_items(&self, c: [usize; 3]) -> &[(u64, u32)] {
        let key = self.key(c);
        let first = self.entries.partition_point(|e| e.0 < key);
        let held = self.entries[first..].partition_point(|e| e.0 == key);
        &self.entries[first..first + held]
    }

    /// The value of the old face a new face at `p` with normal `n` lies on.
    fn value_at(&self, p: P, n: P) -> u8 {
        if !self.holds(p) {
            return UNPAINTED;
        }
        let touching = self.same_surface_sq * ON_THE_SURFACE_SHARE * ON_THE_SURFACE_SHARE;
        let mut similar: Option<(f32, u8)> = None;
        let mut flipped: Option<(f32, u8)> = None;
        let look = |faces: &[(u64, u32)],
                    similar: &mut Option<(f32, u8)>,
                    flipped: &mut Option<(f32, u8)>| {
            for &(_, f) in faces {
                let face = &self.faces[f as usize];
                let d = distance_sq(p, &face.triangle);
                let facing = dot(face.normal, n);
                if facing >= SIMILAR_NORMAL {
                    if similar.is_none_or(|(best, _)| d < best) {
                        *similar = Some((d, face.value));
                    }
                } else if facing <= -SIMILAR_NORMAL
                    && d <= touching
                    && flipped.is_none_or(|(best, _)| d < best)
                {
                    *flipped = Some((d, face.value));
                }
            }
        };
        let home = self.cell_of(p);
        look(self.cell_items(home), &mut similar, &mut flipped);
        if !similar.is_some_and(|(d, _)| d <= touching) {
            for dz in -1isize..=1 {
                for dy in -1isize..=1 {
                    for dx in -1isize..=1 {
                        if dx == 0 && dy == 0 && dz == 0 {
                            continue;
                        }
                        let at = [
                            home[0] as isize + dx,
                            home[1] as isize + dy,
                            home[2] as isize + dz,
                        ];
                        if !(0..3).all(|k| at[k] >= 0 && at[k] < self.dims[k] as isize) {
                            continue;
                        }
                        // Only the cells that can hold something nearer.
                        let cell = at.map(|v| v as usize);
                        let nearest = similar
                            .map_or(self.same_surface_sq, |(d, _)| d.min(self.same_surface_sq));
                        if self.cell_distance_sq(cell, p) < nearest {
                            look(self.cell_items(cell), &mut similar, &mut flipped);
                        }
                    }
                }
            }
        }
        match similar
            .filter(|(d, _)| *d <= self.same_surface_sq)
            .or(flipped)
        {
            Some((_, value)) => value,
            None => UNPAINTED,
        }
    }
}

/// How many cells the box `a`..`b` holds.
fn box_cells(a: &[usize; 3], b: &[usize; 3]) -> usize {
    (b[0] - a[0] + 1) * (b[1] - a[1] + 1) * (b[2] - a[2] + 1)
}

/// What became of a mask on a new mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct CarriedTags {
    /// The mask on the new mesh; `None` when no paint reached it.
    pub tags: Option<SurfaceTags>,
    /// The painted area before, in the meshes' own units, mm².
    pub area_before: f64,
    /// The painted area on the new mesh.
    pub area_after: f64,
}

impl CarriedTags {
    /// The painted area the new mesh did not keep.
    pub fn area_lost(&self) -> f64 {
        (self.area_before - self.area_after).max(0.0)
    }

    /// The share of the painted area the new mesh did not keep, 0 to 1.
    pub fn share_lost(&self) -> f64 {
        if self.area_before > 0.0 {
            self.area_lost() / self.area_before
        } else {
            0.0
        }
    }
}

/// The mask `tags` on the mesh `after`.
pub fn carry_tags(tags: &SurfaceTags, after: &Arc<IndexedMesh>) -> CarriedTags {
    let before = tags.mesh().as_ref();
    let area_before = tags.painted_area();
    let same_faces = before.faces() == after.faces() && before.vertices() == after.vertices();
    if Arc::ptr_eq(tags.mesh(), after) || same_faces {
        return CarriedTags {
            tags: SurfaceTags::new(after.clone(), tags.values().to_vec()),
            area_before,
            area_after: area_before,
        };
    }
    let values = tags.values();
    let Some(grid) = grid_around_paint(before, values) else {
        return CarriedTags {
            tags: None,
            area_before,
            area_after: 0.0,
        };
    };
    let carried: Vec<u8> = (0..after.faces().len())
        .into_par_iter()
        .with_min_len(1 << 12)
        .map(|face| {
            corners(after, face)
                .and_then(|t| Some((centroid(&t), unit_normal(&t)?)))
                .map_or(UNPAINTED, |(p, n)| grid.value_at(p, n))
        })
        .collect();
    let tags = SurfaceTags::new(after.clone(), carried).filter(|t| !t.is_blank());
    let area_after = tags.as_ref().map_or(0.0, SurfaceTags::painted_area);
    CarriedTags {
        tags,
        area_before,
        area_after,
    }
}

/// The grid of the faces of `mesh` around its painted ones; `None` when none
/// is painted.
fn grid_around_paint(mesh: &IndexedMesh, values: &[u8]) -> Option<Grid> {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    let mut longest = 0.0f32;
    for (face, _) in values.iter().enumerate().filter(|(_, v)| **v != UNPAINTED) {
        let Some(t) = corners(mesh, face) else {
            continue;
        };
        for (i, p) in t.iter().enumerate() {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
            let e = sub(t[(i + 1) % 3], *p);
            longest = longest.max(dot(e, e));
        }
    }
    if !lo[0].is_finite() {
        return None;
    }
    let margin = longest.sqrt().max(1e-4) * 2.0;
    for k in 0..3 {
        lo[k] -= margin;
        hi[k] += margin;
    }
    let faces: Vec<Candidate> = (0..mesh.faces().len())
        .into_par_iter()
        .with_min_len(1 << 12)
        .filter_map(|face| {
            let triangle = corners(mesh, face)?;
            let outside = (0..3).any(|k| {
                triangle.iter().all(|p| p[k] < lo[k]) || triangle.iter().all(|p| p[k] > hi[k])
            });
            if outside {
                return None;
            }
            Some(Candidate {
                triangle,
                normal: unit_normal(&triangle)?,
                value: *values.get(face)?,
            })
        })
        .collect();
    Some(Grid::build(lo, hi, faces))
}

#[cfg(test)]
mod tests {
    use sc_mesh_core::primitives::{IndexedTriangle, Vertex};

    use super::*;
    use crate::surface::PAINTED;

    /// A flat `n` × `n` grid of quads over `size` mm at height `z`, facing up
    /// or down.
    fn sheet(n: u32, size: f32, z: f32, up: bool) -> IndexedMesh {
        let mut vertices = Vec::new();
        for y in 0..=n {
            for x in 0..=n {
                let s = size / n as f32;
                vertices.push(Vertex::new([x as f32 * s, y as f32 * s, z]));
            }
        }
        let at = |x: u32, y: u32| y * (n + 1) + x;
        let mut faces = Vec::new();
        for y in 0..n {
            for x in 0..n {
                let (a, b, c, d) = (at(x, y), at(x + 1, y), at(x + 1, y + 1), at(x, y + 1));
                for t in [[a, b, c], [a, c, d]] {
                    let t = if up { t } else { [t[0], t[2], t[1]] };
                    faces.push(IndexedTriangle::from_raw_indices(t));
                }
            }
        }
        IndexedMesh {
            meta: None,
            vertices,
            faces,
        }
    }

    /// Paints the faces of `mesh` whose centroid has `x` under `limit`.
    fn paint_left(mesh: &Arc<IndexedMesh>, limit: f32) -> SurfaceTags {
        let values = (0..mesh.faces().len())
            .map(|f| {
                let t = corners(mesh, f).unwrap();
                if centroid(&t)[0] < limit {
                    PAINTED
                } else {
                    UNPAINTED
                }
            })
            .collect();
        SurfaceTags::new(mesh.clone(), values).unwrap()
    }

    #[test]
    fn the_same_faces_keep_their_values_and_lose_nothing() {
        let mesh = Arc::new(sheet(8, 8.0, 0.0, true));
        let tags = paint_left(&mesh, 3.0);
        let copy = Arc::new((*mesh).clone());
        let carried = carry_tags(&tags, &copy);
        let moved = carried.tags.as_ref().unwrap();
        assert!(Arc::ptr_eq(moved.mesh(), &copy));
        assert_eq!(moved.values(), tags.values());
        assert_eq!(carried.area_lost(), 0.0);
    }

    #[test]
    fn a_finer_mesh_of_the_same_surface_keeps_the_paint_where_it_was() {
        let mesh = Arc::new(sheet(8, 8.0, 0.0, true));
        let tags = paint_left(&mesh, 4.0);
        let finer = Arc::new(sheet(32, 8.0, 0.0, true));
        let carried = carry_tags(&tags, &finer);
        let moved = carried.tags.as_ref().unwrap();
        for (face, value) in moved.values().iter().enumerate() {
            let x = centroid(&corners(&finer, face).unwrap())[0];
            if x < 3.75 {
                assert_eq!(*value, PAINTED, "face at x {x}");
            } else if x > 4.25 {
                assert_eq!(*value, UNPAINTED, "face at x {x}");
            }
        }
        assert!(carried.share_lost() < 0.05, "{}", carried.share_lost());
    }

    #[test]
    fn a_coarser_mesh_does_not_read_as_lost_paint() {
        let mesh = Arc::new(sheet(32, 8.0, 0.0, true));
        let tags = paint_left(&mesh, 8.5);
        let coarse = Arc::new(sheet(4, 8.0, 0.0, true));
        let carried = carry_tags(&tags, &coarse);
        assert!(carried.share_lost() < 0.01, "{}", carried.share_lost());
    }

    #[test]
    fn the_far_side_of_a_thin_wall_takes_nothing() {
        // The top of a 0.2 mm wall is painted; the bottom looks the other way.
        let top = sheet(6, 6.0, 0.2, true);
        let bottom = sheet(6, 6.0, 0.0, false);
        let mut both = top.clone();
        let offset = both.vertices.len() as u32;
        both.vertices.extend(bottom.vertices.iter().copied());
        both.faces.extend(
            bottom
                .faces
                .iter()
                .map(|f| IndexedTriangle::from_raw_indices(f.vertices.map(|v| v.get() + offset))),
        );
        let mesh = Arc::new(both);
        let values: Vec<u8> = (0..mesh.faces().len())
            .map(|f| {
                if f < top.faces.len() {
                    PAINTED
                } else {
                    UNPAINTED
                }
            })
            .collect();
        let tags = SurfaceTags::new(mesh.clone(), values.clone()).unwrap();
        let twin = {
            // The same surfaces with one vertex nudged: not the same mesh.
            let mut m = (*mesh).clone();
            let p = m.vertices[0].coords();
            m.vertices[0] = Vertex::new([p[0] - 1e-4, p[1], p[2]]);
            Arc::new(m)
        };
        let carried = carry_tags(&tags, &twin);
        assert_eq!(carried.tags.unwrap().values(), values);
    }

    #[test]
    fn a_face_flipped_in_place_keeps_its_paint() {
        let mesh = Arc::new(sheet(4, 4.0, 0.0, true));
        let tags = paint_left(&mesh, 2.0);
        let mut flipped = (*mesh).clone();
        for face in &mut flipped.faces {
            let v = face.vertices;
            face.vertices = [v[0], v[2], v[1]];
        }
        let carried = carry_tags(&tags, &Arc::new(flipped));
        assert_eq!(carried.tags.unwrap().values(), tags.values());
    }

    #[test]
    fn a_huge_triangle_is_found_from_anywhere_on_it() {
        // Two triangles the size of the whole sheet, under paint made of
        // small faces around them: the grid is as fine as the small ones.
        let mut mesh = sheet(40, 40.0, 5.0, true);
        let base = mesh.vertices.len() as u32;
        mesh.vertices.extend(
            [
                [-200.0, -200.0, 0.0],
                [200.0, -200.0, 0.0],
                [0.0, 200.0, 0.0],
            ]
            .map(Vertex::new),
        );
        mesh.faces.push(IndexedTriangle::from_raw_indices([
            base,
            base + 1,
            base + 2,
        ]));
        let mesh = Arc::new(mesh);
        let mut values = vec![PAINTED; mesh.faces().len()];
        values[0] = UNPAINTED;
        let tags = SurfaceTags::new(mesh.clone(), values).unwrap();
        // The same surfaces, the big one cut in small faces.
        let fine =
            {
                let mut m = sheet(40, 40.0, 5.0, true);
                let far = sheet(16, 16.0, 0.0, true);
                let offset = m.vertices.len() as u32;
                m.vertices.extend(far.vertices.iter().copied());
                m.faces.extend(far.faces.iter().map(|f| {
                    IndexedTriangle::from_raw_indices(f.vertices.map(|v| v.get() + offset))
                }));
                Arc::new(m)
            };
        let carried = carry_tags(&tags, &fine).tags.unwrap();
        let sheet_faces = 40 * 40 * 2;
        assert!(
            carried.values()[sheet_faces..]
                .iter()
                .all(|v| *v == PAINTED),
            "the part over the huge triangle takes its paint"
        );
    }

    /// How long a whole model takes, from a binary STL: not run by default.
    ///
    /// ```text
    /// WOLVERINE_STL=<path> cargo test -p sc-scene --release --lib carry_timing -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "needs the STL named in WOLVERINE_STL"]
    fn carry_timing_on_a_heavy_model() {
        let Some(path) = std::env::var_os("WOLVERINE_STL") else {
            return;
        };
        let bytes = std::fs::read(path).unwrap();
        let count = u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize;
        let mut vertices = Vec::with_capacity(count * 3);
        let mut faces = Vec::with_capacity(count);
        for i in 0..count {
            let record = &bytes[84 + i * 50 + 12..84 + i * 50 + 48];
            for corner in record.as_chunks::<12>().0 {
                let f = |k: usize| f32::from_le_bytes(corner[k * 4..k * 4 + 4].try_into().unwrap());
                vertices.push(Vertex::new([f(0), f(1), f(2)]));
            }
            let base = (i * 3) as u32;
            faces.push(IndexedTriangle::from_raw_indices([
                base,
                base + 1,
                base + 2,
            ]));
        }
        let mesh = Arc::new(IndexedMesh {
            meta: None,
            vertices,
            faces,
        });
        let mut moved = (*mesh).clone();
        for v in &mut moved.vertices {
            let p = v.coords();
            *v = Vertex::new([p[0] + 1e-3, p[1], p[2]]);
        }
        let moved = Arc::new(moved);
        for share in [10usize, 2, 1] {
            let values: Vec<u8> = (0..count)
                .map(|f| {
                    if f < count / share {
                        PAINTED
                    } else {
                        UNPAINTED
                    }
                })
                .collect();
            let tags = SurfaceTags::new(mesh.clone(), values).unwrap();
            let started = std::time::Instant::now();
            let carried = carry_tags(&tags, &moved);
            println!(
                "{} caras pintadas: {:.0} ms, se perdió {:.3} %",
                tags.painted_faces(),
                started.elapsed().as_secs_f64() * 1e3,
                carried.share_lost() * 100.0
            );
        }
    }

    #[test]
    fn paint_with_nowhere_to_go_is_reported_lost() {
        let mesh = Arc::new(sheet(4, 4.0, 0.0, true));
        let tags = paint_left(&mesh, 2.0);
        let elsewhere = Arc::new(sheet(4, 4.0, 50.0, true));
        let carried = carry_tags(&tags, &elsewhere);
        assert!(carried.tags.is_none());
        assert_eq!(carried.area_after, 0.0);
        assert!((carried.share_lost() - 1.0).abs() < 1e-9);
    }
}
