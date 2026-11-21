//! The model as voxels: which cubes of the grid are inside it, and how far
//! each is, at least, from its surface.
//!
//! Inside is decided at each cube's centre by the winding number along the
//! grid's vertical lines: every triangle is laid flat on the plate, each
//! line through it gets the height it is crossed at and whether the model
//! is entered or left there, and a centre is inside when it was entered more
//! often than left below it. Overlapping parts are inside once; a sealed
//! pocket already in the model is outside. Crossings on a shared edge or
//! corner count once, by a fixed rule on the edge's direction.
//!
//! How far from the surface: the surface is sampled at half a voxel, the
//! cubes holding samples seed an exact distance transform, and each cube's
//! distance to the nearest seed bounds its distance to the surface from
//! both sides (see [`Volume::near`]); the wall then measures the exact
//! distance where it matters.

use rayon::prelude::*;

use crate::bvh::Bvh;
use crate::control::{Cancelled, Control, Stage};
use crate::distance::squared_distance_transform;
use crate::grid::GridSpec;
use crate::mesh::{Aabb, IndexedMesh};

/// How far, in voxels, a surface sample can be from the centre of the cube
/// that holds it (half a cube's diagonal), plus the samples' spacing: what
/// a seed's distance can be off by.
pub(crate) const SEED_SLACK: f32 = 0.866_025_4 + 0.5;

/// A model and its voxels, built once per model and grid.
#[derive(Clone)]
pub struct Volume {
    mesh: IndexedMesh,
    bvh: Bvh,
    bbox: Aabb,
    grid: GridSpec,
    solid: Vec<bool>,
    /// Distance from each cube's centre to the nearest cube holding the
    /// surface, in voxels.
    near: Vec<f32>,
    occupied: usize,
}

impl std::fmt::Debug for Volume {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Volume")
            .field("grid", &self.grid)
            .field("occupied", &self.occupied)
            .finish()
    }
}

/// Where a vertical line crosses a triangle: its column, the height, and
/// `+1` when the model is entered going up there, `-1` when it is left.
type Crossing = (u32, f32, i8);

impl Volume {
    /// Voxelizes `mesh` (closed, facing out) on cubes of `voxel_mm`.
    pub fn build(mesh: IndexedMesh, voxel_mm: f32, ctl: Control<'_>) -> Result<Self, Cancelled> {
        let bbox = mesh.bbox();
        let grid = GridSpec::for_bbox_with_voxel(&bbox, voxel_mm);
        ctl.stage(Stage::Voxelize)?;
        let (crossings, bvh) = rayon::join(|| crossings(&mesh, &grid), || Bvh::build(&mesh));
        ctl.check()?;
        ctl.stage(Stage::FloodFill)?;
        let solid = fill(&grid, &crossings);
        drop(crossings);
        let occupied = solid.par_iter().filter(|s| **s).count();
        ctl.stage(Stage::DistanceField)?;
        let near = seed_distances(&mesh, &grid, ctl)?;
        Ok(Self {
            mesh,
            bvh,
            bbox,
            grid,
            solid,
            near,
            occupied,
        })
    }

    pub fn grid(&self) -> GridSpec {
        self.grid
    }

    pub fn source_mesh(&self) -> &IndexedMesh {
        &self.mesh
    }

    pub fn source_bvh(&self) -> &Bvh {
        &self.bvh
    }

    pub fn source_bbox(&self) -> Aabb {
        self.bbox
    }

    /// Whether each cube's centre is inside the model.
    pub(crate) fn solid_mask(&self) -> &[bool] {
        &self.solid
    }

    pub fn is_solid(&self, index: usize) -> bool {
        self.solid.get(index).copied().unwrap_or(false)
    }

    /// How many cubes are inside.
    pub fn occupied_voxels(&self) -> usize {
        self.occupied
    }

    /// Each cube's distance to the nearest cube holding a surface sample, in
    /// voxels: the centre is at least this less [`SEED_SLACK`] from the
    /// surface, and at most this plus it.
    pub(crate) fn near(&self) -> &[f32] {
        &self.near
    }
}

/// The side of the directed edge `u → v` the point `c` is on, positive to
/// the left, computed the same way whichever way a triangle runs along the
/// edge (from its lower-numbered corner), so the two triangles on an edge
/// always read opposite signs.
fn edge_side(c: [f64; 2], (iu, u): (u32, [f64; 2]), (iv, v): (u32, [f64; 2])) -> f64 {
    let (a, b, flip) = if iu < iv { (u, v, 1.0) } else { (v, u, -1.0) };
    flip * ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]))
}

/// Whether a point exactly on the directed edge `u → v` belongs to the
/// triangle on its left: of the two triangles on an edge, exactly one does.
fn owns_edge(u: [f64; 2], v: [f64; 2]) -> bool {
    let d = [v[0] - u[0], v[1] - u[1]];
    d[1] < 0.0 || (d[1] == 0.0 && d[0] > 0.0)
}

