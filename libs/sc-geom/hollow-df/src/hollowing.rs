//! DragonFruit's voxel hollowing engine.
//!
//! Ported from DragonFruit `rust/dragonfruit-mesh-repair/src/hollowing.rs`
//! (private downstream revision f331fc55, which carries SynrgStudio's parallel
//! slab voxelization and the surface-only visual preview). Algorithms,
//! constants and `f32` operation order are kept, so the same input gives the
//! same voxels and meshes as DragonFruit's own build without its `manifold`
//! feature. Differences, all additive:
//!
//! - every stage reports to a [`Control`] and long loops poll its cancel
//!   token;
//! - the two copies of session construction (`hollow_voxel` and
//!   `HollowSession::with_rotation`) share one function;
//! - DragonFruit's manifold3d round trip (`finalize_hollow_output_mesh_for_manifold`)
//!   is replaced by the edge-manifold check SoulCrafted's slicer applies, with
//!   the same weld ladder ([`stabilize_hollow_mesh`]);
//! - over exact wall distances ([`HollowOptions::exact_wall`]) the cavity is
//!   smoothed in its field only, clamped, without Taubin or the smoothing
//!   back-off ladder, and built once for the preview and the result
//!   ([`HollowSession::exact_cavity`], #29);
//! - the voxel-tunnel drain fallback is not ported: drains are exact cuts
//!   ([`crate::drain`]).

// Index loops over several parallel voxel arrays are DragonFruit's form; the
// port keeps them rather than restate each as zipped iterators.
#![allow(clippy::needless_range_loop)]

use std::collections::VecDeque;

use rayon::prelude::*;

