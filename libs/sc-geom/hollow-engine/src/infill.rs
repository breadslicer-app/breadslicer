//! Beams across a cavity: a lattice of four diagonals or vertical pillars,
//! each a closed round tube where its line runs through carved cubes.
//!
//! The lines lie on a square grid across each direction, through the
//! model's centre. Each is walked cube by cube (the cubes it actually
//! passes through, in order); every stretch through carved cubes becomes a
//! tube, sunk a little into the wall at both ends so it joins it, unless it
//! is too short to brace anything.

use rayon::prelude::*;

use crate::grid::GridSpec;
use crate::mesh::{Aabb, IndexedMesh, Vec3};

/// How the beams cross the cavity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfillMode {
    /// The four diagonals of a cube.
    Lattice,
    /// Straight up (in the model's frame).
    Pillar,
}

impl InfillMode {
    fn directions(self) -> &'static [[f32; 3]] {
        match self {
            InfillMode::Lattice => &[
                [1.0, 1.0, 1.0],
                [1.0, 1.0, -1.0],
                [1.0, -1.0, 1.0],
                [1.0, -1.0, -1.0],
            ],
            InfillMode::Pillar => &[[0.0, 0.0, 1.0]],
        }
    }
}

/// The beams of `pattern` through the cubes `carved` says are cavity, on a
/// grid of `cell_mm` (3–24), `radius_mm` thick (0.25–3).
pub(crate) fn infill_beams(
    grid: &GridSpec,
    bbox: &Aabb,
    carved: &(dyn Fn(usize) -> bool + Sync),
    pattern: InfillMode,
    cell_mm: f32,
    radius_mm: f32,
) -> IndexedMesh {
    let spacing = cell_mm.clamp(3.0, 24.0);
    let radius = radius_mm.clamp(0.25, 3.0);
    // Into the wall at each end, enough to join it.
    let sink = (radius * 0.8).max(grid.voxel_mm * 0.8);
    let shortest = spacing * 0.45;
    let sides = ((std::f32::consts::TAU * radius / 0.45).ceil() as usize).clamp(14, 56);
    let centre = bbox.center();
    let half = (bbox.max - bbox.min) * 0.5;
    let reach = half.length() + spacing * 1.5;

    let mut lines = Vec::new();
    for d in pattern.directions() {
        let axis = Vec3::new(d[0], d[1], d[2])
            .normalized()
            .expect("a direction");
        let (u, v) = axis.frame();
        let across =
            |w: Vec3| half.x * w.x.abs() + half.y * w.y.abs() + half.z * w.z.abs() + spacing;
        let (su, sv) = (across(u), across(v));
        let (nu, nv) = (
            ((su * 2.0) / spacing).ceil() as usize,
            ((sv * 2.0) / spacing).ceil() as usize,
        );
        for i in 0..=nu {
            for j in 0..=nv {
                let start = centre + u * (i as f32 * spacing - su) + v * (j as f32 * spacing - sv)
                    - axis * reach;
                lines.push((start, axis));
            }
        }
    }
    let tubes: Vec<IndexedMesh> = lines
        .par_iter()
        .flat_map_iter(|&(start, axis)| {
            carved_stretches(grid, carved, start, axis, reach * 2.0)
                .into_iter()
                .filter_map(move |(a, b)| {
                    let (a, b) = ((a - sink).max(0.0), b + sink);
                    (b - a >= shortest).then(|| tube(start + axis * a, axis, b - a, radius, sides))
                })
        })
        .collect();
    tubes
        .iter()
        .fold(IndexedMesh::default(), |all, t| all.joined(t))
}

