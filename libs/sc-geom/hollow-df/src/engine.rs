//! SoulCrafted's interface to the hollowing engine.
//!
//! [`Session`] is built once per mesh and voxel size, in the mesh's own frame
//! (DragonFruit's model-local grid): moving or rotating the model does not
//! invalidate it, only a new mesh, a new voxel size or a scale does (the
//! caller expresses the mesh in a frame whose millimetres are physical; see
//! `support-gen-df-sc`'s hollow adapter). A session then answers two
//! questions:
//!
//! - [`Session::preview`]: the inward cavity surface of the same voxel mask
//!   the final mesh uses, without smoothing, stabilization or merging. Fast,
//!   for feedback; never printable.
//! - [`Session::apply`]: the printable mesh. The source shell plus the
//!   smoothed cavity wall (and infill), checked edge-manifold with
//!   DragonFruit's smoothing back-off ladder, then the drain holes cut
//!   exactly. Fails explicitly when the result cannot be made closed.
//!
//! Differences from DragonFruit's front end, all on purpose:
//!
//! - the voxel size it actually used is reported ([`GridInfo`]), including when
//!   the resolution bound changed it (DragonFruit clamps to 24..=192 silently);
//!   the upper bound is a setting ([`GridRequest::max_resolution`]);
//! - solid areas can be given as spheres or boxes in millimetres, which survive
//!   a change of voxel size, besides DragonFruit's voxel indices;
//! - automatic drain depth measures the wall along the drain axis on the
//!   final hollow mesh; DragonFruit measures it on whatever preview mesh is
//!   shown, which since its surface-only preview is the cavity alone;
//! - the result reports volumes in mm³ from the meshes themselves.

use crate::bvh::Bvh;
use crate::control::{Cancelled, Control, Stage};
use crate::drain::{ConvexCutter, CutError, subtract_convex_closed};
use crate::hollowing::{
    GridSpec, HollowMode, HollowOptions, HollowSession, InfillMode, MIN_VOXEL_RESOLUTION, OpenFace,
    vec3_normalize,
};
use crate::mesh::{IndexedMesh, Vec3};
use crate::topology::{EdgeManifoldDefects, edge_manifold_defects};

/// DragonFruit's default voxel size in its Hollow panel.
pub const DEFAULT_VOXEL_SIZE_MM: f32 = 0.65;
/// DragonFruit's default wall.
pub const DEFAULT_WALL_THICKNESS_MM: f32 = 2.0;
/// The finest grid a request may ask for along the model's largest extent.
pub const RESOLUTION_LIMIT: u16 = 512;
/// Bytes per grid cell a session holds at its peak (masks, distances, void
/// labels, the cavity scalar field and its scratch copy).
pub const BYTES_PER_CELL: usize = 24;

/// How fine a grid to build.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridRequest {
    /// Wanted voxel edge in millimetres.
    pub voxel_size_mm: f32,
    /// Most voxels along the largest extent (DragonFruit: 192). Capped at
    /// [`RESOLUTION_LIMIT`].
    pub max_resolution: u16,
}

impl Default for GridRequest {
    fn default() -> Self {
        Self {
            voxel_size_mm: DEFAULT_VOXEL_SIZE_MM,
            max_resolution: crate::hollowing::MAX_VOXEL_RESOLUTION,
        }
    }
}

/// Why the effective voxel differs from the requested one beyond rounding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionBound {
    /// The request wanted more voxels than `max_resolution`: voxels are larger.
    Coarsened { wanted: u32, max: u16 },
    /// The request wanted fewer than DragonFruit's 24: voxels are smaller.
    Refined { wanted: u32, min: u16 },
}

/// The grid a session actually built.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridInfo {
    pub requested_voxel_mm: f32,
    /// The voxel edge used: the largest extent divided by `resolution`.
    pub voxel_mm: f32,
    /// Voxels along the largest extent.
    pub resolution: u16,
    pub bound: Option<ResolutionBound>,
    /// Cells per axis (including a one-voxel margin).
    pub dims: [usize; 3],
    /// Corner of voxel `(0, 0, 0)` in the session frame.
    pub origin: [f32; 3],
    pub memory_estimate_bytes: usize,
}

