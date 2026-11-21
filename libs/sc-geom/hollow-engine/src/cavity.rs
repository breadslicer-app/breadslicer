//! The cavity of a hollow: which inside cubes are carved, and the surface
//! between them and the wall, the one the preview shows and the result is
//! built from (#29).
//!
//! A cube is carved when its exact distance to the surface is more than the
//! wall. Then, in order: channels narrower than the minimum width stay solid
//! (#454), every carved piece but the largest is filled back (one cavity,
//! so no pocket is sealed away from the drains), and the areas kept solid
//! are put back. The wall is the contour of the wall field (the distance
//! less the wall), held a hair off 0 at the cubes so the contour never
//! passes through a centre, with cubes kept by a rule rather than by
//! distance just on the material's side.

use rayon::prelude::*;

use crate::contour::contour;
use crate::control::{Cancelled, Control, Stage};
use crate::distance::squared_distance_transform;
use crate::grid::GridSpec;
use crate::infill::{InfillMode, infill_beams};
use crate::mesh::IndexedMesh;
use crate::solid::{LassoRegion, PreparedLasso};
use crate::topology::is_edge_manifold;
use crate::volume::Volume;

/// How far, in voxels, the wall field keeps its values from 0: the wall
/// moves by at most this much, 0.03 mm at a 0.65 mm voxel.
const EXACT_WALL_SAMPLE_MARGIN: f32 = 0.05;

/// What a cavity is cut for.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CavitySpec {
    pub wall_mm: f32,
    /// Cubes kept solid whatever their distance.
    pub blocked: Vec<usize>,
    /// Beams across the cavity: pattern, cell and beam radius in mm.
    pub infill: Option<(InfillMode, f32, f32)>,
    /// Drawn regions kept solid (#179); their cubes are in `blocked` too,
    /// and the wall lands on their surface.
    pub solid_regions: Vec<LassoRegion>,
    /// The narrowest a cavity may be (#454); 0 keeps every channel.
    pub min_width_mm: f32,
}

/// The cavity of a hollow.
#[derive(Debug, Clone)]
pub(crate) struct ExactCavity {
    /// The cavity wall, facing into the cavity.
    pub wall: IndexedMesh,
    /// The infill beams, when the hollow has infill.
    pub infill: Option<IndexedMesh>,
    /// The wall and the beams are edge-manifold: a result can be built.
    pub closed: bool,
    pub removed_voxels: usize,
    pub blocked_voxel_centers: Vec<f32>,
    pub blocked_voxel_indices: Vec<u32>,
}

/// The cavity of `volume` for `spec`, over the wall distances `dist` (in
/// voxels, exact out past the wall).
pub(crate) fn exact_cavity(
    volume: &Volume,
    spec: &CavitySpec,
    dist: &[f32],
    ctl: Control<'_>,
) -> Result<ExactCavity, Cancelled> {
    let grid = volume.grid();
    let solid = volume.solid_mask();
    let wall_vox = spec.wall_mm.max(0.2) / grid.voxel_mm;

    ctl.stage(Stage::KeepMask)?;
    let keep = keep_mask(&grid, solid, dist, wall_vox, spec);
    let kept = keep.par_iter().filter(|k| **k).count();

    ctl.stage(Stage::CavitySurface)?;
    let mut field = wall_field(solid, &keep, dist, wall_vox);
    place_on_solid_regions(&grid, solid, &keep, &spec.solid_regions, &mut field);
    let wall = contour(&grid, &field, solid);
    let wall_closed = is_edge_manifold(&wall);

    let infill = match spec.infill {
        Some((pattern, cell_mm, radius_mm)) => {
            ctl.stage(Stage::Infill)?;
            let carved = |i: usize| solid[i] && !keep[i];
            let beams = infill_beams(
                &grid,
                &volume.source_bbox(),
                &carved,
                pattern,
                cell_mm,
                radius_mm,
            );
            (!beams.triangles.is_empty()).then_some(beams)
        }
        None => None,
    };
    let closed = wall_closed && infill.as_ref().is_none_or(is_edge_manifold);

    let mut blocked_voxel_centers = Vec::new();
    let mut blocked_voxel_indices = Vec::new();
    for &i in &spec.blocked {
        if solid.get(i).copied().unwrap_or(false) {
            let (x, y, z) = grid.coords(i);
            blocked_voxel_centers.extend(grid.center_world(x, y, z).to_array());
            blocked_voxel_indices.push(i as u32);
        }
    }
    Ok(ExactCavity {
        wall,
        infill,
        closed,
        removed_voxels: volume.occupied_voxels().saturating_sub(kept),
        blocked_voxel_centers,
        blocked_voxel_indices,
    })
}