/// The stretches `[from, to]` (in mm along `axis` from `start`, up to
/// `length`) where the line runs through carved cubes, found by stepping
/// from cube to cube where the line crosses their faces.
fn carved_stretches(
    grid: &GridSpec,
    carved: &(dyn Fn(usize) -> bool + Sync),
    start: Vec3,
    axis: Vec3,
    length: f32,
) -> Vec<(f32, f32)> {
    let v = grid.voxel_mm;
    let o = [
        (start.x - grid.min.x) / v,
        (start.y - grid.min.y) / v,
        (start.z - grid.min.z) / v,
    ];
    let d = axis.to_array();
    let dims = [grid.nx as isize, grid.ny as isize, grid.nz as isize];
    let mut cell = o.map(|c| c.floor() as isize);
    let step = d.map(|c| if c > 0.0 { 1isize } else { -1 });
    // Distance along the line (in voxels) to the next face on each axis,
    // and between faces.
    let mut next = [0usize, 1, 2].map(|k| {
        if d[k] == 0.0 {
            f32::INFINITY
        } else {
            let face = cell[k] as f32 + if d[k] > 0.0 { 1.0 } else { 0.0 };
            (face - o[k]) / d[k]
        }
    });
    let gap = d.map(|c| {
        if c == 0.0 {
            f32::INFINITY
        } else {
            1.0 / c.abs()
        }
    });
    let end = length / v;
    let mut t = 0.0f32;
    let mut open: Option<f32> = None;
    let mut out = Vec::new();
    while t < end {
        let inside = (0..3).all(|k| (0..dims[k]).contains(&cell[k]));
        let here = inside && carved(grid.idx(cell[0] as usize, cell[1] as usize, cell[2] as usize));
        let k = (0..3)
            .min_by(|&a, &b| next[a].total_cmp(&next[b]))
            .expect("three axes");
        let leave = next[k].min(end);
        match (here, open) {
            (true, None) => open = Some(t),
            (false, Some(from)) => {
                out.push((from * v, t * v));
                open = None;
            }
            _ => {}
        }
        t = leave;
        cell[k] += step[k];
        next[k] += gap[k];
    }
    if let Some(from) = open {
        out.push((from * v, t * v));
    }
    out
}

/// A closed round tube from `from` along the unit `axis` for `length`, with
/// `sides` sides and flat ends, facing out.
fn tube(from: Vec3, axis: Vec3, length: f32, radius: f32, sides: usize) -> IndexedMesh {
    let (u, v) = axis.frame();
    let to = from + axis * length;
    let mut positions = vec![from, to];
    for s in 0..sides {
        let a = s as f32 / sides as f32 * std::f32::consts::TAU;
        let out = u * (a.cos() * radius) + v * (a.sin() * radius);
        positions.push(from + out);
        positions.push(to + out);
    }
    let ring = |s: usize| 2 + 2 * (s % sides) as u32;
    let mut triangles = Vec::with_capacity(sides * 4);
    for s in 0..sides {
        let (b0, b1) = (ring(s), ring(s + 1));
        let (t0, t1) = (b0 + 1, b1 + 1);
        triangles.push([0, b1, b0]);
        triangles.push([1, t0, t1]);
        triangles.push([b0, b1, t1]);
        triangles.push([b0, t1, t0]);
    }
    IndexedMesh::new(positions, triangles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::is_edge_manifold;

    #[test]
    fn a_tube_is_closed_and_holds_its_volume() {
        let t = tube(Vec3::ZERO, Vec3::new(0.0, 0.6, 0.8), 10.0, 1.0, 48);
        assert!(is_edge_manifold(&t));
        let want = std::f64::consts::PI * 10.0;
        assert!(
            (t.signed_volume() - want).abs() < want * 0.01,
            "{}",
            t.signed_volume()
        );
    }

    /// A line through a box of carved cubes is carved exactly across it.
    #[test]
    fn a_line_is_carved_where_it_crosses_carved_cubes() {
        let grid = GridSpec {
            nx: 10,
            ny: 10,
            nz: 10,
            voxel_mm: 1.0,
            min: Vec3::ZERO,
        };
        let carved = |i: usize| {
            let (x, y, z) = grid.coords(i);
            (3..7).contains(&x) && (3..7).contains(&y) && (3..7).contains(&z)
        };
        let s = carved_stretches(
            &grid,
            &carved,
            Vec3::new(-1.0, 5.5, 5.5),
            Vec3::new(1.0, 0.0, 0.0),
            12.0,
        );
        assert_eq!(s, vec![(4.0, 8.0)]);
    }
}