impl GridInfo {
    fn of(
        grid: &GridSpec,
        requested_voxel_mm: f32,
        resolution: u16,
        bound: Option<ResolutionBound>,
    ) -> Self {
        Self {
            requested_voxel_mm,
            voxel_mm: grid.voxel_mm,
            resolution,
            bound,
            dims: [grid.nx, grid.ny, grid.nz],
            origin: grid.min.to_array(),
            memory_estimate_bytes: grid.cell_count() * BYTES_PER_CELL,
        }
    }
}

/// What fills the hollow.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HollowFill {
    /// One empty cavity.
    Cavity,
    /// The cavity crossed by beams: a diagonal lattice or vertical pillars
    /// (vertical in the session frame: they turn with the model).
    Infill {
        pattern: InfillMode,
        cell_mm: f32,
        beam_radius_mm: f32,
    },
}

/// Material the hollow must not remove.
#[derive(Debug, Clone, PartialEq)]
pub enum SolidArea {
    /// DragonFruit's form: linear indices of this session's grid.
    Voxels(Vec<usize>),
    /// Every voxel whose center is within `radius_mm` of `center`.
    Sphere { center: [f32; 3], radius_mm: f32 },
    /// Every voxel whose center is inside the box.
    Box { min: [f32; 3], max: [f32; 3] },
}

/// What to hollow, independent of the grid.
#[derive(Debug, Clone, PartialEq)]
pub struct HollowSettings {
    pub fill: HollowFill,
    pub wall_thickness_mm: f32,
    /// Blur and Taubin-smooth the cavity wall (DragonFruit default on).
    pub smooth_internal_surfaces: bool,
    /// Voxel chamfer passes when not smoothing (DragonFruit default 2).
    pub internal_chamfer_passes: u8,
    pub solid_areas: Vec<SolidArea>,
}

impl Default for HollowSettings {
    fn default() -> Self {
        Self {
            fill: HollowFill::Cavity,
            wall_thickness_mm: DEFAULT_WALL_THICKNESS_MM,
            smooth_internal_surfaces: true,
            internal_chamfer_passes: 2,
            solid_areas: Vec::new(),
        }
    }
}

/// How deep a drain goes past the surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DrainDepth {
    /// Through the wall along the axis, plus 1 mm (DragonFruit's auto depth).
    Auto,
    Mm(f32),
}

/// A drain hole, in the session frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrainHole {
    /// A point on the outer surface.
    pub center: [f32; 3],
    /// Into the model (the surface normal, negated).
    pub direction: [f32; 3],
    pub radius_mm: f32,
    /// Second radius for an oval hole.
    pub radius_y_mm: Option<f32>,
    pub depth: DrainDepth,
    /// How far the cutter starts outside the surface (DragonFruit: 3 mm).
    pub outside_mm: f32,
}

impl DrainHole {
    pub fn new(center: [f32; 3], direction: [f32; 3], radius_mm: f32) -> Self {
        Self {
            center,
            direction,
            radius_mm,
            radius_y_mm: None,
            depth: DrainDepth::Auto,
            outside_mm: DRAIN_OUTSIDE_PROTRUSION_MM,
        }
    }
}

/// DragonFruit's `HOLE_PUNCH_OUTSIDE_PROTRUSION_MM`.
pub const DRAIN_OUTSIDE_PROTRUSION_MM: f32 = 3.0;
/// DragonFruit's `HOLE_PUNCH_DEPTH_OFFSET_FROM_SHELL_MM`.
const DRAIN_DEPTH_PAST_WALL_MM: f32 = 1.0;
/// DragonFruit's `HOLE_PUNCH_AUTO_DEPTH_RAY_START_OFFSET_MM`.
const DRAIN_RAY_START_MM: f32 = 0.3;
/// DragonFruit's `HOLE_PUNCH_AUTO_DEPTH_MIN_INSIDE_MM`.
const DRAIN_MIN_INSIDE_MM: f32 = 1.0;
/// DragonFruit's cap on any punch depth.
const DRAIN_MAX_DEPTH_MM: f32 = 120.0;