use crate::bvh::Bvh;
use crate::control::{Cancelled, Control, Stage};
use crate::mesh::{Aabb, IndexedMesh, Vec3};
use crate::topology::{Topology, edge_manifold_defects, is_edge_manifold};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HollowMode {
    Cavity,
    Infill,
    ShellOpenFace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfillMode {
    Lattice,
    Pillar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenFace {
    XMin,
    XMax,
    YMin,
    YMax,
    ZMin,
    ZMax,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DrainHoleSpec {
    /// Normalized position inside source bbox, each axis in [0, 1].
    pub center_norm: [f32; 3],
    /// Radius in millimeters.
    pub radius_mm: f32,
    /// Optional unit direction for a manual punch, in source-mesh local space.
    pub direction: Option<[f32; 3]>,
    /// Optional punch depth in millimeters.
    pub length_mm: Option<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HollowOptions {
    pub mode: HollowMode,
    pub voxel_resolution: u16,
    pub shell_thickness_mm: f32,
    pub blocked_voxel_indices: Vec<usize>,
    pub infill_mode: InfillMode,
    pub infill_cell_mm: f32,
    pub infill_beam_radius_mm: f32,
    pub open_face: OpenFace,
    pub drain_holes: Vec<DrainHoleSpec>,
    pub preview_cavity_only: bool,
    pub smooth_internal_surfaces: bool,
    /// Number of voxel chamfer passes to run on internal cavity boundaries.
    /// 0 disables chamfering, 1-2 progressively bevel 90° steps toward ~45° ramps.
    pub internal_chamfer_passes: u8,
    /// Skip building the smoothed cavity mesh; the caller renders the removed
    /// voxel centers instead.
    pub preview_voxel_spheres: bool,
    /// Visual cavity extraction only: no boolean stabilization or final mesh merge.
    pub preview_surface_only: bool,
    /// Unit quaternion `[x, y, z, w]` to rotate the source mesh before
    /// voxelizing; outputs are rotated back. Identity is a model-local grid.
    pub rotation_quat: [f32; 4],
    /// The wall distances are exact (`engine::Session`'s). A voxel whose
    /// mask agrees with its distance keeps that distance in the cavity
    /// field, so the cavity wall sits at the requested wall (DragonFruit,
    /// `false`, pushes every voxel at least 5 % of the wall away from it,
    /// which moves the wall by up to that much either way). The smoothing
    /// is then clamped (#266): it may only add material, at most
    /// [`EXACT_SMOOTHING_CAP_VOXELS`], and never turns a cavity voxel into
    /// material, so the smoothed cavity has the unsmoothed one's voxels and
    /// shells. The mesh is not Taubin-smoothed, and a cavity that does not
    /// close falls back to the unsmoothed one instead of DragonFruit's ladder.
    pub exact_wall: bool,
}

impl Default for HollowOptions {
    fn default() -> Self {
        Self {
            mode: HollowMode::Cavity,
            voxel_resolution: 64,
            shell_thickness_mm: 2.0,
            blocked_voxel_indices: Vec::new(),
            infill_mode: InfillMode::Lattice,
            infill_cell_mm: 4.2426,
            infill_beam_radius_mm: 0.35,
            open_face: OpenFace::ZMax,
            drain_holes: Vec::new(),
            preview_cavity_only: false,
            smooth_internal_surfaces: true,
            internal_chamfer_passes: 2,
            preview_voxel_spheres: false,
            preview_surface_only: false,
            rotation_quat: [0.0, 0.0, 0.0, 1.0],
            exact_wall: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HollowReport {
    pub mode: HollowMode,
    /// The requested resolution, as DragonFruit reports it (before its clamp
    /// to 24..=192; see [`GridSpec`] for the effective grid).
    pub voxel_resolution: u16,
    pub voxel_size_mm: f32,
    pub shell_thickness_mm: f32,
    pub source_triangle_count: usize,
    pub output_triangle_count: usize,
    pub grid_size: [usize; 3],
    pub occupied_voxels: usize,
    pub shell_voxels: usize,
    pub removed_voxels: usize,
}

#[derive(Debug, Clone)]
pub struct HollowOutcome {
    pub mesh: IndexedMesh,
    /// Interior cavity surface, extracted separately for interior views.
    pub cavity_mesh: Option<IndexedMesh>,
    pub preview_infill_mesh: Option<IndexedMesh>,
    pub removed_voxel_centers: Vec<f32>,
    pub removed_voxel_indices: Vec<u32>,
    pub blocked_voxel_centers: Vec<f32>,
    pub blocked_voxel_indices: Vec<u32>,
    pub report: HollowReport,
}

/// The resolution DragonFruit accepts: requests outside are clamped.
pub const MIN_VOXEL_RESOLUTION: u16 = 24;
pub const MAX_VOXEL_RESOLUTION: u16 = 192;

/// The voxel grid a session hollows on, in the session's own frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridSpec {
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    pub voxel_mm: f32,
    /// Corner of voxel `(0, 0, 0)`.
    pub min: Vec3,
}

impl GridSpec {
    #[inline]
    pub fn idx(&self, x: usize, y: usize, z: usize) -> usize {
        x + self.nx * (y + self.ny * z)
    }

    #[inline]
    pub fn in_bounds(&self, x: isize, y: isize, z: isize) -> bool {
        x >= 0
            && y >= 0
            && z >= 0
            && (x as usize) < self.nx
            && (y as usize) < self.ny
            && (z as usize) < self.nz
    }

    #[inline]
    pub fn center_world(&self, x: usize, y: usize, z: usize) -> Vec3 {
        Vec3::new(
            self.min.x + (x as f32 + 0.5) * self.voxel_mm,
            self.min.y + (y as f32 + 0.5) * self.voxel_mm,
            self.min.z + (z as f32 + 0.5) * self.voxel_mm,
        )
    }

    /// `(x, y, z)` of a linear voxel index.
    #[inline]
    pub fn coords(&self, index: usize) -> (usize, usize, usize) {
        let z = index / (self.nx * self.ny);
        let rem = index - z * self.nx * self.ny;
        let y = rem / self.nx;
        (rem - y * self.nx, y, z)
    }

    pub fn cell_count(&self) -> usize {
        self.nx * self.ny * self.nz
    }

    /// The grid DragonFruit builds for `bbox` at `voxel_resolution`: the
    /// largest extent divided into the clamped resolution (voxels no smaller
    /// than 0.05 mm), padded by one voxel on each side.
    pub fn for_bbox(bbox: &Aabb, voxel_resolution: u16) -> Self {
        let diag = bbox.max.sub(bbox.min);
        let max_extent = diag.x.max(diag.y).max(diag.z).max(1e-3);
        let resolution = voxel_resolution.clamp(MIN_VOXEL_RESOLUTION, MAX_VOXEL_RESOLUTION) as f32;
        let voxel_mm = (max_extent / resolution).max(0.05);
        Self::for_bbox_with_voxel(bbox, voxel_mm)
    }

    /// As [`GridSpec::for_bbox`] with the voxel size given directly.
    pub fn for_bbox_with_voxel(bbox: &Aabb, voxel_mm: f32) -> Self {
        // Pad by 1 voxel so outside flood-fill has a guaranteed margin.
        let padded_min = bbox.min.sub(Vec3::new(voxel_mm, voxel_mm, voxel_mm));
        let padded_max = bbox.max.add(Vec3::new(voxel_mm, voxel_mm, voxel_mm));
        let size = padded_max.sub(padded_min);
        Self {
            nx: ((size.x / voxel_mm).ceil() as usize).max(4),
            ny: ((size.y / voxel_mm).ceil() as usize).max(4),
            nz: ((size.z / voxel_mm).ceil() as usize).max(4),
            voxel_mm,
            min: padded_min,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HollowSession {
    source_mesh: IndexedMesh,
    /// Ray and closest-point queries on `source_mesh` (built for the parity
    /// test, kept for the exact wall distance and the drains).
    source_bvh: Bvh,
    source_bbox: Aabb,
    grid: GridSpec,
    solid: Vec<bool>,
    /// Chamfer distance to the surface, in voxels (see `build`).
    dist: Vec<f32>,
    source_void_components: Vec<i32>,
    source_triangle_count: usize,
    occupied_voxels: usize,
    voxel_resolution: u16,
    /// The rotation used when building this session.
    rotation_quat: [f32; 4],
}

#[derive(Clone, Copy)]
struct TriangleCache {
    a: Vec3,
    b: Vec3,
    c: Vec3,
    min: Vec3,
    max: Vec3,
}

impl TriangleCache {
    fn from_points(a: Vec3, b: Vec3, c: Vec3) -> Self {
        let min = a.min(b).min(c);
        let max = a.max(b).max(c);
        Self { a, b, c, min, max }
    }
}

/// Classify independent Z slabs in parallel, without atomic writes or a full
/// per-worker grid. A surface cell is a union, so skip further distance tests
/// as soon as any triangle has marked it. Keep the original distance predicate
/// and conservative triangle bounds: this changes scheduling, not geometry.
fn voxelize_surface(grid: &GridSpec, triangles: &[TriangleCache], ctl: Control<'_>) -> Vec<bool> {
    let mut surface = vec![false; grid.nx * grid.ny * grid.nz];
    let plane = grid.nx * grid.ny;
    let slab_depth = 8;
    let voxel_mm = grid.voxel_mm;
    let voxel_diag_half = (3.0f32).sqrt() * voxel_mm * 0.5;
    surface
        .par_chunks_mut(plane * slab_depth)
        .enumerate()
        .for_each(|(slab, cells)| {
            if ctl.is_cancelled() {
                return;
            }
            let z_start = slab * slab_depth;
            let z_end = z_start + cells.len() / plane;
            for tri in triangles {
                let min_iz =
                    (((tri.min.z - grid.min.z) / voxel_mm).floor() as isize - 1).max(0) as usize;
                let max_iz = (((tri.max.z - grid.min.z) / voxel_mm).ceil() as isize + 1)
                    .min(grid.nz as isize - 1) as usize;
                if min_iz >= z_end || max_iz < z_start {
                    continue;
                }
                let min_ix =
                    (((tri.min.x - grid.min.x) / voxel_mm).floor() as isize - 1).max(0) as usize;
                let max_ix = (((tri.max.x - grid.min.x) / voxel_mm).ceil() as isize + 1)
                    .min(grid.nx as isize - 1) as usize;
                let min_iy =
                    (((tri.min.y - grid.min.y) / voxel_mm).floor() as isize - 1).max(0) as usize;
                let max_iy = (((tri.max.y - grid.min.y) / voxel_mm).ceil() as isize + 1)
                    .min(grid.ny as isize - 1) as usize;
                for z in min_iz.max(z_start)..=max_iz.min(z_end - 1) {
                    for y in min_iy..=max_iy {
                        for x in min_ix..=max_ix {
                            let i = x + grid.nx * (y + grid.ny * (z - z_start));
                            if !cells[i]
                                && point_triangle_distance(
                                    grid.center_world(x, y, z),
                                    tri.a,
                                    tri.b,
                                    tri.c,
                                ) <= voxel_diag_half
                            {
                                cells[i] = true;
                            }
                        }
                    }
                }
            }
        });
    surface
}

const N6: [(isize, isize, isize); 6] = [
    (1, 0, 0),
    (-1, 0, 0),
    (0, 1, 0),
    (0, -1, 0),
    (0, 0, 1),
    (0, 0, -1),
];

const SQRT_2: f32 = std::f32::consts::SQRT_2;
const SQRT_3: f32 = 1.732_050_8_f32;

/// Forward-scan neighbour offsets and weights for the two-pass 3-D chamfer
/// distance transform (z-outer, y-middle, x-inner ascending scan order).
const SHELL_DIST_FORWARD: [((isize, isize, isize), f32); 13] = [
    ((-1, -1, -1), SQRT_3),
    ((0, -1, -1), SQRT_2),
    ((1, -1, -1), SQRT_3),
    ((-1, 0, -1), SQRT_2),
    ((0, 0, -1), 1.0_f32),
    ((1, 0, -1), SQRT_2),
    ((-1, 1, -1), SQRT_3),
    ((0, 1, -1), SQRT_2),
    ((1, 1, -1), SQRT_3),
    ((-1, -1, 0), SQRT_2),
    ((0, -1, 0), 1.0_f32),
    ((1, -1, 0), SQRT_2),
    ((-1, 0, 0), 1.0_f32),
];

/// Complementary backward-scan mask for the second pass.
const SHELL_DIST_BACKWARD: [((isize, isize, isize), f32); 13] = [
    ((-1, -1, 1), SQRT_3),
    ((0, -1, 1), SQRT_2),
    ((1, -1, 1), SQRT_3),
    ((-1, 0, 1), SQRT_2),
    ((0, 0, 1), 1.0_f32),
    ((1, 0, 1), SQRT_2),
    ((-1, 1, 1), SQRT_3),
    ((0, 1, 1), SQRT_2),
    ((1, 1, 1), SQRT_3),
    ((-1, 1, 0), SQRT_2),
    ((0, 1, 0), 1.0_f32),
    ((1, 1, 0), SQRT_2),
    ((1, 0, 0), 1.0_f32),
];

#[inline]
fn is_identity_quat(q: [f32; 4]) -> bool {
    q[0] == 0.0 && q[1] == 0.0 && q[2] == 0.0
}

#[inline]
fn inverse_quat(q: [f32; 4]) -> [f32; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}

/// DragonFruit's one-shot hollow: a session built and run once. The smoothing
/// profile is chosen from the ceiling of the wall thickness in voxels, as
/// DragonFruit's `hollow_voxel` does (a session `run` uses the exact value).
pub fn hollow_voxel(mut mesh: IndexedMesh, options: &HollowOptions) -> HollowOutcome {
    let source_triangle_count = mesh.triangle_count();
    if source_triangle_count == 0 || mesh.positions.is_empty() {
        if !is_identity_quat(options.rotation_quat) {
            for p in &mut mesh.positions {
                *p = p.rotate_by_quat(options.rotation_quat);
            }
        }
        return HollowOutcome {
            mesh,
            cavity_mesh: None,
            preview_infill_mesh: None,
            removed_voxel_centers: Vec::new(),
            removed_voxel_indices: Vec::new(),
            blocked_voxel_centers: Vec::new(),
            blocked_voxel_indices: Vec::new(),
            report: HollowReport {
                mode: options.mode,
                voxel_resolution: options.voxel_resolution,
                voxel_size_mm: 0.0,
                shell_thickness_mm: options.shell_thickness_mm,
                source_triangle_count,
                output_triangle_count: source_triangle_count,
                grid_size: [0, 0, 0],
                occupied_voxels: 0,
                shell_voxels: 0,
                removed_voxels: 0,
            },
        };
    }
    let session =
        HollowSession::with_rotation(mesh, options.voxel_resolution, options.rotation_quat);
    let shell_voxels =
        ((options.shell_thickness_mm.max(0.2) / session.grid.voxel_mm).ceil() as i32).max(1);
    session
        .run_with(options, shell_voxels as f32, Control::none())
        .unwrap_or_else(|Cancelled| unreachable!("no cancel token"))
}

impl HollowSession {
    pub fn new(mesh: IndexedMesh, voxel_resolution: u16) -> Self {
        Self::with_rotation(mesh, voxel_resolution, [0.0, 0.0, 0.0, 1.0])
    }

    pub fn with_rotation(
        mesh: IndexedMesh,
        voxel_resolution: u16,
        rotation_quat: [f32; 4],
    ) -> Self {
        Self::build(mesh, voxel_resolution, rotation_quat, None, Control::none())
            .unwrap_or_else(|Cancelled| unreachable!("no cancel token"))
    }

    /// Builds a session: rotates the mesh into the grid frame, voxelizes the
    /// surface, fills the outside, restores sealed source voids and measures
    /// every solid voxel's distance to the surface. `voxel_mm`, when given,
    /// replaces DragonFruit's `max_extent / clamp(resolution)` derivation.
    pub fn build(
        mut mesh: IndexedMesh,
        voxel_resolution: u16,
        rotation_quat: [f32; 4],
        voxel_mm: Option<f32>,
        ctl: Control<'_>,
    ) -> Result<Self, Cancelled> {
        // Apply rotation so the voxel grid aligns with the rotated model. The
        // output is unrotated before returning.
        if !is_identity_quat(rotation_quat) {
            for p in &mut mesh.positions {
                *p = p.rotate_by_quat(rotation_quat);
            }
        }

        let source_triangle_count = mesh.triangle_count();
        let source_bbox = mesh.bbox();
        let grid = match voxel_mm {
            Some(voxel_mm) => GridSpec::for_bbox_with_voxel(&source_bbox, voxel_mm),
            None => GridSpec::for_bbox(&source_bbox, voxel_resolution),
        };
        let (nx, ny, nz) = (grid.nx, grid.ny, grid.nz);

        ctl.stage(Stage::Voxelize)?;
        let tri_cache: Vec<TriangleCache> = mesh
            .triangles
            .iter()
            .map(|tri| {
                let a = mesh.positions[tri[0] as usize];
                let b = mesh.positions[tri[1] as usize];
                let c = mesh.positions[tri[2] as usize];
                TriangleCache::from_points(a, b, c)
            })
            .collect();

        let surface = voxelize_surface(&grid, &tri_cache, ctl);
        drop(tri_cache);

        // Outside flood-fill through non-surface voxels.
        ctl.stage(Stage::FloodFill)?;
        let mut outside = vec![false; nx * ny * nz];
        let mut q = VecDeque::<(usize, usize, usize)>::new();
        let mut push_seed = |x: usize, y: usize, z: usize| {
            let i = grid.idx(x, y, z);
            if surface[i] || outside[i] {
                return;
            }
            outside[i] = true;
            q.push_back((x, y, z));
        };

        for x in 0..nx {
            for y in 0..ny {
                push_seed(x, y, 0);
                push_seed(x, y, nz - 1);
            }
        }
        for x in 0..nx {
            for z in 0..nz {
                push_seed(x, 0, z);
                push_seed(x, ny - 1, z);
            }
        }
        for y in 0..ny {
            for z in 0..nz {
                push_seed(0, y, z);
                push_seed(nx - 1, y, z);
            }
        }

        while let Some((x, y, z)) = q.pop_front() {
            for (dx, dy, dz) in N6 {
                let nx_i = x as isize + dx;
                let ny_i = y as isize + dy;
                let nz_i = z as isize + dz;
                if !grid.in_bounds(nx_i, ny_i, nz_i) {
                    continue;
                }
                let ux = nx_i as usize;
                let uy = ny_i as usize;
                let uz = nz_i as usize;
                let i = grid.idx(ux, uy, uz);
                if surface[i] || outside[i] {
                    continue;
                }
                outside[i] = true;
                q.push_back((ux, uy, uz));
            }
        }

        // Fill interior = !outside. This includes the surface layer itself.
        let mut solid: Vec<bool> = outside.iter().map(|o| !*o).collect();
        drop(outside);

        // Flood-fill alone treats sealed air pockets in already-hollow source
        // shells as "solid". Classify only the non-surface components with
        // parity so nested shells preserve their existing cavities.
        ctl.stage(Stage::ParityRefine)?;
        let source_bvh = Bvh::build(&mesh);
        refine_non_surface_solid_components_with_parity(
            &grid,
            &surface,
            &mut solid,
            &mesh,
            &source_bvh,
            ctl,
        )?;
        let source_void_components = label_void_components(&grid, &solid);
        let occupied_voxels = solid.iter().filter(|v| **v).count();

        // Two-pass 26-neighbour chamfer distance transform (face 1, edge √2,
        // corner √3), seeded at every surface voxel so thickness holds to the
        // outside and to pre-existing cavities. It never under-measures the
        // distance between voxel centres, but over-measures it by up to
        // 12.8 % (direction ≈ (280, 116, 89)), and the seeds sit up to half a
        // voxel diagonal off the surface. DragonFruit's wall comes from it;
        // `engine::Session` measures exact distances near the wall instead.
        ctl.stage(Stage::DistanceField)?;
        let mut dist = vec![f32::INFINITY; nx * ny * nz];
        for i in 0..dist.len() {
            if solid[i] && surface[i] {
                dist[i] = 0.0;
            }
        }
        drop(surface);

        for z in 0..nz {
            ctl.check()?;
            for y in 0..ny {
                for x in 0..nx {
                    let i = grid.idx(x, y, z);
                    if !solid[i] {
                        continue;
                    }
                    let mut d = dist[i];
                    for &((dx, dy, dz), w) in &SHELL_DIST_FORWARD {
                        let nx_i = x as isize + dx;
                        let ny_i = y as isize + dy;
                        let nz_i = z as isize + dz;
                        if !grid.in_bounds(nx_i, ny_i, nz_i) {
                            continue;
                        }
                        let ni = grid.idx(nx_i as usize, ny_i as usize, nz_i as usize);
                        if !solid[ni] {
                            continue;
                        }
                        let candidate = dist[ni] + w;
                        if candidate < d {
                            d = candidate;
                        }
                    }
                    dist[i] = d;
                }
            }
        }

        for z in (0..nz).rev() {
            ctl.check()?;
            for y in (0..ny).rev() {
                for x in (0..nx).rev() {
                    let i = grid.idx(x, y, z);
                    if !solid[i] {
                        continue;
                    }
                    let mut d = dist[i];
                    for &((dx, dy, dz), w) in &SHELL_DIST_BACKWARD {
                        let nx_i = x as isize + dx;
                        let ny_i = y as isize + dy;
                        let nz_i = z as isize + dz;
                        if !grid.in_bounds(nx_i, ny_i, nz_i) {
                            continue;
                        }
                        let ni = grid.idx(nx_i as usize, ny_i as usize, nz_i as usize);
                        if !solid[ni] {
                            continue;
                        }
                        let candidate = dist[ni] + w;
                        if candidate < d {
                            d = candidate;
                        }
                    }
                    dist[i] = d;
                }
            }
        }

        Ok(Self {
            source_mesh: mesh,
            source_bvh,
            source_bbox,
            grid,
            solid,
            dist,
            source_void_components,
            source_triangle_count,
            occupied_voxels,
            voxel_resolution,
            rotation_quat,
        })
    }

    pub fn voxel_resolution(&self) -> u16 {
        self.voxel_resolution
    }

    /// The rotation quaternion used when creating this session.
    pub fn rotation_quat(&self) -> [f32; 4] {
        self.rotation_quat
    }

    /// The grid, in the session's (rotated) frame.
    pub fn grid(&self) -> GridSpec {
        self.grid
    }

    /// The source mesh, in the session's (rotated) frame.
    pub fn source_mesh(&self) -> &IndexedMesh {
        &self.source_mesh
    }

    /// The BVH of [`HollowSession::source_mesh`].
    pub fn source_bvh(&self) -> &Bvh {
        &self.source_bvh
    }

    /// Which voxels are inside the source (surface layer included).
    pub(crate) fn solid_mask(&self) -> &[bool] {
        &self.solid
    }

    /// The chamfer distance of every voxel to the surface, in voxels: 0 on
    /// the surface layer, infinite outside the source.
    pub(crate) fn chamfer_distances(&self) -> &[f32] {
        &self.dist
    }

    pub fn source_bbox(&self) -> Aabb {
        self.source_bbox
    }

    /// Solid voxels inside the source (surface layer included).
    pub fn occupied_voxels(&self) -> usize {
        self.occupied_voxels
    }

    /// Whether voxel `index` is inside the source.
    pub fn is_solid(&self, index: usize) -> bool {
        self.solid.get(index).copied().unwrap_or(false)
    }

    /// Recomputes the `keep` mask (which solid voxels remain material) for
    /// `options`, plus the count of voxels the shell-distance pass kept. The
    /// single source of truth for the mask: output meshes and voxel selection
    /// read the same cavity (`removed = solid && !keep`).
    pub(crate) fn compute_keep_mask(&self, options: &HollowOptions) -> (Vec<bool>, usize) {
        self.compute_keep_mask_with(options, &self.dist)
    }

    /// [`HollowSession::compute_keep_mask`] over the wall distances `dist`
    /// (in voxels, one per cell) instead of the chamfer distances.
    pub(crate) fn compute_keep_mask_with(
        &self,
        options: &HollowOptions,
        dist: &[f32],
    ) -> (Vec<bool>, usize) {
        let shell_voxels = (options.shell_thickness_mm.max(0.2) / self.grid.voxel_mm).ceil() as i32;
        let shell_voxels = shell_voxels.max(1);
        let shell_voxels_f = options.shell_thickness_mm.max(0.2) / self.grid.voxel_mm;

        let mut keep = vec![false; self.solid.len()];
        let mut kept_shell = 0usize;
        for i in 0..keep.len() {
            if self.solid[i] && dist[i] <= shell_voxels_f {
                keep[i] = true;
                kept_shell += 1;
            }
        }

        // Preserve separator voxels between different source void regions, so
        // the cavity cannot punch through walls of multi-shell sources.
        preserve_source_void_separators(
            &self.grid,
            &self.solid,
            &self.source_void_components,
            &mut keep,
        );

        if matches!(options.mode, HollowMode::Cavity) && !options.drain_holes.is_empty() {
            for hole in &options.drain_holes {
                apply_drain_hole_corridor(
                    &self.grid,
                    &mut keep,
                    hole,
                    &self.source_bbox,
                    self.grid.voxel_mm,
                );
            }
        }

        if matches!(options.mode, HollowMode::ShellOpenFace) {
            let depth = shell_voxels.max(1) as usize;
            for z in 0..self.grid.nz {
                for y in 0..self.grid.ny {
                    for x in 0..self.grid.nx {
                        let remove = match options.open_face {
                            OpenFace::XMin => x < depth,
                            OpenFace::XMax => x + depth >= self.grid.nx,
                            OpenFace::YMin => y < depth,
                            OpenFace::YMax => y + depth >= self.grid.ny,
                            OpenFace::ZMin => z < depth,
                            OpenFace::ZMax => z + depth >= self.grid.nz,
                        };
                        if remove {
                            keep[self.grid.idx(x, y, z)] = false;
                        }
                    }
                }
            }
        }

        // Optional voxel-level chamfering of cavity boundaries, turning hard
        // orthogonal internal steps into printable ~45° transitions. Not over
        // exact distances: their contour has no steps, and every kept voxel
        // is within the wall, so carving one thins the wall (#29: down to
        // 1.46 mm of 2 on the Wolverine).
        if options.internal_chamfer_passes > 0
            && !options.smooth_internal_surfaces
            && !options.exact_wall
        {
            let passes = effective_internal_cavity_chamfer_passes(
                options.shell_thickness_mm,
                shell_voxels_f,
                options.internal_chamfer_passes,
            );
            for _ in 0..passes {
                apply_internal_cavity_chamfer_pass(&self.grid, &self.solid, &mut keep, dist);
            }
            if passes > 0 {
                preserve_source_void_separators(
                    &self.grid,
                    &self.solid,
                    &self.source_void_components,
                    &mut keep,
                );
            }
        }

        // In cavity mode, keep exactly one connected interior cavity; other
        // pockets are filled back, which keeps the wall thickness guarantee.
        if matches!(options.mode, HollowMode::Cavity) {
            retain_largest_connected_cavity_component(&self.grid, &self.solid, &mut keep);
        }

        for &blocked_index in &options.blocked_voxel_indices {
            if blocked_index < keep.len() && self.solid[blocked_index] {
                keep[blocked_index] = true;
            }
        }

        (keep, kept_shell)
    }

    /// Selects removed (cavity) voxels whose projected center falls in
    /// `polygon` (container pixels), over the full through-depth cavity.
    ///
    /// `session_to_world` is the column-major 4x4 that takes a point of the
    /// session frame to the scene, `view_proj` the camera's column-major
    /// projection times view. A generalisation of DragonFruit's lasso, whose
    /// geometry-center/scale/quaternion/position chain is one such matrix.
    pub fn select_removed_voxels_in_polygon(
        &self,
        options: &HollowOptions,
        polygon: &[[f32; 2]],
        session_to_world: &[f32; 16],
        view_proj: &[f32; 16],
        rect: [f32; 2],
    ) -> Vec<u32> {
        let (keep, _kept_shell) = self.compute_keep_mask(options);
        select_removed_voxels_in_polygon(
            &self.grid,
            &self.solid,
            &keep,
            polygon,
            session_to_world,
            view_proj,
            rect,
        )
    }

    /// DragonFruit's `HollowSession::run` (without its `manifold` feature).
    pub fn run(&self, options: &HollowOptions) -> HollowOutcome {
        let shell_voxels_f = options.shell_thickness_mm.max(0.2) / self.grid.voxel_mm;
        self.run_with(options, shell_voxels_f, Control::none())
            .unwrap_or_else(|Cancelled| unreachable!("no cancel token"))
    }

    /// The body of [`HollowSession::run`], with the wall thickness in voxels
    /// the smoothing profile is chosen from, and a control.
    pub(crate) fn run_with(
        &self,
        options: &HollowOptions,
        smoothing_shell_voxels: f32,
        ctl: Control<'_>,
    ) -> Result<HollowOutcome, Cancelled> {
        self.run_with_distances(options, smoothing_shell_voxels, &self.dist, ctl)
    }

    /// [`HollowSession::run_with`] over the wall distances `dist` (in
    /// voxels, one per cell) instead of the chamfer distances.
    pub(crate) fn run_with_distances(
        &self,
        options: &HollowOptions,
        smoothing_shell_voxels: f32,
        dist: &[f32],
        ctl: Control<'_>,
    ) -> Result<HollowOutcome, Cancelled> {
        let shell_voxels_f = options.shell_thickness_mm.max(0.2) / self.grid.voxel_mm;
        let smoothing_profile = effective_internal_cavity_smoothing_profile(
            options.shell_thickness_mm,
            options.smooth_internal_surfaces,
            smoothing_shell_voxels,
        )
        .for_wall(options.exact_wall);

        ctl.stage(Stage::KeepMask)?;
        let (keep, kept_shell) = self.compute_keep_mask_with(options, dist);

        let removed_voxels = self
            .occupied_voxels
            .saturating_sub(keep.iter().filter(|v| **v).count());
        ctl.stage(Stage::CavitySurface)?;
        let (mut out_mesh, cavity_mesh) = if options.preview_surface_only {
            (
                build_visual_cavity_mesh(
                    &self.grid,
                    &self.solid,
                    dist,
                    &keep,
                    shell_voxels_f,
                    options.exact_wall,
                ),
                IndexedMesh::default(),
            )
        } else if options.preview_voxel_spheres {
            // Sphere preview: skip the mesh building entirely.
            (self.source_mesh.clone(), IndexedMesh::default())
        } else {
            let (out, cavity, _cavity_wall_score) = build_hollow_output_mesh(
                &self.source_mesh,
                &self.source_bbox,
                &self.grid,
                &self.solid,
                dist,
                &keep,
                options,
                shell_voxels_f,
                smoothing_profile,
            );
            (out, cavity)
        };
        let output_triangle_count = out_mesh.triangle_count();
        let mut maybe_cavity = if cavity_mesh.triangles.is_empty() {
            None
        } else {
            Some(cavity_mesh)
        };
        let mut preview_infill_mesh =
            if options.preview_cavity_only && matches!(options.mode, HollowMode::Infill) {
                ctl.stage(Stage::Infill)?;
                let mesh = build_smooth_infill_mesh(
                    &self.source_bbox,
                    &self.grid,
                    &self.solid,
                    &keep,
                    options.infill_mode,
                    options.infill_cell_mm,
                    options.infill_beam_radius_mm,
                );
                if mesh.triangles.is_empty() {
                    None
                } else {
                    Some(mesh)
                }
            } else {
                None
            };
        let mut removed_voxel_centers =
            collect_removed_voxel_centers(&self.grid, &self.solid, &keep);
        let removed_voxel_indices = collect_removed_voxel_indices(&self.grid, &self.solid, &keep);
        let (mut blocked_voxel_centers, accepted_blocked_voxel_indices) =
            collect_blocked_voxel_data(&self.grid, &self.solid, &options.blocked_voxel_indices);

        // Unrotate all outputs.
        if !is_identity_quat(self.rotation_quat) {
            let inv_quat = inverse_quat(self.rotation_quat);
            for p in &mut out_mesh.positions {
                *p = p.rotate_by_quat(inv_quat);
            }
            if let Some(ref mut cm) = maybe_cavity {
                for p in &mut cm.positions {
                    *p = p.rotate_by_quat(inv_quat);
                }
            }
            if let Some(ref mut im) = preview_infill_mesh {
                for p in &mut im.positions {
                    *p = p.rotate_by_quat(inv_quat);
                }
            }
            for chunk in removed_voxel_centers.chunks_exact_mut(3) {
                let v = Vec3::new(chunk[0], chunk[1], chunk[2]).rotate_by_quat(inv_quat);
                chunk.copy_from_slice(&v.to_array());
            }
            for chunk in blocked_voxel_centers.chunks_exact_mut(3) {
                let v = Vec3::new(chunk[0], chunk[1], chunk[2]).rotate_by_quat(inv_quat);
                chunk.copy_from_slice(&v.to_array());
            }
        }

        Ok(HollowOutcome {
            mesh: out_mesh,
            cavity_mesh: maybe_cavity,
            preview_infill_mesh,
            removed_voxel_centers,
            removed_voxel_indices,
            blocked_voxel_centers,
            blocked_voxel_indices: accepted_blocked_voxel_indices,
            report: HollowReport {
                mode: options.mode,
                voxel_resolution: self.voxel_resolution,
                voxel_size_mm: self.grid.voxel_mm,
                shell_thickness_mm: options.shell_thickness_mm,
                source_triangle_count: self.source_triangle_count,
                output_triangle_count,
                grid_size: [self.grid.nx, self.grid.ny, self.grid.nz],
                occupied_voxels: self.occupied_voxels,
                shell_voxels: kept_shell,
                removed_voxels,
            },
        })
    }

    /// The cavity of an exact-wall hollow (#29): the one surface the preview
    /// shows and the result is built from, so the hollow carries exactly the
    /// cavity on screen. It is the contour of the exact wall field itself:
    /// over exact distances smoothing only thickens the wall (#266) and the
    /// voxel chamfer only thins it, so neither runs. Ignores the preview
    /// flags and the voxel drain corridors of `options` (drains are exact
    /// cuts on the result). In the session frame: the session must not be
    /// rotated.
    pub(crate) fn exact_cavity(
        &self,
        options: &HollowOptions,
        dist: &[f32],
        ctl: Control<'_>,
    ) -> Result<ExactCavity, Cancelled> {
        debug_assert!(options.exact_wall && is_identity_quat(self.rotation_quat));
        let mut options = options.clone();
        options.drain_holes.clear();
        let shell_voxels_f = options.shell_thickness_mm.max(0.2) / self.grid.voxel_mm;

        ctl.stage(Stage::KeepMask)?;
        let (keep, _kept_shell) = self.compute_keep_mask_with(&options, dist);
        let kept = keep.iter().filter(|v| **v).count();

        ctl.stage(Stage::CavitySurface)?;
        let (wall, wall_closed) =
            exact_cavity_wall(&self.grid, &self.solid, &keep, dist, shell_voxels_f);

        let infill = if matches!(options.mode, HollowMode::Infill) {
            ctl.stage(Stage::Infill)?;
            let mesh = build_smooth_infill_mesh(
                &self.source_bbox,
                &self.grid,
                &self.solid,
                &keep,
                options.infill_mode,
                options.infill_cell_mm,
                options.infill_beam_radius_mm,
            );
            (!mesh.triangles.is_empty()).then_some(mesh)
        } else {
            None
        };
        // Beams are closed tubes by construction; checked all the same, so
        // `closed` speaks for everything the result adds to the source.
        let closed = wall_closed && infill.as_ref().is_none_or(is_edge_manifold);

        let (blocked_voxel_centers, blocked_voxel_indices) =
            collect_blocked_voxel_data(&self.grid, &self.solid, &options.blocked_voxel_indices);
        Ok(ExactCavity {
            wall,
            infill,
            closed,
            removed_voxels: self.occupied_voxels.saturating_sub(kept),
            removed_voxel_centers: collect_removed_voxel_centers(&self.grid, &self.solid, &keep),
            removed_voxel_indices: collect_removed_voxel_indices(&self.grid, &self.solid, &keep),
            blocked_voxel_centers,
            blocked_voxel_indices,
        })
    }
}

/// The cavity of an exact-wall hollow ([`HollowSession::exact_cavity`]).
#[derive(Debug, Clone)]
pub(crate) struct ExactCavity {
    /// The cavity wall, facing inward.
    pub wall: IndexedMesh,
    /// The infill beams, when the hollow has infill.
    pub infill: Option<IndexedMesh>,
    /// The wall and the beams are edge-manifold: a result can be built.
    pub closed: bool,
    pub removed_voxels: usize,
    /// Centres of the removed voxels on the cavity wall (x, y, z each).
    pub removed_voxel_centers: Vec<f32>,
    pub removed_voxel_indices: Vec<u32>,
    pub blocked_voxel_centers: Vec<f32>,
    pub blocked_voxel_indices: Vec<u32>,
}

/// The cavity wall of `keep` over exact distances, and whether it is closed
/// (edge-manifold, no degenerate triangle). Its edges are interpolated in
/// one order, so it is normally contoured closed and taken as it is; any
/// other goes through the repairs the hollow has always run (near seams
/// welded, open shards dropped, degenerate triangles out, the weld ladder),
/// which leave a closed wall unchanged.
fn exact_cavity_wall(
    grid: &GridSpec,
    solid: &[bool],
    keep: &[bool],
    dist: &[f32],
    shell_voxels_f: f32,
) -> (IndexedMesh, bool) {
    let negative: Vec<bool> = solid
        .par_iter()
        .zip(keep)
        .map(|(solid, kept)| *solid && !*kept)
        .collect();
    let scalar =
        build_smoothed_cavity_scalar_field(grid, solid, keep, dist, shell_voxels_f, 0, true);
    let mesh = organic_boundary_mesh(grid, keep, &negative, &scalar, true);
    if edge_manifold_defects(&mesh).is_clean() && !has_degenerate_triangle(&mesh) {
        return (mesh, true);
    }
    let (mesh, _score) =
        drop_open_cavity_fragments(stabilize_cavity_mesh_for_boolean(mesh, grid.voxel_mm));
    let mesh = normalize_mesh_for_boolean(mesh);
    match stabilize_hollow_mesh(&mesh) {
        Some(closed) => (closed, true),
        None => (mesh, false),
    }
}

/// A triangle [`normalize_mesh_for_boolean`] would drop: a repeated vertex
/// or no area.
fn has_degenerate_triangle(mesh: &IndexedMesh) -> bool {
    mesh.triangles.par_iter().any(|tri| {
        if tri[0] == tri[1] || tri[1] == tri[2] || tri[0] == tri[2] {
            return true;
        }
        let [a, b, c] = tri.map(|i| mesh.positions[i as usize]);
        b.sub(a).cross(c.sub(a)).length() * 0.5 <= 1e-16
    })
}

/// Whether the removed voxel at (x, y, z) is 6-adjacent to a voxel that is
/// kept material or outside the solid: only those sit on the visible cavity
/// wall.
#[inline]
fn is_removed_voxel_boundary(
    grid: &GridSpec,
    solid: &[bool],
    keep: &[bool],
    x: usize,
    y: usize,
    z: usize,
) -> bool {
    for (dx, dy, dz) in N6 {
        let (nx, ny, nz) = (x as isize + dx, y as isize + dy, z as isize + dz);
        if !grid.in_bounds(nx, ny, nz) {
            return true;
        }
        let n = grid.idx(nx as usize, ny as usize, nz as usize);
        if !solid[n] || keep[n] {
            return true;
        }
    }
    false
}

pub(crate) fn collect_removed_voxel_centers(
    grid: &GridSpec,
    solid: &[bool],
    keep: &[bool],
) -> Vec<f32> {
    let mut centers = Vec::new();
    for z in 0..grid.nz {
        for y in 0..grid.ny {
            for x in 0..grid.nx {
                let index = grid.idx(x, y, z);
                if !solid[index] || keep[index] {
                    continue;
                }
                if !is_removed_voxel_boundary(grid, solid, keep, x, y, z) {
                    continue;
                }
                centers.extend_from_slice(&grid.center_world(x, y, z).to_array());
            }
        }
    }
    centers
}

pub(crate) fn collect_removed_voxel_indices(
    grid: &GridSpec,
    solid: &[bool],
    keep: &[bool],
) -> Vec<u32> {
    let mut indices = Vec::new();
    for z in 0..grid.nz {
        for y in 0..grid.ny {
            for x in 0..grid.nx {
                let index = grid.idx(x, y, z);
                if !solid[index] || keep[index] {
                    continue;
                }
                if !is_removed_voxel_boundary(grid, solid, keep, x, y, z) {
                    continue;
                }
                indices.push(index as u32);
            }
        }
    }
    indices
}

/// Decodes committed blocked-voxel indices into centers, with DragonFruit's
/// acceptance rule (in bounds and solid). Centers and accepted indices stay
/// in lockstep.
pub(crate) fn collect_blocked_voxel_data(
    grid: &GridSpec,
    solid: &[bool],
    blocked_indices: &[usize],
) -> (Vec<f32>, Vec<u32>) {
    let mut centers = Vec::with_capacity(blocked_indices.len() * 3);
    let mut accepted = Vec::with_capacity(blocked_indices.len());
    for &index in blocked_indices {
        if index >= solid.len() || !solid[index] {
            continue;
        }
        let (x, y, z) = grid.coords(index);
        centers.extend_from_slice(&grid.center_world(x, y, z).to_array());
        accepted.push(index as u32);
    }
    (centers, accepted)
}

/// Ray-casting (even-odd) point-in-polygon test, DragonFruit's lasso
/// resolver with its zero-slope guard.
#[inline]
fn point_in_polygon(polygon: &[[f32; 2]], x: f32, y: f32) -> bool {
    let mut inside = false;
    let n = polygon.len();
    if n == 0 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let xi = polygon[i][0];
        let yi = polygon[i][1];
        let xj = polygon[j][0];
        let yj = polygon[j][1];
        let denom = if (yj - yi) != 0.0 { yj - yi } else { 1e-6 };
        let intersects = ((yi > y) != (yj > y)) && (x < ((xj - xi) * (y - yi)) / denom + xi);
        if intersects {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Applies a column-major 4x4 to a point, dividing by `w`.
#[inline]
fn apply_mat4(m: &[f32; 16], v: Vec3) -> Option<[f32; 4]> {
    let x = m[0] * v.x + m[4] * v.y + m[8] * v.z + m[12];
    let y = m[1] * v.x + m[5] * v.y + m[9] * v.z + m[13];
    let z = m[2] * v.x + m[6] * v.y + m[10] * v.z + m[14];
    let w = m[3] * v.x + m[7] * v.y + m[11] * v.z + m[15];
    if w == 0.0 { None } else { Some([x, y, z, w]) }
}

/// Projects a point to container pixels, DragonFruit's `projectWorldPoint`:
/// NDC must be finite with `z` in `[-1, 1]`.
#[inline]
fn project_world_to_pixel(view_proj: &[f32; 16], v: Vec3, rect: [f32; 2]) -> Option<(f32, f32)> {
    let [x, y, z, w] = apply_mat4(view_proj, v)?;
    let ndc_x = x / w;
    let ndc_y = y / w;
    let ndc_z = z / w;
    if !ndc_x.is_finite() || !ndc_y.is_finite() || !ndc_z.is_finite() {
        return None;
    }
    if !(-1.0..=1.0).contains(&ndc_z) {
        return None;
    }
    Some(((ndc_x + 1.0) * 0.5 * rect[0], (1.0 - ndc_y) * 0.5 * rect[1]))
}

/// Every removed voxel (`solid && !keep`, no boundary gate, so the full
/// through-depth column) whose projected center falls in `polygon`.
fn select_removed_voxels_in_polygon(
    grid: &GridSpec,
    solid: &[bool],
    keep: &[bool],
    polygon: &[[f32; 2]],
    session_to_world: &[f32; 16],
    view_proj: &[f32; 16],
    rect: [f32; 2],
) -> Vec<u32> {
    if polygon.len() < 3 {
        return Vec::new();
    }
    let mut selected = Vec::new();
    for z in 0..grid.nz {
        for y in 0..grid.ny {
            for x in 0..grid.nx {
                let index = grid.idx(x, y, z);
                if !solid[index] || keep[index] {
                    continue;
                }
                let Some([wx, wy, wz, ww]) =
                    apply_mat4(session_to_world, grid.center_world(x, y, z))
                else {
                    continue;
                };
                let world = Vec3::new(wx / ww, wy / ww, wz / ww);
                let Some((px, py)) = project_world_to_pixel(view_proj, world, rect) else {
                    continue;
                };
                if point_in_polygon(polygon, px, py) {
                    selected.push(index as u32);
                }
            }
        }
    }
    selected
}

const CUBE_CORNERS: [(usize, usize, usize); 8] = [
    (0, 0, 0),
    (1, 0, 0),
    (1, 1, 0),
    (0, 1, 0),
    (0, 0, 1),
    (1, 0, 1),
    (1, 1, 1),
    (0, 1, 1),
];

const TETRAHEDRA_IN_CUBE: [[usize; 4]; 6] = [
    [0, 5, 1, 6],
    [0, 1, 2, 6],
    [0, 2, 3, 6],
    [0, 3, 7, 6],
    [0, 7, 4, 6],
    [0, 4, 5, 6],
];

/// The isosurface of `scalar_field` between the classified voxels, by
/// marching tetrahedra. The z slabs run in parallel and are joined in
/// order: the same triangles, in the same order, as one sweep.
///
/// With `canonical_edges`, each grid edge is interpolated from the same end
/// whichever tetrahedron crosses it, so the tetrahedra on both sides put its
/// point on the same bits and the weld always joins them. In DragonFruit's
/// order (kept for its parity) the two can round into neighbouring weld
/// cells and leave a hairline crack the hollow then has to repair.
fn organic_boundary_mesh(
    grid: &GridSpec,
    positive: &[bool],
    negative: &[bool],
    scalar_field: &[f32],
    canonical_edges: bool,
) -> IndexedMesh {
    let slabs: Vec<Vec<f32>> = (0..grid.nz.saturating_sub(1))
        .into_par_iter()
        .map(|z| {
            let mut soup = Vec::<f32>::new();
            let mut corner_pos = [Vec3::ZERO; 8];
            let mut corner_scalar = [0.0f32; 8];
            let mut corner_kept = [false; 8];
            let mut corner_carved = [false; 8];
            for y in 0..grid.ny.saturating_sub(1) {
                for x in 0..grid.nx.saturating_sub(1) {
                    let mut has_scalar_positive = false;
                    let mut has_scalar_negative = false;

                    for (corner_i, &(dx, dy, dz)) in CUBE_CORNERS.iter().enumerate() {
                        let vx = x + dx;
                        let vy = y + dy;
                        let vz = z + dz;
                        let vi = grid.idx(vx, vy, vz);

                        corner_pos[corner_i] = grid.center_world(vx, vy, vz);
                        corner_kept[corner_i] = positive[vi];
                        corner_carved[corner_i] = negative[vi];
                        corner_scalar[corner_i] = scalar_field[vi];
                        if corner_kept[corner_i] || corner_carved[corner_i] {
                            if corner_scalar[corner_i] >= 0.0 {
                                has_scalar_positive = true;
                            } else {
                                has_scalar_negative = true;
                            }
                        }
                    }

                    // Cubes whose classified corners all share one scalar
                    // sign contain no isosurface.
                    if !(has_scalar_positive && has_scalar_negative) {
                        continue;
                    }

                    for tet in TETRAHEDRA_IN_CUBE {
                        polygonize_cavity_tetrahedron(
                            &mut soup,
                            tet,
                            &corner_pos,
                            &corner_scalar,
                            &corner_kept,
                            &corner_carved,
                            canonical_edges,
                        );
                    }
                }
            }
            soup
        })
        .collect();

    IndexedMesh::from_triangle_soup(&slabs.concat(), 1e-6)
}

/// How far, in voxels, an exact cavity field keeps its values from 0 (see
/// `build_smoothed_cavity_scalar_field`): the wall moves by at most this
/// much, 0.03 mm at DragonFruit's voxel.
const EXACT_WALL_SAMPLE_MARGIN: f32 = 0.05;

/// How far, in voxels, the smoothing of an exact cavity field may raise a
/// value (#266): the smoothed cavity wall stays within about this much of
/// the unsmoothed one, the preview's (0.16 mm at DragonFruit's voxel).
/// Over exact distances the blur hardly smooths anything (the unsmoothed
/// surface is as even as the smoothed one); it rounds inner creases and
/// thickens convex cavities evenly, so a small cap loses nothing.
pub(crate) const EXACT_SMOOTHING_CAP_VOXELS: f32 = 0.25;

fn build_smoothed_cavity_scalar_field(
    grid: &GridSpec,
    solid: &[bool],
    keep: &[bool],
    dist: &[f32],
    shell_voxels_f: f32,
    smoothing_iterations: usize,
    exact_wall: bool,
) -> Vec<f32> {
    let exterior_value = -2.5f32;
    let active_band_voxels = 4.5f32;
    // Each cell from its own inputs only: in parallel, the same values.
    let mut field: Vec<f32> = (0..solid.len())
        .into_par_iter()
        .map(|i| {
            if !solid[i] {
                return exterior_value;
            }
            let shell_signed = shell_voxels_f - dist[i];
            if exact_wall && keep[i] == (shell_signed >= 0.0) {
                // Off the sample by a hair: a value of 0 puts the wall on
                // the voxel centre, where its triangles collapse.
                if keep[i] {
                    shell_signed.max(EXACT_WALL_SAMPLE_MARGIN)
                } else {
                    shell_signed.min(-EXACT_WALL_SAMPLE_MARGIN)
                }
            } else if keep[i] {
                // Blocked voxels deep in the cavity get a small epsilon so the
                // zero-crossing sits just inside the blocked region.
                shell_signed.max(0.05 * shell_voxels_f.max(0.2))
            } else {
                shell_signed.min(-0.05 * shell_voxels_f.max(0.2))
            }
        })
        .collect();

    // The cells the blur reads: none without a blur.
    let mut active = vec![false; solid.len() * usize::from(smoothing_iterations > 0)];
    for z in 0..grid.nz * usize::from(smoothing_iterations > 0) {
        for y in 0..grid.ny {
            for x in 0..grid.nx {
                let i = grid.idx(x, y, z);
                if !solid[i] {
                    continue;
                }

                let center = field[i];
                let mut touches_sign_change = false;
                'outer: for dz in -1isize..=1 {
                    for dy in -1isize..=1 {
                        for dx in -1isize..=1 {
                            if dx == 0 && dy == 0 && dz == 0 {
                                continue;
                            }
                            let nx_i = x as isize + dx;
                            let ny_i = y as isize + dy;
                            let nz_i = z as isize + dz;
                            if !grid.in_bounds(nx_i, ny_i, nz_i) {
                                continue;
                            }
                            let ni = grid.idx(nx_i as usize, ny_i as usize, nz_i as usize);
                            if !solid[ni] {
                                continue;
                            }
                            if (center >= 0.0) != (field[ni] >= 0.0) {
                                touches_sign_change = true;
                                break 'outer;
                            }
                        }
                    }
                }

                if touches_sign_change || center.abs() <= active_band_voxels {
                    active[i] = true;
                }
            }
        }
    }

    // Over exact distances the blur is clamped (#266): each value stays
    // between the exact field and `cap` above it, and a cavity value stays
    // a cavity value. The smoothed wall is never thinner than the exact one,
    // and the cavity keeps the exact field's voxels, so it has the shape and
    // the topology of the preview's: no channel closes, no pocket seals.
    let exact_bounds = (exact_wall && smoothing_iterations > 0).then(|| field.clone());
    let mut scratch = if smoothing_iterations > 0 {
        field.clone()
    } else {
        Vec::new()
    };
    let plane = grid.nx * grid.ny;
    for _ in 0..smoothing_iterations {
        // Each cell reads the previous pass only: the slabs run in parallel
        // and give the same values as one sweep.
        scratch
            .par_chunks_mut(plane)
            .enumerate()
            .for_each(|(z, layer)| {
                for (k, out) in layer.iter_mut().enumerate() {
                    let (x, y) = (k % grid.nx, k / grid.nx);
                    let i = z * plane + k;
                    if !active[i] {
                        *out = field[i];
                        continue;
                    }

                    let mut sum = field[i] * 5.0;
                    let mut weight = 5.0;
                    for dz in -1isize..=1 {
                        for dy in -1isize..=1 {
                            for dx in -1isize..=1 {
                                if dx == 0 && dy == 0 && dz == 0 {
                                    continue;
                                }
                                let nx_i = x as isize + dx;
                                let ny_i = y as isize + dy;
                                let nz_i = z as isize + dz;
                                if !grid.in_bounds(nx_i, ny_i, nz_i) {
                                    continue;
                                }
                                let ni = grid.idx(nx_i as usize, ny_i as usize, nz_i as usize);
                                if !solid[ni] {
                                    continue;
                                }

                                let step = dx.abs() + dy.abs() + dz.abs();
                                let w = match step {
                                    1 => 2.5,
                                    2 => 1.25,
                                    _ => 0.6,
                                };
                                sum += field[ni] * w;
                                weight += w;
                            }
                        }
                    }
                    let blurred = sum / weight;
                    let smoothed = field[i] * 0.2 + blurred * 0.8;
                    *out = match &exact_bounds {
                        Some(exact) => {
                            clamp_smoothed(smoothed, exact[i], EXACT_SMOOTHING_CAP_VOXELS)
                        }
                        None => smoothed,
                    };
                }
            });
        std::mem::swap(&mut field, &mut scratch);
    }
    // Outer-skin guard: blur must never flip a kept voxel on the model's outer
    // surface negative, or the cavity exits through the skin at thin features.
    // Each cell reads only the masks: the slabs run in parallel.
    let skin_floor = 0.05 * shell_voxels_f.max(0.2);
    field
        .par_chunks_mut(plane)
        .enumerate()
        .for_each(|(z, layer)| {
            for y in 0..grid.ny {
                for x in 0..grid.nx {
                    let i = grid.idx(x, y, z);
                    if !keep[i] || !solid[i] {
                        continue;
                    }
                    let mut touches_outside = false;
                    for (dx, dy, dz) in N6 {
                        let nx_i = x as isize + dx;
                        let ny_i = y as isize + dy;
                        let nz_i = z as isize + dz;
                        if !grid.in_bounds(nx_i, ny_i, nz_i) {
                            touches_outside = true;
                            break;
                        }
                        let ni = grid.idx(nx_i as usize, ny_i as usize, nz_i as usize);
                        if !solid[ni] {
                            touches_outside = true;
                            break;
                        }
                    }
                    if touches_outside {
                        let k = i - z * plane;
                        layer[k] = layer[k].max(skin_floor);
                    }
                }
            }
        });

    field
}

/// A smoothed cavity value over exact distances: never below the exact
/// value `exact` (no material removed), at most `cap` above it, and on the
/// same side of the wall (a cavity voxel stays at least
/// [`EXACT_WALL_SAMPLE_MARGIN`] inside the cavity).
fn clamp_smoothed(smoothed: f32, exact: f32, cap: f32) -> f32 {
    let mut high = exact + cap;
    if exact < 0.0 {
        high = high.min(-EXACT_WALL_SAMPLE_MARGIN);
    }
    smoothed.min(high).max(exact)
}

/// A visual inward surface from the same keep mask as the final mesh.
/// Deliberately skips smoothing, stabilization, source merging and validation.
pub(crate) fn build_visual_cavity_mesh(
    grid: &GridSpec,
    solid: &[bool],
    dist: &[f32],
    keep: &[bool],
    shell_voxels_f: f32,
    exact_wall: bool,
) -> IndexedMesh {
    let negative: Vec<bool> = solid
        .iter()
        .zip(keep)
        .map(|(solid, kept)| *solid && !*kept)
        .collect();
    let scalar =
        build_smoothed_cavity_scalar_field(grid, solid, keep, dist, shell_voxels_f, 0, exact_wall);
    organic_boundary_mesh(grid, keep, &negative, &scalar, exact_wall)
}

#[allow(clippy::too_many_arguments)]
fn build_hollow_output_mesh(
    source_mesh: &IndexedMesh,
    source_bbox: &Aabb,
    grid: &GridSpec,
    solid: &[bool],
    dist: &[f32],
    keep: &[bool],
    options: &HollowOptions,
    shell_voxels_f: f32,
    smoothing_profile: InternalCavitySmoothingProfile,
) -> (IndexedMesh, IndexedMesh, usize) {
    let (cavity_mesh, cavity_wall_score) = build_cavity_inner_mesh(
        source_bbox,
        grid,
        solid,
        dist,
        keep,
        options,
        shell_voxels_f,
        smoothing_profile,
    );
    let out_mesh = if options.preview_cavity_only {
        cavity_mesh.clone()
    } else {
        let filtered_source =
            filter_source_mesh_for_openings(source_mesh, options, source_bbox, grid.voxel_mm);
        merge_meshes(&filtered_source, &cavity_mesh)
    };

    (
        normalize_mesh_for_boolean(out_mesh),
        cavity_mesh,
        cavity_wall_score,
    )
}

/// Builds only the internal cavity (+ infill) mesh without the outer shell:
/// the part that depends on the smoothing profile.
#[allow(clippy::too_many_arguments)]
fn build_cavity_inner_mesh(
    source_bbox: &Aabb,
    grid: &GridSpec,
    solid: &[bool],
    dist: &[f32],
    keep: &[bool],
    options: &HollowOptions,
    shell_voxels_f: f32,
    smoothing_profile: InternalCavitySmoothingProfile,
) -> (IndexedMesh, usize) {
    let cavity_negative: Vec<bool> = solid
        .iter()
        .zip(keep.iter())
        .map(|(is_solid, is_kept)| *is_solid && !*is_kept)
        .collect();

    let cavity_scalar = build_smoothed_cavity_scalar_field(
        grid,
        solid,
        keep,
        dist,
        shell_voxels_f,
        smoothing_profile.scalar_field_blur_iterations,
        options.exact_wall,
    );
    let (cavity_mesh, wall_defect_score) =
        drop_open_cavity_fragments(stabilize_cavity_mesh_for_boolean(
            smooth_cavity_mesh(
                organic_boundary_mesh(
                    grid,
                    keep,
                    &cavity_negative,
                    &cavity_scalar,
                    options.exact_wall,
                ),
                grid.voxel_mm,
                smoothing_profile.taubin_iterations,
                smoothing_profile.taubin_max_step_scale,
            ),
            grid.voxel_mm,
        ));

    let infill_mesh = if matches!(options.mode, HollowMode::Infill) {
        build_smooth_infill_mesh(
            source_bbox,
            grid,
            solid,
            keep,
            options.infill_mode,
            options.infill_cell_mm,
            options.infill_beam_radius_mm,
        )
    } else {
        IndexedMesh::default()
    };

    // Infill beams are closed tubes by construction; the cavity wall's defect
    // score is the meaningful failure predictor.
    let combined = if infill_mesh.triangles.is_empty() {
        cavity_mesh
    } else if cavity_mesh.triangles.is_empty() {
        infill_mesh
    } else {
        merge_meshes(&cavity_mesh, &infill_mesh)
    };
    (combined, wall_defect_score)
}

fn polygonize_cavity_tetrahedron(
    soup: &mut Vec<f32>,
    tet: [usize; 4],
    positions: &[Vec3; 8],
    scalar: &[f32; 8],
    kept: &[bool; 8],
    carved: &[bool; 8],
    canonical_edges: bool,
) {
    let tet_edges = [(0usize, 1usize), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)];
    let mut intersections = [Vec3::ZERO; 4];
    let mut intersection_count = 0usize;
    let mut side = [None; 4];
    let mut scalar_positive_count = 0usize;
    let mut scalar_negative_count = 0usize;

    for (local_i, &corner_i) in tet.iter().enumerate() {
        if !(kept[corner_i] || carved[corner_i]) {
            continue;
        }

        let is_positive = scalar[corner_i] >= 0.0;
        side[local_i] = Some(is_positive);
        if is_positive {
            scalar_positive_count += 1;
        } else {
            scalar_negative_count += 1;
        }
    }

    // Contour strictly by the scalar field's sign: a tet whose classified
    // corners share one sign contains no isosurface and emits nothing (the
    // old hard-label fallback emitted detached "shard" triangles).
    if scalar_positive_count == 0 || scalar_negative_count == 0 {
        return;
    }

    for (ea, eb) in tet_edges {
        let mut ia = tet[ea];
        let mut ib = tet[eb];
        let Some(a_positive) = side[ea] else {
            continue;
        };
        let Some(b_positive) = side[eb] else {
            continue;
        };

        if a_positive == b_positive {
            continue;
        }
        if canonical_edges && precedes(positions[ib], positions[ia]) {
            std::mem::swap(&mut ia, &mut ib);
        }

        let pa = positions[ia];
        let pb = positions[ib];
        let va = scalar[ia];
        let vb = scalar[ib];
        let denom = va - vb;
        let t = if denom.abs() <= 1e-6 {
            0.5
        } else {
            (va / denom).clamp(0.0, 1.0)
        };

        intersections[intersection_count] = pa.add(pb.sub(pa).scale(t));
        intersection_count += 1;
    }

    if intersection_count < 3 {
        return;
    }

    let mut positive_centroid = Vec3::ZERO;
    let mut negative_centroid = Vec3::ZERO;
    let mut positive_count = 0usize;
    let mut negative_count = 0usize;
    for (local_i, &corner_i) in tet.iter().enumerate() {
        match side[local_i] {
            Some(true) => {
                positive_centroid = positive_centroid.add(positions[corner_i]);
                positive_count += 1;
            }
            Some(false) => {
                negative_centroid = negative_centroid.add(positions[corner_i]);
                negative_count += 1;
            }
            None => {}
        }
    }

    if positive_count == 0 || negative_count == 0 {
        return;
    }

    positive_centroid = positive_centroid.scale(1.0 / positive_count as f32);
    negative_centroid = negative_centroid.scale(1.0 / negative_count as f32);
    let desired_normal = negative_centroid.sub(positive_centroid);

    if intersection_count == 3 {
        emit_oriented_triangle(
            soup,
            intersections[0],
            intersections[1],
            intersections[2],
            desired_normal,
        );
        return;
    }

    if intersection_count == 4 {
        let center = intersections[0]
            .add(intersections[1])
            .add(intersections[2])
            .add(intersections[3])
            .scale(0.25);

        let mut ordered = intersections;
        sort_points_around_axis(&mut ordered, center, desired_normal);

        emit_oriented_triangle(soup, ordered[0], ordered[1], ordered[2], desired_normal);
        emit_oriented_triangle(soup, ordered[0], ordered[2], ordered[3], desired_normal);
    }
}

/// Whether `a` comes before `b` in (z, y, x) order: which end of a grid edge
/// its interpolation starts from.
fn precedes(a: Vec3, b: Vec3) -> bool {
    (a.z, a.y, a.x) < (b.z, b.y, b.x)
}

fn emit_oriented_triangle(soup: &mut Vec<f32>, a: Vec3, b: Vec3, c: Vec3, desired_normal: Vec3) {
    let normal = b.sub(a).cross(c.sub(a));
    if normal.dot(desired_normal) < 0.0 {
        soup.extend_from_slice(&[a.x, a.y, a.z, c.x, c.y, c.z, b.x, b.y, b.z]);
    } else {
        soup.extend_from_slice(&[a.x, a.y, a.z, b.x, b.y, b.z, c.x, c.y, c.z]);
    }
}

fn sort_points_around_axis(points: &mut [Vec3; 4], center: Vec3, axis: Vec3) {
    let axis = vec3_normalize(axis).unwrap_or(Vec3::new(0.0, 0.0, 1.0));
    let helper = if axis.z.abs() < 0.95 {
        Vec3::new(0.0, 0.0, 1.0)
    } else {
        Vec3::new(0.0, 1.0, 0.0)
    };
    let u = vec3_normalize(helper.cross(axis)).unwrap_or(Vec3::new(1.0, 0.0, 0.0));
    let v = vec3_normalize(axis.cross(u)).unwrap_or(Vec3::new(0.0, 1.0, 0.0));

    points.sort_by(|a, b| {
        let da = (*a).sub(center);
        let db = (*b).sub(center);
        let aa = da.dot(v).atan2(da.dot(u));
        let ab = db.dot(v).atan2(db.dot(u));
        aa.total_cmp(&ab)
    });
}

/// Taubin-smooths the cavity surface with locked boundary vertices and a
/// voxel-scaled step.
fn smooth_cavity_mesh(
    mesh: IndexedMesh,
    voxel_mm: f32,
    iterations: usize,
    max_step_scale: f32,
) -> IndexedMesh {
    let mut out = mesh;
    crate::smoothing::taubin_smooth(
        &mut out,
        iterations,
        voxel_mm,
        (voxel_mm * max_step_scale).max(0.01),
    );
    out
}

fn stabilize_cavity_mesh_for_boolean(mesh: IndexedMesh, voxel_mm: f32) -> IndexedMesh {
    if mesh.triangles.is_empty() || mesh.positions.is_empty() {
        return mesh;
    }

    let topo = Topology::build(&mesh);
    let boundary_edges = topo.boundary_edges().len();
    let non_manifold_edges = topo.non_manifold_edges().len();
    if boundary_edges == 0 && non_manifold_edges == 0 {
        return mesh;
    }

    let bbox_diag = mesh.bbox().diag().max(1e-6);
    let mut best_mesh = mesh;
    let mut best_score = boundary_edges + non_manifold_edges * 4;

    for absolute_weld_mm in [voxel_mm * 0.015, voxel_mm * 0.035, voxel_mm * 0.075] {
        let weld_epsilon = (absolute_weld_mm / bbox_diag).clamp(1e-7, 1e-3);
        let candidate = normalize_mesh_for_boolean_with_weld(best_mesh.clone(), weld_epsilon);
        let candidate_topo = Topology::build(&candidate);
        let candidate_boundary = candidate_topo.boundary_edges().len();
        let candidate_non_manifold = candidate_topo.non_manifold_edges().len();
        let candidate_score = candidate_boundary + candidate_non_manifold * 4;

        if candidate_score < best_score
            || (candidate_score == best_score
                && candidate.triangle_count() >= best_mesh.triangle_count().saturating_sub(8))
        {
            best_score = candidate_score;
            best_mesh = candidate;
        }

        if best_score == 0 {
            break;
        }
    }

    best_mesh
}

fn apply_internal_cavity_chamfer_pass(
    grid: &GridSpec,
    solid: &[bool],
    keep: &mut [bool],
    dist: &[f32],
) {
    let mut carve = vec![false; keep.len()];

    for z in 0..grid.nz {
        for y in 0..grid.ny {
            for x in 0..grid.nx {
                let i = grid.idx(x, y, z);
                if !keep[i] || !solid[i] {
                    continue;
                }

                // Only bevel deeper shell voxels: never within one diagonal
                // step of the surface, so convex exterior corners keep their
                // margin.
                if dist[i] <= SQRT_2 {
                    continue;
                }

                let mut cavity_x = false;
                let mut cavity_y = false;
                let mut cavity_z = false;

                for (dx, dy, dz) in N6 {
                    let nx_i = x as isize + dx;
                    let ny_i = y as isize + dy;
                    let nz_i = z as isize + dz;
                    if !grid.in_bounds(nx_i, ny_i, nz_i) {
                        continue;
                    }

                    let ni = grid.idx(nx_i as usize, ny_i as usize, nz_i as usize);
                    if solid[ni] && !keep[ni] {
                        if dx != 0 {
                            cavity_x = true;
                        }
                        if dy != 0 {
                            cavity_y = true;
                        }
                        if dz != 0 {
                            cavity_z = true;
                        }
                    }
                }

                let axis_count = (cavity_x as u8) + (cavity_y as u8) + (cavity_z as u8);
                if axis_count >= 2 {
                    carve[i] = true;
                }
            }
        }
    }

    for (i, should_carve) in carve.into_iter().enumerate() {
        if should_carve {
            keep[i] = false;
        }
    }
}

fn effective_internal_cavity_chamfer_passes(
    shell_thickness_mm: f32,
    shell_voxels_f: f32,
    requested_passes: u8,
) -> u8 {
    if requested_passes == 0 {
        return 0;
    }

    if shell_thickness_mm < 1.5 {
        return 0;
    }

    // Thin shells skip chamfering (it can punch through or leave brittle
    // seams); thicker shells unlock one or two passes.
    let max_passes = if shell_voxels_f < 2.5 {
        0
    } else if shell_voxels_f < 4.0 {
        1
    } else {
        2
    };

    requested_passes.min(max_passes)
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct InternalCavitySmoothingProfile {
    scalar_field_blur_iterations: usize,
    taubin_iterations: usize,
    taubin_max_step_scale: f32,
}

impl InternalCavitySmoothingProfile {
    /// Over exact distances the cavity is smoothed in its field only, where
    /// the blur is clamped (#266): Taubin moves the mesh both ways, into the
    /// wall too.
    fn for_wall(self, exact_wall: bool) -> Self {
        if exact_wall {
            Self {
                taubin_iterations: 0,
                ..self
            }
        } else {
            self
        }
    }
}

fn effective_internal_cavity_smoothing_profile(
    shell_thickness_mm: f32,
    requested: bool,
    shell_voxels_f: f32,
) -> InternalCavitySmoothingProfile {
    if !requested {
        return InternalCavitySmoothingProfile {
            scalar_field_blur_iterations: 0,
            taubin_iterations: 0,
            taubin_max_step_scale: 0.42,
        };
    }

    // Thin shells get a light pass that cannot pinch the wall.
    if shell_thickness_mm < 1.5 || shell_voxels_f < 2.5 {
        return InternalCavitySmoothingProfile {
            scalar_field_blur_iterations: 3,
            taubin_iterations: 8,
            taubin_max_step_scale: 0.38,
        };
    }

    if shell_voxels_f < 3.5 {
        return InternalCavitySmoothingProfile {
            scalar_field_blur_iterations: 6,
            taubin_iterations: 12,
            taubin_max_step_scale: 0.50,
        };
    }

    InternalCavitySmoothingProfile {
        scalar_field_blur_iterations: 9,
        taubin_iterations: 18,
        taubin_max_step_scale: 0.62,
    }
}

/// Drops vertices no triangle references and remaps indices in place.
pub(crate) fn compact_unreferenced_vertices(mesh: &mut IndexedMesh) {
    let mut used = vec![false; mesh.positions.len()];
    for tri in &mesh.triangles {
        used[tri[0] as usize] = true;
        used[tri[1] as usize] = true;
        used[tri[2] as usize] = true;
    }
    if used.iter().all(|&u| u) {
        return;
    }
    let mut remap = vec![u32::MAX; mesh.positions.len()];
    let mut next = 0u32;
    let mut new_positions = Vec::with_capacity(mesh.positions.len());
    for (i, &is_used) in used.iter().enumerate() {
        if is_used {
            remap[i] = next;
            new_positions.push(mesh.positions[i]);
            next += 1;
        }
    }
    for tri in &mut mesh.triangles {
        tri[0] = remap[tri[0] as usize];
        tri[1] = remap[tri[1] as usize];
        tri[2] = remap[tri[2] as usize];
    }
    mesh.positions = new_positions;
}

/// Removes detached open "shard" fragments from a generated cavity wall: edge
/// components other than the largest that carry boundary edges. Returns the
/// cleaned mesh and its residual defect score
/// (`boundary_edges + 4 * non_manifold_edges`).
fn drop_open_cavity_fragments(mesh: IndexedMesh) -> (IndexedMesh, usize) {
    if mesh.triangles.is_empty() {
        return (mesh, 0);
    }

    let topo = Topology::build(&mesh);
    let boundary_edges = topo.boundary_edges();
    let non_manifold_count = topo.non_manifold_edges().len();
    if boundary_edges.is_empty() {
        return (mesh, non_manifold_count * 4);
    }

    fn find(parent: &mut [u32], mut i: u32) -> u32 {
        while parent[i as usize] != i {
            parent[i as usize] = parent[parent[i as usize] as usize];
            i = parent[i as usize];
        }
        i
    }

    // Union-find over faces sharing an edge (any multiplicity).
    let face_count = mesh.triangles.len();
    let mut parent: Vec<u32> = (0..face_count as u32).collect();
    for info in topo.edges.values() {
        let mut faces = info.faces.iter();
        if let Some(&first) = faces.next() {
            let root = find(&mut parent, first);
            for &other in faces {
                let other_root = find(&mut parent, other);
                parent[other_root as usize] = root;
            }
        }
    }

    let mut component_size = vec![0usize; face_count];
    for face in 0..face_count as u32 {
        let root = find(&mut parent, face);
        component_size[root as usize] += 1;
    }
    let mut component_open = vec![false; face_count];
    for key in &boundary_edges {
        if let Some(info) = topo.edges.get(key) {
            for &face in &info.faces {
                let root = find(&mut parent, face);
                component_open[root as usize] = true;
            }
        }
    }
    let largest_root = (0..face_count)
        .max_by_key(|&i| component_size[i])
        .unwrap_or(0) as u32;

    let retained: Vec<[u32; 3]> = mesh
        .triangles
        .iter()
        .enumerate()
        .filter(|(face, _)| {
            let root = find(&mut parent, *face as u32);
            root == largest_root || !component_open[root as usize]
        })
        .map(|(_, tri)| *tri)
        .collect();

    if retained.len() == mesh.triangles.len() {
        return (mesh, boundary_edges.len() + non_manifold_count * 4);
    }

    let mut cleaned = IndexedMesh {
        positions: mesh.positions,
        triangles: retained,
    };
    compact_unreferenced_vertices(&mut cleaned);

    let cleaned_topo = Topology::build(&cleaned);
    let score = cleaned_topo.boundary_edges().len() + cleaned_topo.non_manifold_edges().len() * 4;
    (cleaned, score)
}

/// Cheap topology-level defect summary, used to decide whether weld retries
/// can possibly help.
#[derive(Debug, Clone, Copy)]
struct MeshDefectSummary {
    boundary_edges: usize,
    non_manifold_edges: usize,
    inconsistent_edges: usize,
}

fn summarize_mesh_defects(mesh: &IndexedMesh) -> MeshDefectSummary {
    let topo = Topology::build(mesh);
    MeshDefectSummary {
        boundary_edges: topo.boundary_edges().len(),
        non_manifold_edges: topo.non_manifold_edges().len(),
        inconsistent_edges: topo.inconsistent_edges(),
    }
}

/// Welding can only close hairline boundary cracks; it never repairs edges of
/// more than two faces nor winding, so retries against those are futile.
fn weld_retries_worthwhile(defects: &MeshDefectSummary) -> bool {
    defects.non_manifold_edges == 0 && defects.inconsistent_edges == 0 && defects.boundary_edges > 0
}

pub(crate) fn normalize_mesh_for_boolean(mesh: IndexedMesh) -> IndexedMesh {
    normalize_mesh_for_boolean_with_weld(mesh, 1e-6)
}

pub(crate) fn normalize_mesh_for_boolean_with_weld(
    mesh: IndexedMesh,
    weld_epsilon: f32,
) -> IndexedMesh {
    let weld_epsilon = weld_epsilon.clamp(1e-7, 1e-3);
    let mut normalized = IndexedMesh::from_triangle_soup(&mesh.to_triangle_soup(), weld_epsilon);
    let positions = normalized.positions.clone();
    normalized.triangles.retain(|tri| {
        if tri[0] == tri[1] || tri[1] == tri[2] || tri[0] == tri[2] {
            return false;
        }

        let a = positions[tri[0] as usize];
        let b = positions[tri[1] as usize];
        let c = positions[tri[2] as usize];
        let area = b.sub(a).cross(c.sub(a)).length() * 0.5;
        area > 1e-16
    });

    if normalized.triangles.len() != mesh.triangles.len() {
        // Degenerate-triangle removal can orphan vertices.
        compact_unreferenced_vertices(&mut normalized);
    }

    normalized
}

/// DragonFruit's `stabilize_hollow_mesh_for_manifold` with the manifold3d round
/// trip replaced by the edge-manifold check: the mesh as is, else the weld
/// ladder when welding can help. `None` when nothing passes.
fn stabilize_hollow_mesh(mesh: &IndexedMesh) -> Option<IndexedMesh> {
    if is_edge_manifold(mesh) {
        return Some(mesh.clone());
    }
    if weld_retries_worthwhile(&summarize_mesh_defects(mesh)) {
        for weld_epsilon in [2e-6_f32, 5e-6_f32, 1e-5_f32] {
            let candidate = normalize_mesh_for_boolean_with_weld(mesh.clone(), weld_epsilon);
            if is_edge_manifold(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn retain_largest_connected_cavity_component(grid: &GridSpec, solid: &[bool], keep: &mut [bool]) {
    let mut component_ids = vec![-1i32; keep.len()];
    let mut component_sizes = Vec::<usize>::new();
    let mut queue = VecDeque::<(usize, usize, usize)>::new();

    for z in 0..grid.nz {
        for y in 0..grid.ny {
            for x in 0..grid.nx {
                let start_idx = grid.idx(x, y, z);
                if !solid[start_idx] || keep[start_idx] || component_ids[start_idx] >= 0 {
                    continue;
                }

                let component_id = component_sizes.len() as i32;
                component_ids[start_idx] = component_id;
                queue.push_back((x, y, z));

                let mut size = 0usize;
                while let Some((cx, cy, cz)) = queue.pop_front() {
                    size += 1;

                    for (dx, dy, dz) in N6 {
                        let nx_i = cx as isize + dx;
                        let ny_i = cy as isize + dy;
                        let nz_i = cz as isize + dz;
                        if !grid.in_bounds(nx_i, ny_i, nz_i) {
                            continue;
                        }

                        let ux = nx_i as usize;
                        let uy = ny_i as usize;
                        let uz = nz_i as usize;
                        let ni = grid.idx(ux, uy, uz);

                        if !solid[ni] || keep[ni] || component_ids[ni] >= 0 {
                            continue;
                        }

                        component_ids[ni] = component_id;
                        queue.push_back((ux, uy, uz));
                    }
                }

                component_sizes.push(size);
            }
        }
    }

    if component_sizes.len() <= 1 {
        return;
    }

    let mut largest_component_id = 0i32;
    let mut largest_size = 0usize;
    for (idx, size) in component_sizes.iter().enumerate() {
        if *size > largest_size {
            largest_size = *size;
            largest_component_id = idx as i32;
        }
    }

    for i in 0..keep.len() {
        if solid[i] && !keep[i] && component_ids[i] != largest_component_id {
            keep[i] = true;
        }
    }
}

pub(crate) fn merge_meshes(a: &IndexedMesh, b: &IndexedMesh) -> IndexedMesh {
    if b.triangles.is_empty() {
        return a.clone();
    }
    if a.triangles.is_empty() {
        return b.clone();
    }

    let mut out = IndexedMesh {
        positions: Vec::with_capacity(a.positions.len() + b.positions.len()),
        triangles: Vec::with_capacity(a.triangles.len() + b.triangles.len()),
    };

    out.positions.extend_from_slice(&a.positions);
    out.triangles.extend_from_slice(&a.triangles);

    let index_offset = out.positions.len() as u32;
    out.positions.extend_from_slice(&b.positions);
    for tri in &b.triangles {
        out.triangles.push([
            tri[0] + index_offset,
            tri[1] + index_offset,
            tri[2] + index_offset,
        ]);
    }

    out
}

fn filter_source_mesh_for_openings(
    mesh: &IndexedMesh,
    options: &HollowOptions,
    bbox: &Aabb,
    voxel_mm: f32,
) -> IndexedMesh {
    let mut out = IndexedMesh {
        positions: mesh.positions.clone(),
        triangles: Vec::with_capacity(mesh.triangles.len()),
    };

    let shell_cut_depth = options.shell_thickness_mm.max(voxel_mm * 1.5);

    for tri in &mesh.triangles {
        let a = mesh.positions[tri[0] as usize];
        let b = mesh.positions[tri[1] as usize];
        let c = mesh.positions[tri[2] as usize];
        let centroid = a.add(b).add(c).scale(1.0 / 3.0);

        let mut drop = false;

        if matches!(options.mode, HollowMode::ShellOpenFace) {
            let dist_to_open_face = match options.open_face {
                OpenFace::XMin => centroid.x - bbox.min.x,
                OpenFace::XMax => bbox.max.x - centroid.x,
                OpenFace::YMin => centroid.y - bbox.min.y,
                OpenFace::YMax => bbox.max.y - centroid.y,
                OpenFace::ZMin => centroid.z - bbox.min.z,
                OpenFace::ZMax => bbox.max.z - centroid.z,
            };
            if dist_to_open_face <= shell_cut_depth {
                drop = true;
            }
        }

        if !drop && matches!(options.mode, HollowMode::Cavity) && !options.drain_holes.is_empty() {
            for hole in &options.drain_holes {
                if point_in_drain_hole_cylinder(centroid, hole, bbox, voxel_mm) {
                    drop = true;
                    break;
                }
            }
        }

        if !drop {
            out.triangles.push(*tri);
        }
    }

    out
}

#[inline]
fn bbox_point(bbox: &Aabb, norm: [f32; 3]) -> Vec3 {
    Vec3::new(
        bbox.min.x + (bbox.max.x - bbox.min.x) * norm[0],
        bbox.min.y + (bbox.max.y - bbox.min.y) * norm[1],
        bbox.min.z + (bbox.max.z - bbox.min.z) * norm[2],
    )
}

fn point_in_drain_hole_cylinder(p: Vec3, hole: &DrainHoleSpec, bbox: &Aabb, voxel_mm: f32) -> bool {
    // A hole centered outside the bbox cannot contain a point of the mesh.
    if hole.center_norm.iter().any(|&c| !(0.0..=1.0).contains(&c)) {
        return false;
    }
    let center = bbox_point(bbox, hole.center_norm);

    let (axis, length_to_surface) = hole_axis_and_length(
        hole.direction,
        hole.center_norm,
        hole.length_mm,
        bbox,
        voxel_mm,
    );

    let r = hole.radius_mm.max(voxel_mm * 0.75) * 1.2;
    let d = p.sub(center);
    let proj = d.dot(axis);
    if proj < -voxel_mm || proj > length_to_surface + voxel_mm {
        return false;
    }

    let radial_sq = d.dot(d) - (proj * proj);
    radial_sq <= r * r
}

fn apply_drain_hole_corridor(
    grid: &GridSpec,
    keep: &mut [bool],
    hole: &DrainHoleSpec,
    bbox: &Aabb,
    voxel_mm: f32,
) {
    if hole.center_norm.iter().any(|&c| !(0.0..=1.0).contains(&c)) {
        return;
    }
    let center = bbox_point(bbox, hole.center_norm);
    let (axis, length_to_surface) = hole_axis_and_length(
        hole.direction,
        hole.center_norm,
        hole.length_mm,
        bbox,
        voxel_mm,
    );

    let radius = hole.radius_mm.max(voxel_mm * 0.75) * 1.15;
    let radius_sq = radius * radius;
    let corridor_pad = voxel_mm * 1.5;
    let corridor_min = -corridor_pad;
    let corridor_max = length_to_surface + corridor_pad;

    let (min_x, max_x, min_y, max_y, min_z, max_z) =
        corridor_index_bounds(grid, center, axis, length_to_surface, radius, corridor_pad);

    for z in min_z..=max_z {
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let i = grid.idx(x, y, z);
                if !keep[i] {
                    continue;
                }

                let p = grid.center_world(x, y, z);
                let d = p.sub(center);
                let proj = d.dot(axis);
                if proj < corridor_min || proj > corridor_max {
                    continue;
                }

                let radial_sq = d.dot(d) - (proj * proj);
                if radial_sq <= radius_sq {
                    keep[i] = false;
                }
            }
        }
    }
}

fn corridor_index_bounds(
    grid: &GridSpec,
    center: Vec3,
    axis: Vec3,
    length_to_surface: f32,
    radius: f32,
    pad: f32,
) -> (usize, usize, usize, usize, usize, usize) {
    let end = center.add(axis.scale(length_to_surface.max(0.0)));
    let reach = radius + pad + grid.voxel_mm;

    let min_w = center.min(end).sub(Vec3::new(reach, reach, reach));
    let max_w = center.max(end).add(Vec3::new(reach, reach, reach));

    let to_index_min = |value: f32, min_world: f32| -> isize {
        ((value - min_world) / grid.voxel_mm).floor() as isize
    };
    let to_index_max = |value: f32, min_world: f32| -> isize {
        ((value - min_world) / grid.voxel_mm).ceil() as isize
    };

    let min_x = to_index_min(min_w.x, grid.min.x).clamp(0, grid.nx as isize - 1) as usize;
    let max_x = to_index_max(max_w.x, grid.min.x).clamp(0, grid.nx as isize - 1) as usize;
    let min_y = to_index_min(min_w.y, grid.min.y).clamp(0, grid.ny as isize - 1) as usize;
    let max_y = to_index_max(max_w.y, grid.min.y).clamp(0, grid.ny as isize - 1) as usize;
    let min_z = to_index_min(min_w.z, grid.min.z).clamp(0, grid.nz as isize - 1) as usize;
    let max_z = to_index_max(max_w.z, grid.min.z).clamp(0, grid.nz as isize - 1) as usize;

    (min_x, max_x, min_y, max_y, min_z, max_z)
}

fn build_smooth_infill_mesh(
    source_bbox: &Aabb,
    grid: &GridSpec,
    solid: &[bool],
    keep: &[bool],
    infill_mode: InfillMode,
    infill_cell_mm: f32,
    infill_beam_radius_mm: f32,
) -> IndexedMesh {
    let spacing_mm = infill_cell_mm.clamp(3.0, 24.0);
    let radius_mm = infill_beam_radius_mm.clamp(0.25, 3.0);
    let sample_step = (grid.voxel_mm * 0.75).clamp(0.2, 1.2);
    let embed_pad = (radius_mm * 0.8).max(grid.voxel_mm * 0.8);
    let min_run_length = spacing_mm * 0.45;
    let circumference = std::f32::consts::TAU * radius_mm;
    let radial_segments = ((circumference / 0.45).ceil() as usize).clamp(14, 56);

    let center = source_bbox.min.add(source_bbox.max).scale(0.5);
    let extent = source_bbox.max.sub(source_bbox.min).scale(0.5);
    let dir_extent_pad = extent.length() + spacing_mm * 1.5;

    let directions: &[Vec3] = match infill_mode {
        InfillMode::Lattice => &[
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(1.0, 1.0, -1.0),
            Vec3::new(1.0, -1.0, 1.0),
            Vec3::new(1.0, -1.0, -1.0),
        ],
        InfillMode::Pillar => &[Vec3::new(0.0, 0.0, 1.0)],
    };

    let beam = BeamShape {
        embed_pad,
        min_run_length,
        radius_mm,
        radial_segments,
    };
    let mut soup = Vec::<f32>::new();
    for direction in directions {
        let axis = vec3_normalize(*direction).unwrap_or(Vec3::new(1.0, 1.0, 1.0));
        let helper = if axis.z.abs() < 0.95 {
            Vec3::new(0.0, 0.0, 1.0)
        } else {
            Vec3::new(0.0, 1.0, 0.0)
        };
        let basis_u = vec3_normalize(helper.cross(axis)).unwrap_or(Vec3::new(1.0, 0.0, 0.0));
        let basis_v = vec3_normalize(axis.cross(basis_u)).unwrap_or(Vec3::new(0.0, 1.0, 0.0));

        let u_extent = projected_half_extent(extent, basis_u) + spacing_mm;
        let v_extent = projected_half_extent(extent, basis_v) + spacing_mm;
        let line_span = dir_extent_pad * 2.0;
        let line_sample_count = (line_span / sample_step).ceil() as usize;
        let u_steps = ((u_extent * 2.0) / spacing_mm).ceil() as isize;
        let v_steps = ((v_extent * 2.0) / spacing_mm).ceil() as isize;

        for u_step in 0..=u_steps {
            let u_offset = -u_extent + u_step as f32 * spacing_mm;
            for v_step in 0..=v_steps {
                let v_offset = -v_extent + v_step as f32 * spacing_mm;
                let line_origin = center
                    .add(basis_u.scale(u_offset))
                    .add(basis_v.scale(v_offset))
                    .sub(axis.scale(dir_extent_pad));

                let mut run_start: Option<f32> = None;
                let mut last_inside_t = 0.0f32;
                for sample_idx in 0..=line_sample_count {
                    let t = sample_idx as f32 * sample_step;
                    let point = line_origin.add(axis.scale(t));
                    let inside = point_samples_carved_cavity(grid, solid, keep, point);
                    if inside {
                        if run_start.is_none() {
                            run_start = Some(t);
                        }
                        last_inside_t = t;
                    } else if let Some(start_t) = run_start.take() {
                        beam.append(&mut soup, line_origin, axis, start_t, last_inside_t);
                    }
                }

                if let Some(start_t) = run_start.take() {
                    beam.append(&mut soup, line_origin, axis, start_t, last_inside_t);
                }
            }
        }
    }

    if soup.is_empty() {
        return IndexedMesh::default();
    }

    normalize_mesh_for_boolean(IndexedMesh::from_triangle_soup(&soup, 1e-6))
}

struct BeamShape {
    embed_pad: f32,
    min_run_length: f32,
    radius_mm: f32,
    radial_segments: usize,
}

impl BeamShape {
    fn append(&self, soup: &mut Vec<f32>, line_origin: Vec3, axis: Vec3, start_t: f32, end_t: f32) {
        let length = (end_t - start_t) + self.embed_pad * 2.0;
        if length < self.min_run_length {
            return;
        }

        let origin = line_origin.add(axis.scale((start_t - self.embed_pad).max(0.0)));
        let beam = build_cylinder_mesh(
            origin,
            axis,
            self.radius_mm,
            self.radius_mm,
            length,
            self.radial_segments,
        );
        soup.extend_from_slice(&beam.to_triangle_soup());
    }
}

#[inline]
fn point_samples_carved_cavity(
    grid: &GridSpec,
    solid: &[bool],
    keep: &[bool],
    point: Vec3,
) -> bool {
    let x = ((point.x - grid.min.x) / grid.voxel_mm).floor() as isize;
    let y = ((point.y - grid.min.y) / grid.voxel_mm).floor() as isize;
    let z = ((point.z - grid.min.z) / grid.voxel_mm).floor() as isize;
    if !grid.in_bounds(x, y, z) {
        return false;
    }
    let i = grid.idx(x as usize, y as usize, z as usize);
    solid[i] && !keep[i]
}

#[inline]
fn projected_half_extent(extent: Vec3, axis: Vec3) -> f32 {
    extent.x * axis.x.abs() + extent.y * axis.y.abs() + extent.z * axis.z.abs()
}

/// A closed capped cylinder (an elliptic one when the radii differ) from
/// `origin` along `axis` for `length`, DragonFruit's punch and beam solid.
pub(crate) fn build_cylinder_mesh(
    origin: Vec3,
    axis: Vec3,
    radius: f32,
    radius_y: f32,
    length: f32,
    segments: usize,
) -> IndexedMesh {
    let axis = vec3_normalize(axis).unwrap_or(Vec3::new(0.0, 0.0, -1.0));

    let helper = if axis.z.abs() < 0.95 {
        Vec3::new(0.0, 0.0, 1.0)
    } else {
        Vec3::new(0.0, 1.0, 0.0)
    };
    let u = vec3_normalize(helper.cross(axis)).unwrap_or(Vec3::new(1.0, 0.0, 0.0));
    let v = vec3_normalize(axis.cross(u)).unwrap_or(Vec3::new(0.0, 1.0, 0.0));

    let segs = segments.max(8);
    let mut positions = Vec::<Vec3>::with_capacity(2 + segs * 2);
    let mut triangles = Vec::<[u32; 3]>::with_capacity(segs * 4);

    let bottom_center_index = 0u32;
    let top_center_index = 1u32;

    positions.push(origin);
    positions.push(origin.add(axis.scale(length)));

    for i in 0..segs {
        let t = i as f32 / segs as f32;
        let theta = t * std::f32::consts::TAU;
        let cs = theta.cos();
        let sn = theta.sin();
        let radial = u.scale(cs * radius).add(v.scale(sn * radius_y));

        positions.push(origin.add(radial));
        positions.push(origin.add(axis.scale(length)).add(radial));
    }

    let ring_base = 2u32;

    for i in 0..segs as u32 {
        let next = (i + 1) % segs as u32;

        let bi = ring_base + i * 2;
        let ti = bi + 1;
        let bn = ring_base + next * 2;
        let tn = bn + 1;

        triangles.push([bottom_center_index, bn, bi]);
        triangles.push([top_center_index, ti, tn]);
        triangles.push([bi, bn, tn]);
        triangles.push([bi, tn, ti]);
    }

    IndexedMesh {
        positions,
        triangles,
    }
}

fn refine_non_surface_solid_components_with_parity(
    grid: &GridSpec,
    surface: &[bool],
    solid: &mut [bool],
    mesh: &IndexedMesh,
    bvh: &Bvh,
    ctl: Control<'_>,
) -> Result<(), Cancelled> {
    let mut visited = vec![false; solid.len()];
    let mut component = Vec::<usize>::new();
    let mut queue = VecDeque::<usize>::new();

    for start in 0..solid.len() {
        if visited[start] || !solid[start] || surface[start] {
            continue;
        }
        ctl.check()?;

        visited[start] = true;
        queue.push_back(start);
        component.clear();

        while let Some(i) = queue.pop_front() {
            component.push(i);
            let (x, y, z) = grid.coords(i);

            for (dx, dy, dz) in N6 {
                let nx_i = x as isize + dx;
                let ny_i = y as isize + dy;
                let nz_i = z as isize + dz;
                if !grid.in_bounds(nx_i, ny_i, nz_i) {
                    continue;
                }

                let ni = grid.idx(nx_i as usize, ny_i as usize, nz_i as usize);
                if visited[ni] || !solid[ni] || surface[ni] {
                    continue;
                }

                visited[ni] = true;
                queue.push_back(ni);
            }
        }

        let (x, y, z) = grid.coords(component[0]);
        let sample_p = grid.center_world(x, y, z);

        if !point_inside_mesh_parity(mesh, bvh, sample_p, grid.voxel_mm) {
            for &i in &component {
                solid[i] = false;
            }
        }
    }
    Ok(())
}

fn label_void_components(grid: &GridSpec, solid: &[bool]) -> Vec<i32> {
    let mut labels = vec![-1i32; solid.len()];
    let mut queue = VecDeque::<usize>::new();
    let mut next_label = 0i32;

    for start in 0..solid.len() {
        if solid[start] || labels[start] >= 0 {
            continue;
        }

        labels[start] = next_label;
        queue.push_back(start);

        while let Some(i) = queue.pop_front() {
            let (x, y, z) = grid.coords(i);

            for (dx, dy, dz) in N6 {
                let nx_i = x as isize + dx;
                let ny_i = y as isize + dy;
                let nz_i = z as isize + dz;
                if !grid.in_bounds(nx_i, ny_i, nz_i) {
                    continue;
                }

                let ni = grid.idx(nx_i as usize, ny_i as usize, nz_i as usize);
                if solid[ni] || labels[ni] >= 0 {
                    continue;
                }

                labels[ni] = next_label;
                queue.push_back(ni);
            }
        }

        next_label += 1;
    }

    labels
}

fn preserve_source_void_separators(
    grid: &GridSpec,
    solid: &[bool],
    void_components: &[i32],
    keep: &mut [bool],
) {
    for z in 0..grid.nz {
        for y in 0..grid.ny {
            for x in 0..grid.nx {
                let i = grid.idx(x, y, z);
                if !solid[i] || keep[i] {
                    continue;
                }

                let mut first_label = -1i32;
                let mut separates_distinct_voids = false;

                for (dx, dy, dz) in N6 {
                    let nx_i = x as isize + dx;
                    let ny_i = y as isize + dy;
                    let nz_i = z as isize + dz;
                    if !grid.in_bounds(nx_i, ny_i, nz_i) {
                        continue;
                    }

                    let ni = grid.idx(nx_i as usize, ny_i as usize, nz_i as usize);
                    if solid[ni] {
                        continue;
                    }

                    let label = void_components[ni];
                    if label < 0 {
                        continue;
                    }

                    if first_label < 0 {
                        first_label = label;
                    } else if label != first_label {
                        separates_distinct_voids = true;
                        break;
                    }
                }

                if separates_distinct_voids {
                    keep[i] = true;
                }
            }
        }
    }
}

pub(crate) fn point_inside_mesh_parity(
    mesh: &IndexedMesh,
    bvh: &Bvh,
    p: Vec3,
    voxel_mm: f32,
) -> bool {
    // Skewed direction avoids axis-aligned degeneracy against many triangles.
    let ray_dir = Vec3::new(0.893, 0.372, 0.254);
    let origin = p.add(ray_dir.scale(voxel_mm * 0.173));
    (bvh.ray_hit_count(mesh, origin, ray_dir) & 1) == 1
}

pub(crate) fn hole_axis_and_length(
    direction: Option<[f32; 3]>,
    center_norm: [f32; 3],
    length_mm: Option<f32>,
    bbox: &Aabb,
    tolerance_mm: f32,
) -> (Vec3, f32) {
    if let Some(dir) = direction
        && let Some(axis) = vec3_normalize(Vec3::new(dir[0], dir[1], dir[2]))
    {
        let length = length_mm
            .unwrap_or_else(|| bbox.diag())
            .max(tolerance_mm * 2.0);
        return (axis, length);
    }

    let center = bbox_point(
        bbox,
        [
            center_norm[0].clamp(0.0, 1.0),
            center_norm[1].clamp(0.0, 1.0),
            center_norm[2].clamp(0.0, 1.0),
        ],
    );

    let distances = [
        (center.x - bbox.min.x, Vec3::new(-1.0, 0.0, 0.0)),
        (bbox.max.x - center.x, Vec3::new(1.0, 0.0, 0.0)),
        (center.y - bbox.min.y, Vec3::new(0.0, -1.0, 0.0)),
        (bbox.max.y - center.y, Vec3::new(0.0, 1.0, 0.0)),
        (center.z - bbox.min.z, Vec3::new(0.0, 0.0, -1.0)),
        (bbox.max.z - center.z, Vec3::new(0.0, 0.0, 1.0)),
    ];

    distances
        .iter()
        .copied()
        .min_by(|(da, _), (db, _)| da.total_cmp(db))
        .map(|(length, axis)| (axis, length.max(tolerance_mm * 2.0)))
        .unwrap_or((Vec3::new(0.0, 0.0, -1.0), tolerance_mm * 2.0))
}

pub(crate) fn vec3_normalize(v: Vec3) -> Option<Vec3> {
    let len = v.length();
    if len <= 1e-6 {
        None
    } else {
        Some(v.scale(1.0 / len))
    }
}

#[inline]
pub(crate) fn point_triangle_distance(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> f32 {
    // Real-Time Collision Detection (Christer Ericson), closest point on triangle.
    let ab = b.sub(a);
    let ac = c.sub(a);
    let ap = p.sub(a);

    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return ap.length();
    }

    let bp = p.sub(b);
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return bp.length();
    }

    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        let proj = a.add(ab.scale(v));
        return p.sub(proj).length();
    }

    let cp = p.sub(c);
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return cp.length();
    }

    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        let proj = a.add(ac.scale(w));
        return p.sub(proj).length();
    }

    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let edge = c.sub(b);
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        let proj = b.add(edge.scale(w));
        return p.sub(proj).length();
    }

    let n = ab.cross(ac);
    let n_len = n.length().max(1e-20);
    (p.sub(a).dot(n)).abs() / n_len
}

#[cfg(test)]
#[path = "hollowing_tests.rs"]
mod tests;