/// Which inside cubes stay material (`true`) and which are carved.
fn keep_mask(
    grid: &GridSpec,
    solid: &[bool],
    dist: &[f32],
    wall_vox: f32,
    spec: &CavitySpec,
) -> Vec<bool> {
    let mut keep: Vec<bool> = solid
        .par_iter()
        .zip(dist)
        .map(|(&s, &d)| s && d <= wall_vox)
        .collect();
    if spec.min_width_mm > 0.0 {
        let r_vox = spec.min_width_mm * 0.5 / grid.voxel_mm;
        for i in narrow_cavity_cells(grid, solid, &keep, dist, wall_vox, r_vox) {
            keep[i] = true;
        }
    }
    keep_largest_cavity(grid, solid, &mut keep);
    for &i in &spec.blocked {
        if solid.get(i).copied().unwrap_or(false) {
            keep[i] = true;
        }
    }
    keep
}

/// Fills back every carved piece (joined through faces) but the largest; of
/// equal ones, the first in the grid's order stays.
fn keep_largest_cavity(grid: &GridSpec, solid: &[bool], keep: &mut [bool]) {
    fn root(parent: &mut [u32], mut i: u32) -> u32 {
        while parent[i as usize] != i {
            let up = parent[parent[i as usize] as usize];
            parent[i as usize] = up;
            i = up;
        }
        i
    }
    let carved: Vec<bool> = solid
        .par_iter()
        .zip(&*keep)
        .map(|(s, k)| *s && !*k)
        .collect();
    let mut parent: Vec<u32> = (0..keep.len() as u32).collect();
    let steps = [1, grid.nx, grid.nx * grid.ny];
    for i in (0..keep.len()).filter(|&i| carved[i]) {
        let (x, y, z) = grid.coords(i);
        for (back, step) in [x, y, z].into_iter().zip(steps) {
            if back > 0 && carved[i - step] {
                let (a, b) = (
                    root(&mut parent, i as u32),
                    root(&mut parent, (i - step) as u32),
                );
                if a != b {
                    parent[a.max(b) as usize] = a.min(b);
                }
            }
        }
    }
    let mut size = vec![0u32; keep.len()];
    for i in (0..keep.len()).filter(|&i| carved[i]) {
        size[root(&mut parent, i as u32) as usize] += 1;
    }
    let Some(largest) = (0..keep.len()).max_by(|&a, &b| size[a].cmp(&size[b]).then(b.cmp(&a)))
    else {
        return;
    };
    for i in (0..keep.len()).filter(|&i| carved[i]) {
        if root(&mut parent, i as u32) as usize != largest {
            keep[i] = true;
        }
    }
}

/// The wall field: positive in the material, negative in the cavity, the
/// wall where it is 0. Outside the model it is the wall (material, as far
/// from the cavity as the wall can be).
fn wall_field(solid: &[bool], keep: &[bool], dist: &[f32], wall_vox: f32) -> Vec<f32> {
    let held = EXACT_WALL_SAMPLE_MARGIN.max(0.05 * wall_vox.max(0.2));
    (0..solid.len())
        .into_par_iter()
        .map(|i| {
            if !solid[i] {
                return wall_vox;
            }
            let signed = wall_vox - dist[i];
            match (keep[i], signed >= 0.0) {
                (true, true) => signed.max(EXACT_WALL_SAMPLE_MARGIN),
                (false, false) => signed.min(-EXACT_WALL_SAMPLE_MARGIN),
                // Kept by a rule deep in the cavity: just inside the material.
                (true, false) => held,
                (false, true) => -held,
            }
        })
        .collect()
}