/// What one drain did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrainReport {
    /// The depth used past the surface.
    pub depth_mm: f32,
    /// Depth came from the wall measurement.
    pub auto_depth: bool,
    /// The automatic measurement found no wall along the axis and fell back to
    /// the wall thickness plus 1 mm.
    pub depth_fallback: bool,
    /// The drain opens into the cavity.
    pub reaches_cavity: bool,
    /// Sides of the prism the cylinder was cut as.
    pub segments: usize,
}

/// Counts and volumes of a preview or an apply.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HollowStats {
    pub grid: GridInfo,
    pub wall_thickness_mm: f32,
    pub source_triangles: usize,
    pub output_triangles: usize,
    /// Voxels inside the source.
    pub occupied_voxels: usize,
    /// Voxels the hollow removes.
    pub removed_voxels: usize,
    /// `removed_voxels` times the voxel volume.
    pub voxel_removed_volume_mm3: f64,
    pub source_volume_mm3: f64,
    /// Volume enclosed by the cavity surface (infill not subtracted).
    pub cavity_volume_mm3: f64,
    /// Apply only: the volume of the printable mesh.
    pub output_volume_mm3: Option<f64>,
    /// Apply only: `source - output`, the resin saved.
    pub removed_volume_mm3: Option<f64>,
    /// Apply only: cavity rebuilds with reduced smoothing it took.
    pub smoothing_retries: usize,
}

/// The visual cavity of [`Session::preview`].
#[derive(Debug, Clone)]
pub struct HollowPreview {
    /// The cavity wall, facing inward (into the cavity).
    pub cavity: IndexedMesh,
    pub infill: Option<IndexedMesh>,
    /// Centers of removed voxels on the cavity wall, for solid-area editing.
    pub removed_voxel_centers: Vec<f32>,
    pub removed_voxel_indices: Vec<u32>,
    pub blocked_voxel_centers: Vec<f32>,
    pub blocked_voxel_indices: Vec<u32>,
    pub stats: HollowStats,
}

/// The printable result of [`Session::apply`].
#[derive(Debug, Clone)]
pub struct HollowApplied {
    /// Edge-manifold: the source shell, the cavity wall (and infill), drains cut.
    pub mesh: IndexedMesh,
    /// The cavity wall alone, facing inward.
    pub cavity: IndexedMesh,
    pub drains: Vec<DrainReport>,
    pub stats: HollowStats,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HollowError {
    Cancelled,
    /// No triangles to hollow.
    EmptyMesh,
    /// The grid would exceed the memory limit the caller set.
    GridTooLarge {
        cells: usize,
        limit_cells: usize,
    },
    /// The hollow mesh stayed open even without smoothing (typically an open
    /// or self-intersecting source).
    NotClosed(EdgeManifoldDefects),
    /// Drain `index` could not be cut.
    Drain {
        index: usize,
        error: CutError,
    },
}

impl std::fmt::Display for HollowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HollowError::Cancelled => write!(f, "cancelled"),
            HollowError::EmptyMesh => write!(f, "the mesh has no triangles"),
            HollowError::GridTooLarge { cells, limit_cells } => {
                write!(
                    f,
                    "the voxel grid would have {cells} cells (limit {limit_cells})"
                )
            }
            HollowError::NotClosed(d) => write!(f, "the hollow mesh is not closed ({d:?})"),
            HollowError::Drain { index, error } => write!(f, "drain {}: {error}", index + 1),
        }
    }
}

impl std::error::Error for HollowError {}

impl From<Cancelled> for HollowError {
    fn from(_: Cancelled) -> Self {
        HollowError::Cancelled
    }
}

/// A voxelized mesh, ready for any number of previews and applies.
#[derive(Debug, Clone)]
pub struct Session {
    inner: HollowSession,
    grid: GridInfo,
    source_volume_mm3: f64,
}