/// Every crossing of the grid's vertical lines (through the cubes' centres)
/// with the triangles, sorted by column and height.
fn crossings(mesh: &IndexedMesh, grid: &GridSpec) -> Vec<Crossing> {
    let (x0, y0, v) = (
        f64::from(grid.min.x),
        f64::from(grid.min.y),
        f64::from(grid.voxel_mm),
    );
    let centre = |n: isize, from: f64| from + (n as f64 + 0.5) * v;
    let mut all: Vec<Crossing> = mesh
        .triangles
        .par_iter()
        .flat_map_iter(|&tri| {
            let p = tri.map(|i| {
                let q = mesh.positions[i as usize];
                ([f64::from(q.x), f64::from(q.y)], f64::from(q.z))
            });
            let area = (p[1].0[0] - p[0].0[0]) * (p[2].0[1] - p[0].0[1])
                - (p[1].0[1] - p[0].0[1]) * (p[2].0[0] - p[0].0[0]);
            let mut out = Vec::new();
            if area == 0.0 || !area.is_finite() {
                return out;
            }
            // Counter-clockwise seen from above; facing up means the model
            // is left going up through it.
            let (order, step) = if area > 0.0 {
                ([0, 1, 2], -1i8)
            } else {
                ([0, 2, 1], 1i8)
            };
            let corner = |k: usize| (tri[order[k]], p[order[k]].0);
            let heights = order.map(|k| p[k].1);
            let span = |k: usize| {
                let lo = p.iter().map(|q| q.0[k]).fold(f64::INFINITY, f64::min);
                let hi = p.iter().map(|q| q.0[k]).fold(f64::NEG_INFINITY, f64::max);
                (lo, hi)
            };
            let ((xa, xb), (ya, yb)) = (span(0), span(1));
            let first = |lo: f64, from: f64| ((lo - from) / v - 0.5).ceil().max(0.0) as isize;
            let last = |hi: f64, from: f64, n: usize| {
                (((hi - from) / v - 0.5).floor() as isize).min(n as isize - 1)
            };
            for gy in first(ya, y0)..=last(yb, y0, grid.ny) {
                for gx in first(xa, x0)..=last(xb, x0, grid.nx) {
                    let c = [centre(gx, x0), centre(gy, y0)];
                    let mut weights = [0.0f64; 3];
                    let mut inside = true;
                    for k in 0..3 {
                        let (u, w) = (corner((k + 1) % 3), corner((k + 2) % 3));
                        let side = edge_side(c, u, w);
                        if side < 0.0 || (side == 0.0 && !owns_edge(u.1, w.1)) {
                            inside = false;
                            break;
                        }
                        weights[k] = side;
                    }
                    if !inside {
                        continue;
                    }
                    let total = weights[0] + weights[1] + weights[2];
                    let z = (weights[0] * heights[0]
                        + weights[1] * heights[1]
                        + weights[2] * heights[2])
                        / total;
                    out.push(((gy as usize * grid.nx + gx as usize) as u32, z as f32, step));
                }
            }
            out
        })
        .collect();
    all.par_sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)));
    all
}

/// The cubes whose centres the crossings put inside: wound more than zero
/// times from below.
fn fill(grid: &GridSpec, crossings: &[Crossing]) -> Vec<bool> {
    let runs: Vec<(u32, Vec<(usize, usize)>)> = crossings
        .par_chunk_by(|a, b| a.0 == b.0)
        .map(|column| {
            let mut spans = Vec::new();
            let mut winding = 0i32;
            let mut from = 0usize;
            for &(_, z, step) in column {
                let was = winding > 0;
                winding += i32::from(step);
                // The first cube whose centre is above this crossing.
                let k = (((z - grid.min.z) / grid.voxel_mm - 0.5).floor() + 1.0).max(0.0) as usize;
                let k = k.min(grid.nz);
                match (was, winding > 0) {
                    (false, true) => from = k,
                    (true, false) if k > from => spans.push((from, k)),
                    _ => {}
                }
            }
            (column[0].0, spans)
        })
        .collect();
    let mut solid = vec![false; grid.cell_count()];
    let plane = grid.nx * grid.ny;
    for (column, spans) in runs {
        for (a, b) in spans {
            for z in a..b {
                solid[z * plane + column as usize] = true;
            }
        }
    }
    solid
}

