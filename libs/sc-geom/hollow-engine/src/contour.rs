//! The surface where a field over the grid changes sign, as a closed mesh.
//!
//! Each cube between eight neighbouring centres is cut into six tetrahedra
//! around its main diagonal, the same way in every cube, so neighbouring
//! cubes cut their shared faces alike. Inside each tetrahedron the surface is
//! a triangle (one corner apart from three) or a quad (two from two), with
//! corners on the edges whose ends differ in sign. A grid edge's corner is
//! interpolated from its lower-numbered end, so every tetrahedron that uses
//! the edge puts it in the same place, and corners are numbered per edge: the
//! surface comes out indexed and, for a field that is never 0, closed.

use rayon::prelude::*;

use crate::grid::GridSpec;
use crate::mesh::{IndexedMesh, Vec3};

/// The six tetrahedra of a cube, by corner number `x + 2y + 4z`: one per
/// order the three axes can be stepped in, from corner 0 to corner 7.
const TETRAHEDRA: [[usize; 4]; 6] = [
    [0, 1, 3, 7],
    [0, 1, 5, 7],
    [0, 2, 3, 7],
    [0, 2, 6, 7],
    [0, 4, 5, 7],
    [0, 4, 6, 7],
];

/// A grid edge by its ends' cube numbers, lower first.
fn edge(a: usize, b: usize) -> u64 {
    let (lo, hi) = if a < b { (a, b) } else { (b, a) };
    ((lo as u64) << 32) | hi as u64
}

/// The surface where `field` is 0, facing toward its negative side. Only
/// cubes some corner of which `active` holds are looked at (the others hold
/// no surface the caller wants).
pub(crate) fn contour(grid: &GridSpec, field: &[f32], active: &[bool]) -> IndexedMesh {
    let (nx, ny) = (grid.nx, grid.ny);
    let cube = |x: usize, y: usize, z: usize| -> [usize; 8] {
        std::array::from_fn(|c| grid.idx(x + (c & 1), y + ((c >> 1) & 1), z + (c >> 2)))
    };
    // Each slab of cubes: its triangles as three edges each.
    let slabs: Vec<Vec<[u64; 3]>> = (0..grid.nz.saturating_sub(1))
        .into_par_iter()
        .map(|z| {
            let mut out = Vec::new();
            for y in 0..ny.saturating_sub(1) {
                for x in 0..nx.saturating_sub(1) {
                    let corners = cube(x, y, z);
                    if !corners.iter().any(|&i| active[i]) {
                        continue;
                    }
                    let negative = corners.map(|i| field[i] < 0.0);
                    if negative.iter().all(|&n| n) || negative.iter().all(|&n| !n) {
                        continue;
                    }
                    for tet in TETRAHEDRA {
                        cut_tetrahedron(grid, field, tet.map(|c| corners[c]), &mut out);
                    }
                }
            }
            out
        })
        .collect();
    let triangles: Vec<[u64; 3]> = slabs.concat();
    let mut edges: Vec<u64> = triangles.iter().flatten().copied().collect();
    edges.par_sort_unstable();
    edges.dedup();
    let positions = edges
        .par_iter()
        .map(|&e| {
            let (a, b) = ((e >> 32) as usize, (e & 0xffff_ffff) as usize);
            let (pa, pb) = (centre(grid, a), centre(grid, b));
            let (fa, fb) = (field[a], field[b]);
            pa + (pb - pa) * (fa / (fa - fb))
        })
        .collect();
    let number = |e: u64| edges.binary_search(&e).expect("every edge was listed") as u32;
    let triangles = triangles.par_iter().map(|t| t.map(number)).collect();
    IndexedMesh::new(positions, triangles)
}

fn centre(grid: &GridSpec, i: usize) -> Vec3 {
    let (x, y, z) = grid.coords(i);
    grid.center_world(x, y, z)
}

/// The surface inside one tetrahedron (corners as cube numbers), facing
/// toward its negative corners.
fn cut_tetrahedron(grid: &GridSpec, field: &[f32], tet: [usize; 4], out: &mut Vec<[u64; 3]>) {
    let (neg, pos): (Vec<usize>, Vec<usize>) = tet.iter().partition(|&&i| field[i] < 0.0);
    let at = |i: usize| centre(grid, i);
    let mean =
        |set: &[usize]| set.iter().fold(Vec3::ZERO, |s, &i| s + at(i)) * (1.0 / set.len() as f32);
    let (corners, quad): (Vec<u64>, bool) = match (neg.len(), pos.len()) {
        (1, 3) => (pos.iter().map(|&p| edge(neg[0], p)).collect(), false),
        (3, 1) => (neg.iter().map(|&n| edge(n, pos[0])).collect(), false),
        // Around the quad: each edge shares an end with the next.
        (2, 2) => (
            vec![
                edge(neg[0], pos[0]),
                edge(neg[0], pos[1]),
                edge(neg[1], pos[1]),
                edge(neg[1], pos[0]),
            ],
            true,
        ),
        _ => return,
    };
    let facing = mean(&neg) - mean(&pos);
    let point = |e: u64| {
        let (a, b) = ((e >> 32) as usize, (e & 0xffff_ffff) as usize);
        (at(a) + at(b)) * 0.5
    };
    // Orientation from the edges' midpoints: the same cyclic order the
    // interpolated corners take, which only slide along their edges.
    let oriented = |a: u64, b: u64, c: u64| {
        let normal = (point(b) - point(a)).cross(point(c) - point(a));
        if normal.dot(facing) >= 0.0 {
            [a, b, c]
        } else {
            [a, c, b]
        }
    };
    out.push(oriented(corners[0], corners[1], corners[2]));
    if quad {
        out.push(oriented(corners[0], corners[2], corners[3]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::is_edge_manifold;

    /// A ball of negative field contours closed, facing in, with about its
    /// volume.
    #[test]
    fn a_ball_contours_closed_and_facing_its_negative_side() {
        let grid = GridSpec {
            nx: 24,
            ny: 24,
            nz: 24,
            voxel_mm: 0.5,
            min: Vec3::ZERO,
        };
        let centre_mm = Vec3::new(6.0, 6.0, 6.0);
        let field: Vec<f32> = (0..grid.cell_count())
            .map(|i| {
                let (x, y, z) = grid.coords(i);
                let d = (grid.center_world(x, y, z) - centre_mm).length() - 4.0;
                if d == 0.0 { 1e-3 } else { d }
            })
            .collect();
        let active = vec![true; grid.cell_count()];
        let mesh = contour(&grid, &field, &active);
        assert!(is_edge_manifold(&mesh));
        // Facing the negative side (the ball's inside): a negative volume.
        let volume = -mesh.signed_volume();
        let want = 4.0 / 3.0 * std::f64::consts::PI * 64.0;
        assert!((volume - want).abs() < want * 0.03, "{volume} {want}");
    }
}