impl Session {
    /// Voxelizes `mesh` (in a frame whose millimetres are physical) at the
    /// requested voxel size. `max_cells`, when given, refuses grids larger
    /// than that before allocating them.
    pub fn build(
        mesh: IndexedMesh,
        request: &GridRequest,
        max_cells: Option<usize>,
        ctl: Control<'_>,
    ) -> Result<Self, HollowError> {
        if mesh.triangles.is_empty() || mesh.positions.is_empty() {
            return Err(HollowError::EmptyMesh);
        }
        let bbox = mesh.bbox();
        let diag = bbox.max.sub(bbox.min);
        let max_extent = diag.x.max(diag.y).max(diag.z).max(1e-3);
        let requested = request.voxel_size_mm;
        // DragonFruit's `computeVoxelResolution`, with the bound reported.
        let wanted = (max_extent / requested.max(0.05)).round().max(1.0) as u32;
        let max = request
            .max_resolution
            .clamp(MIN_VOXEL_RESOLUTION, RESOLUTION_LIMIT);
        let (resolution, bound) = if wanted > max as u32 {
            (max, Some(ResolutionBound::Coarsened { wanted, max }))
        } else if wanted < MIN_VOXEL_RESOLUTION as u32 {
            (
                MIN_VOXEL_RESOLUTION,
                Some(ResolutionBound::Refined {
                    wanted,
                    min: MIN_VOXEL_RESOLUTION,
                }),
            )
        } else {
            (wanted as u16, None)
        };
        let voxel_mm = (max_extent / resolution as f32).max(0.05);
        let grid = GridSpec::for_bbox_with_voxel(&bbox, voxel_mm);
        if let Some(limit_cells) = max_cells
            && grid.cell_count() > limit_cells
        {
            return Err(HollowError::GridTooLarge {
                cells: grid.cell_count(),
                limit_cells,
            });
        }
        let source_volume_mm3 = mesh.signed_volume();
        let inner =
            HollowSession::build(mesh, resolution, [0.0, 0.0, 0.0, 1.0], Some(voxel_mm), ctl)?;
        Ok(Self {
            grid: GridInfo::of(&inner.grid(), requested, resolution, bound),
            inner,
            source_volume_mm3,
        })
    }

    pub fn grid(&self) -> &GridInfo {
        &self.grid
    }

    /// The DragonFruit session underneath (for its voxel queries).
    pub fn inner(&self) -> &HollowSession {
        &self.inner
    }

    pub fn source_mesh(&self) -> &IndexedMesh {
        self.inner.source_mesh()
    }

    /// The solid voxels a solid area covers in this grid.
    pub fn solid_area_voxels(&self, area: &SolidArea) -> Vec<usize> {
        let grid = self.inner.grid();
        let inside: Box<dyn Fn(Vec3) -> bool> = match area {
            SolidArea::Voxels(indices) => {
                return indices
                    .iter()
                    .copied()
                    .filter(|&i| self.inner.is_solid(i))
                    .collect();
            }
            SolidArea::Sphere { center, radius_mm } => {
                let c = Vec3::new(center[0], center[1], center[2]);
                let r2 = radius_mm * radius_mm;
                Box::new(move |p: Vec3| p.sub(c).dot(p.sub(c)) <= r2)
            }
            SolidArea::Box { min, max } => {
                let (min, max) = (*min, *max);
                Box::new(move |p: Vec3| {
                    let p = p.to_array();
                    (0..3).all(|k| p[k] >= min[k] && p[k] <= max[k])
                })
            }
        };
        (0..grid.cell_count())
            .filter(|&i| {
                if !self.inner.is_solid(i) {
                    return false;
                }
                let (x, y, z) = grid.coords(i);
                inside(grid.center_world(x, y, z))
            })
            .collect()
    }