/// Each cube's distance, in voxels, to the nearest cube holding a sample of
/// the surface, the surface sampled no farther apart than half a voxel.
fn seed_distances(
    mesh: &IndexedMesh,
    grid: &GridSpec,
    ctl: Control<'_>,
) -> Result<Vec<f32>, Cancelled> {
    use std::sync::atomic::{AtomicBool, Ordering};
    let spacing = grid.voxel_mm * 0.5;
    let seeded: Vec<AtomicBool> = (0..grid.cell_count())
        .map(|_| AtomicBool::new(false))
        .collect();
    let mark = |p| {
        if let Some(cell) = grid.cell_at(p) {
            seeded[cell].store(true, Ordering::Relaxed);
        }
    };
    // Every corner once; a triangle wider than the spacing adds the points
    // of a finer net over it.
    mesh.positions.par_iter().for_each(|&p| mark(p));
    mesh.triangles.par_iter().for_each(|&tri| {
        let [a, b, c] = tri.map(|i| mesh.positions[i as usize]);
        let longest = (b - a).length().max((c - b).length()).max((a - c).length());
        if longest <= spacing {
            return;
        }
        let n = (longest / spacing).ceil() as usize;
        for i in 0..=n {
            for j in 0..=n - i {
                let (s, t) = (i as f32 / n as f32, j as f32 / n as f32);
                mark(a + (b - a) * s + (c - a) * t);
            }
        }
    });
    ctl.check()?;
    let mut field: Vec<f32> = seeded
        .into_par_iter()
        .map(|s| if s.into_inner() { 0.0 } else { f32::INFINITY })
        .collect();
    squared_distance_transform(grid, &mut field);
    field.par_iter_mut().for_each(|d| *d = d.sqrt());
    Ok(field)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::Vec3;
    use crate::shapes::{cube, icosphere, merged};

    fn inside_by_rays(mesh: &IndexedMesh, p: Vec3) -> bool {
        crate::bvh::point_inside_mesh_parity(mesh, &Bvh::build(mesh), p, 1e-3)
    }

    /// Every centre is inside exactly when a ray from it says so, on a
    /// sphere whose vertices fall on the grid's lines.
    #[test]
    fn the_cubes_inside_are_the_centres_inside() {
        for (mesh, voxel) in [(icosphere(5.0, 3), 0.5), (cube(-2.0, 2.0, false), 0.5)] {
            let volume = Volume::build(mesh.clone(), voxel, Control::none()).unwrap();
            let grid = volume.grid();
            let bvh = Bvh::build(&mesh);
            let mut wrong = 0;
            for i in 0..grid.cell_count() {
                let (x, y, z) = grid.coords(i);
                let p = grid.center_world(x, y, z);
                if volume.is_solid(i) != crate::bvh::point_inside_mesh_parity(&mesh, &bvh, p, 1e-3)
                {
                    wrong += 1;
                }
            }
            assert_eq!(wrong, 0);
            assert!(volume.occupied_voxels() > 0);
        }
    }

    /// Two overlapping cubes are inside once, and a sealed pocket is not.
    #[test]
    fn overlapping_parts_count_once_and_a_pocket_is_outside() {
        let pair = merged(&[cube(0.0, 4.0, false), cube(2.0, 6.0, false)]);
        let volume = Volume::build(pair, 0.5, Control::none()).unwrap();
        let grid = volume.grid();
        let at = |p: Vec3| volume.is_solid(grid.cell_at(p).unwrap());
        assert!(at(Vec3::new(3.0, 3.0, 3.0)));
        assert!(at(Vec3::new(1.0, 1.0, 1.0)) && at(Vec3::new(5.0, 5.0, 5.0)));

        let pocket = merged(&[cube(0.0, 8.0, false), cube(3.0, 5.0, true)]);
        let volume = Volume::build(pocket.clone(), 0.5, Control::none()).unwrap();
        let grid = volume.grid();
        assert!(!volume.is_solid(grid.cell_at(Vec3::new(4.1, 4.1, 4.1)).unwrap()));
        assert!(volume.is_solid(grid.cell_at(Vec3::new(1.1, 1.1, 1.1)).unwrap()));
        assert!(!inside_by_rays(&pocket, Vec3::new(4.1, 4.1, 4.1)));
    }

    /// The seed distance bounds the distance to the surface from both sides.
    #[test]
    fn the_seed_distance_bounds_the_surface_distance() {
        let mesh = icosphere(6.0, 3);
        let volume = Volume::build(mesh.clone(), 0.4, Control::none()).unwrap();
        let grid = volume.grid();
        let bvh = volume.source_bvh();
        for i in (0..grid.cell_count()).step_by(7) {
            let (x, y, z) = grid.coords(i);
            let p = grid.center_world(x, y, z);
            let d = bvh.nearest_distance(&mesh, p, 1e9).unwrap() / grid.voxel_mm;
            let e = volume.near()[i];
            assert!(
                d >= e - SEED_SLACK - 1e-3 && d <= e + SEED_SLACK + 1e-3,
                "{d} {e}"
            );
        }
    }
}