/// The cavity cells of `keep` a ball of radius `r_vox` (in voxels) does not
/// reach (#454): the morphological opening of the cavity. A cell whose wall
/// distance leaves `r_vox` of room is a centre the ball fits on; a cavity
/// cell farther than `r_vox` from every centre, plus a voxel for the
/// centres' grid, is in a channel too narrow for it. The ball reaches the
/// wall of a wide cavity, so that wall is not moved.
fn narrow_cavity_cells(
    grid: &GridSpec,
    solid: &[bool],
    keep: &[bool],
    dist: &[f32],
    shell_voxels_f: f32,
    r_vox: f32,
) -> Vec<usize> {
    let cavity = |i: usize| solid[i] && !keep[i];
    let mut field: Vec<f32> = (0..solid.len())
        .into_par_iter()
        .map(|i| {
            if cavity(i) && dist[i] - shell_voxels_f >= r_vox {
                0.0
            } else {
                f32::INFINITY
            }
        })
        .collect();
    squared_distance_transform(grid, &mut field);
    let reach = (r_vox + 1.0) * (r_vox + 1.0);
    (0..solid.len())
        .into_par_iter()
        .filter(|&i| cavity(i) && field[i] > reach)
        .collect()
}

/// Puts the cavity wall on the surface of the drawn regions kept solid
/// (#179). The keep mask holds the voxels whose centre a region holds, so
/// alone the wall would land between those and the carved ones, up to a
/// voxel off the region. Where the mask changes across a region's surface,
/// the cells on both sides take the region's signed distance instead (in
/// voxels, kept on their side of 0), and the contour, interpolated between
/// them, follows the surface: exactly for a flat side, to second order on a
/// cone's. Cells away from it keep their values.
fn place_on_solid_regions(
    grid: &GridSpec,
    solid: &[bool],
    keep: &[bool],
    regions: &[LassoRegion],
    field: &mut [f32],
) {
    let prepared: Vec<PreparedLasso> = regions.iter().filter_map(LassoRegion::prepared).collect();
    if prepared.is_empty() {
        return;
    }
    let held = |i: usize| {
        let (x, y, z) = grid.coords(i);
        let p = grid.center_world(x, y, z);
        prepared.iter().any(|r| r.contains(p))
    };
    let in_region: Vec<bool> = (0..solid.len())
        .into_par_iter()
        .map(|i| solid[i] && held(i))
        .collect();
    let voxel = grid.voxel_mm;
    let plane = grid.nx * grid.ny;
    field
        .par_chunks_mut(plane)
        .enumerate()
        .for_each(|(z, layer)| {
            for (k, value) in layer.iter_mut().enumerate() {
                let i = z * plane + k;
                if !solid[i] {
                    continue;
                }
                // Kept for a region, or carved: the two sides of its surface.
                // A wall cell outside every region keeps its value.
                let side = if in_region[i] {
                    true
                } else if !keep[i] {
                    false
                } else {
                    continue;
                };
                let (x, y) = (k % grid.nx, k / grid.nx);
                let mut crosses = false;
                'around: for dz in -1isize..=1 {
                    for dy in -1isize..=1 {
                        for dx in -1isize..=1 {
                            let (nx, ny, nz) = (x as isize + dx, y as isize + dy, z as isize + dz);
                            if !grid.in_bounds(nx, ny, nz) {
                                continue;
                            }
                            let n = grid.idx(nx as usize, ny as usize, nz as usize);
                            let other = if in_region[n] {
                                true
                            } else if solid[n] && !keep[n] {
                                false
                            } else {
                                continue;
                            };
                            if other != side {
                                crosses = true;
                                break 'around;
                            }
                        }
                    }
                }
                if !crosses {
                    continue;
                }
                let p = grid.center_world(x, y, z);
                let distance = prepared
                    .iter()
                    .map(|r| r.signed_distance_mm(p))
                    .fold(f32::MIN, f32::max)
                    / voxel;
                *value = if side {
                    value.max(distance).max(EXACT_WALL_SAMPLE_MARGIN)
                } else {
                    // Carved: the nearer of the wall and the region, still
                    // a cavity value.
                    value.max(distance).min(-EXACT_WALL_SAMPLE_MARGIN)
                };
            }
        });
}