    /// DragonFruit options for `settings` on this session.
    pub fn options(&self, settings: &HollowSettings) -> HollowOptions {
        let mut blocked: Vec<usize> = settings
            .solid_areas
            .iter()
            .flat_map(|area| self.solid_area_voxels(area))
            .collect();
        blocked.sort_unstable();
        blocked.dedup();
        let (mode, infill_mode, infill_cell_mm, infill_beam_radius_mm) = match settings.fill {
            HollowFill::Cavity => (HollowMode::Cavity, InfillMode::Lattice, 4.2426, 0.35),
            HollowFill::Infill {
                pattern,
                cell_mm,
                beam_radius_mm,
            } => (HollowMode::Infill, pattern, cell_mm, beam_radius_mm),
        };
        HollowOptions {
            mode,
            voxel_resolution: self.grid.resolution,
            shell_thickness_mm: settings.wall_thickness_mm,
            blocked_voxel_indices: blocked,
            infill_mode,
            infill_cell_mm,
            infill_beam_radius_mm,
            open_face: OpenFace::ZMax,
            drain_holes: Vec::new(),
            preview_cavity_only: false,
            smooth_internal_surfaces: settings.smooth_internal_surfaces,
            internal_chamfer_passes: settings.internal_chamfer_passes,
            preview_voxel_spheres: false,
            preview_surface_only: false,
            rotation_quat: [0.0, 0.0, 0.0, 1.0],
        }
    }

    fn stats(
        &self,
        settings: &HollowSettings,
        removed_voxels: usize,
        output_triangles: usize,
        cavity: &IndexedMesh,
    ) -> HollowStats {
        let voxel = self.grid.voxel_mm as f64;
        HollowStats {
            grid: self.grid,
            wall_thickness_mm: settings.wall_thickness_mm,
            source_triangles: self.inner.source_mesh().triangle_count(),
            output_triangles,
            occupied_voxels: self.inner.occupied_voxels(),
            removed_voxels,
            voxel_removed_volume_mm3: removed_voxels as f64 * voxel * voxel * voxel,
            source_volume_mm3: self.source_volume_mm3,
            cavity_volume_mm3: -cavity.signed_volume(),
            output_volume_mm3: None,
            removed_volume_mm3: None,
            smoothing_retries: 0,
        }
    }

    /// The fast visual cavity (DragonFruit's surface-only preview).
    pub fn preview(
        &self,
        settings: &HollowSettings,
        ctl: Control<'_>,
    ) -> Result<HollowPreview, HollowError> {
        let mut options = self.options(settings);
        options.preview_cavity_only = true;
        options.preview_surface_only = true;
        let shell_voxels_f = options.shell_thickness_mm.max(0.2) / self.grid.voxel_mm;
        let outcome = self.inner.run_with(&options, shell_voxels_f, ctl)?;
        let stats = self.stats(
            settings,
            outcome.report.removed_voxels,
            outcome.mesh.triangle_count(),
            &outcome.mesh,
        );
        Ok(HollowPreview {
            cavity: outcome.mesh,
            infill: outcome.preview_infill_mesh,
            removed_voxel_centers: outcome.removed_voxel_centers,
            removed_voxel_indices: outcome.removed_voxel_indices,
            blocked_voxel_centers: outcome.blocked_voxel_centers,
            blocked_voxel_indices: outcome.blocked_voxel_indices,
            stats,
        })
    }

    /// The printable hollow with `drains` cut, or why it cannot be made.
    pub fn apply(
        &self,
        settings: &HollowSettings,
        drains: &[DrainHole],
        ctl: Control<'_>,
    ) -> Result<HollowApplied, HollowError> {
        let options = self.options(settings);
        let fin = self.inner.final_mesh(&options, ctl)?;
        if !fin.stabilized {
            return Err(HollowError::NotClosed(edge_manifold_defects(&fin.mesh)));
        }
        let mut mesh = fin.mesh;
        let mut reports = Vec::with_capacity(drains.len());
        if !drains.is_empty() {
            ctl.stage(Stage::Drains)?;
            let hollow_bvh = Bvh::build(&mesh);
            let cavity_bvh = Bvh::build(&fin.cavity);
            for (index, hole) in drains.iter().enumerate() {
                ctl.check()?;
                let (cutter, report) = drain_cutter(
                    hole,
                    settings.wall_thickness_mm,
                    (&mesh, &hollow_bvh),
                    (&fin.cavity, &cavity_bvh),
                );
                // Auto depth is measured on the hollow before any drain, so
                // drains do not depend on their order.
                // Closed: proven by the stabilization or by the previous cut.
                mesh = subtract_convex_closed(&mesh, &cutter)
                    .map_err(|error| HollowError::Drain { index, error })?;
                reports.push(report);
            }
        }
        // Every step that produced `mesh` checked it edge-manifold (the
        // stabilization, then each cut); the stage marks the hand-over.
        ctl.stage(Stage::Validate)?;
        debug_assert!(edge_manifold_defects(&mesh).is_clean());
        let output_volume = mesh.signed_volume();
        let mut stats = self.stats(
            settings,
            fin.removed_voxels,
            mesh.triangle_count(),
            &fin.cavity,
        );
        stats.output_volume_mm3 = Some(output_volume);
        stats.removed_volume_mm3 = Some(self.source_volume_mm3 - output_volume);
        stats.smoothing_retries = fin.smoothing_retries;
        Ok(HollowApplied {
            mesh,
            cavity: fin.cavity,
            drains: reports,
            stats,
        })
    }
}

/// The prism a drain cuts, and what it will do. `hollow` is measured for the
/// automatic depth, `cavity` for whether the drain opens into it.
fn drain_cutter(
    hole: &DrainHole,
    wall_thickness_mm: f32,
    hollow: (&IndexedMesh, &Bvh),
    cavity: (&IndexedMesh, &Bvh),
) -> (ConvexCutter, DrainReport) {
    let axis = vec3_normalize(Vec3::new(
        hole.direction[0],
        hole.direction[1],
        hole.direction[2],
    ))
    .unwrap_or(Vec3::new(0.0, 0.0, -1.0));
    let center = Vec3::new(hole.center[0], hole.center[1], hole.center[2]);
    let fallback_depth =
        (wall_thickness_mm + DRAIN_DEPTH_PAST_WALL_MM).clamp(1.0, DRAIN_MAX_DEPTH_MM);
    let (depth_mm, auto_depth, depth_fallback) = match hole.depth {
        DrainDepth::Mm(mm) => (mm.clamp(0.01, DRAIN_MAX_DEPTH_MM), false, false),
        DrainDepth::Auto => match auto_drain_depth_mm(hollow.0, hollow.1, center, axis) {
            Some(depth) => (depth, true, false),
            None => (fallback_depth, true, true),
        },
    };
    let reaches_cavity = cavity
        .1
        .ray_hit_distances(cavity.0, center, axis)
        .first()
        .is_some_and(|&t| t < depth_mm);
    let radius_x = hole.radius_mm.max(0.02);
    let radius_y = hole.radius_y_mm.unwrap_or(hole.radius_mm).max(0.02);
    let segments =
        ((std::f32::consts::TAU * radius_x.max(radius_y) / 0.7).ceil() as usize).clamp(16, 80);
    let origin = center.sub(axis.scale(hole.outside_mm.max(0.0)));
    let length = depth_mm + hole.outside_mm.max(0.0);
    (
        ConvexCutter::prism(origin, axis, radius_x, radius_y, length, segments, 0.0),
        DrainReport {
            depth_mm,
            auto_depth,
            depth_fallback,
            reaches_cavity,
            segments,
        },
    )
}

/// DragonFruit's `computeAutoHolePunchDepthMmForGeometry`: along the axis
/// from just outside the surface, the first two distinct hits are the outer
/// skin and the cavity wall; the depth is the wall between them plus 1 mm.
pub fn auto_drain_depth_mm(mesh: &IndexedMesh, bvh: &Bvh, center: Vec3, axis: Vec3) -> Option<f32> {
    let origin = center.sub(axis.scale(DRAIN_RAY_START_MM));
    let mut distinct: Vec<f32> = Vec::new();
    for t in bvh.ray_hit_distances(mesh, origin, axis) {
        if t > 240.0 {
            break;
        }
        if distinct
            .last()
            .is_some_and(|&last| (t - last).abs() <= 0.05)
        {
            continue;
        }
        distinct.push(t);
        if distinct.len() >= 2 {
            break;
        }
    }
    if distinct.len() < 2 {
        return None;
    }
    let wall = (distinct[1] - distinct[0]).max(DRAIN_MIN_INSIDE_MM);
    let depth = (wall + DRAIN_DEPTH_PAST_WALL_MM).min(DRAIN_MAX_DEPTH_MM);
    // DragonFruit rounds the panel value to 0.1 mm.
    Some((depth * 10.0).round() / 10.0)
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
