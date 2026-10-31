//! SoulCrafted's interface to the hollowing engine.
//!
//! [`Session`] is built once per mesh and voxel size, in the mesh's own frame
//! (DragonFruit's model-local grid): moving or rotating the model does not
//! invalidate it, only a new mesh, a new voxel size or a scale does (the
//! caller expresses the mesh in a frame whose millimetres are physical; see
//! `support-gen-df-sc`'s hollow adapter). A session then answers two
//! questions:
//!
//! - [`Session::preview`]: the inward cavity wall (and infill) the hollow
//!   will have, for the person to see and place drains on;
//! - [`Session::apply`]: the printable mesh: the source shell plus that same
//!   cavity, then the drain holes cut exactly. Fails explicitly when the
//!   result cannot be made closed.
//!
//! Both read one cavity (#29): the session builds it once per settings and
//! keeps the last one, so the hollow carries exactly the cavity the preview
//! showed, vertex for vertex, and an apply after its preview does not
//! compute it again. Drains are measured against it too: a drain that opens
//! into the cavity on screen opens into the hollow's.
//!
//! Differences from DragonFruit's front end, all on purpose:
//!
//! - the wall is cut from each voxel's exact distance to the source's
//!   triangles, measured within the wall plus [`WALL_BAND_MARGIN_VOXELS`] of
//!   the surface; DragonFruit cuts it from a chamfer distance that runs up to
//!   12.8 % long and starts half a voxel off the surface, so its wall comes
//!   out thicker than asked on average and thinner in places;
//! - the cavity is the contour of that exact field, unsmoothed: over exact
//!   distances the contour is already even, and smoothing only thickened
//!   the wall (#266, #29). DragonFruit blurs the field and Taubin-smooths the
//!   mesh, which moves it into the wall too, retries with less smoothing when
//!   the result does not close, and without smoothing chamfers the voxels,
//!   which thins the wall; none of that runs here;
//! - DragonFruit's preview is a separate, unsmoothed surface, so its result
//!   can differ from what the preview showed; here they are one cavity, and
//!   its edges are contoured closed (no hairline cracks to repair);
//! - the voxel size it actually used is reported ([`GridInfo`]), including when
//!   the resolution bound changed it (DragonFruit clamps to 24..=192 silently);
//!   the upper bound is a setting ([`GridRequest::max_resolution`]);
//! - solid areas can be given as spheres or boxes in millimetres, which survive
//!   a change of voxel size, besides DragonFruit's voxel indices;
//! - automatic drain depth measures the wall along the drain axis from the
//!   model's surface to the cavity wall itself; DragonFruit takes the first
//!   two surfaces the axis crosses on whatever preview mesh is shown, which
//!   runs to the far side of the model when the axis misses the cavity or
//!   slips between two triangles of the surface;
//! - a drain whose axis does not open into the cavity is not cut (#264): it
//!   would drain nothing, and at the automatic depth it drills through the
//!   model;
//! - the result reports volumes in mm³ from the meshes themselves.

use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use crate::bvh::Bvh;
use crate::control::{Cancelled, Control, Stage};
use crate::drain::{ConvexCutter, CutError, subtract_convex_closed};
use crate::hollowing::{
    ExactCavity, GridSpec, HollowMode, HollowOptions, HollowSession, InfillMode,
    MIN_VOXEL_RESOLUTION, OpenFace, merge_meshes, normalize_mesh_for_boolean,
    normalize_mesh_for_boolean_with_weld, vec3_normalize,
};
use crate::mesh::{IndexedMesh, Vec3};
use crate::topology::{EdgeManifoldDefects, edge_manifold_defects, is_edge_manifold};
use crate::wall::WallDistances;

/// DragonFruit's default voxel size in its Hollow panel.
pub const DEFAULT_VOXEL_SIZE_MM: f32 = 0.65;
/// DragonFruit's default wall.
pub const DEFAULT_WALL_THICKNESS_MM: f32 = 2.0;
/// The finest grid a request may ask for along the model's largest extent.
pub const RESOLUTION_LIMIT: u16 = 512;
/// Bytes per grid cell a session holds at its peak (masks, chamfer and exact
/// distances and the copy a run reads, void labels, the cavity scalar
/// field).
pub const BYTES_PER_CELL: usize = 28;
/// How far past the wall, in voxels, distances are exact: the cavity wall is
/// interpolated along the edges of the grid's tetrahedra, the longest a
/// cube's diagonal, so a voxel it reads is at most √3 voxels past the wall.
pub const WALL_BAND_MARGIN_VOXELS: f32 = 2.0;

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
    pub solid_areas: Vec<SolidArea>,
}

impl Default for HollowSettings {
    fn default() -> Self {
        Self {
            fill: HollowFill::Cavity,
            wall_thickness_mm: DEFAULT_WALL_THICKNESS_MM,
            solid_areas: Vec::new(),
        }
    }
}

/// How deep a drain goes past the surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DrainDepth {
    /// Through the wall along the axis, plus 1 mm (DragonFruit's auto depth),
    /// the wall measured up to the cavity.
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
/// Hits along a drain axis closer than this are one surface (DragonFruit's
/// tolerance for a ray through a shared edge or vertex).
const DRAIN_SAME_SURFACE_MM: f32 = 0.05;

/// What one drain did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrainReport {
    /// The depth past the surface: the one cut, or the one a drain that does
    /// not reach the cavity would have had.
    pub depth_mm: f32,
    /// Depth came from the wall measurement.
    pub auto_depth: bool,
    /// The drain opens into the cavity, and was cut. One that does not is
    /// left uncut: it would drain nothing, and could go through the model.
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
}

/// The cavity of [`Session::preview`]: the one [`Session::apply`] builds
/// the hollow from with the same settings.
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
    /// The cavity wall (and the infill beams), facing inward: the
    /// preview's, as it is.
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
    /// The hollow mesh would not be closed (typically an open or
    /// self-intersecting source).
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
#[derive(Debug)]
pub struct Session {
    inner: HollowSession,
    grid: GridInfo,
    source_volume_mm3: f64,
    /// The exact wall distances measured so far: the widest band a preview
    /// or an apply has needed. It only grows; a narrower wall reads it
    /// clamped, so the result does not depend on what was measured before.
    wall: Mutex<Option<Arc<WallDistances>>>,
    /// The cavity built last, and the settings it was built for: a
    /// preview's, which the apply that follows it reuses.
    cavity: Mutex<Option<(HollowSettings, Arc<ExactCavity>)>>,
    /// The source as a result carries it, prepared on the first apply.
    shell: OnceLock<SourceShell>,
}

impl Clone for Session {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            grid: self.grid,
            source_volume_mm3: self.source_volume_mm3,
            wall: Mutex::new(self.measured_wall()),
            cavity: Mutex::new(self.cached_cavity()),
            shell: self.shell.clone(),
        }
    }
}

/// The source as a result carries it: welded and without degenerate
/// triangles, as the hollow always had it, and its edge defects.
#[derive(Debug, Clone)]
struct SourceShell {
    /// `None` when that is the session's source as it is (the usual case).
    mesh: Option<IndexedMesh>,
    defects: EdgeManifoldDefects,
}

impl SourceShell {
    fn of(source: &IndexedMesh) -> Self {
        let mut mesh = normalize_mesh_for_boolean(source.clone());
        let mut defects = edge_manifold_defects(&mesh);
        // The weld ladder, for hairline cracks: welding never repairs an
        // edge of more than two faces nor the winding.
        if defects.boundary > 0 && defects.overloaded == 0 && defects.same_direction == 0 {
            for weld_epsilon in [2e-6_f32, 5e-6_f32, 1e-5_f32] {
                let candidate = normalize_mesh_for_boolean_with_weld(source.clone(), weld_epsilon);
                let candidate_defects = edge_manifold_defects(&candidate);
                if candidate_defects.is_clean() {
                    (mesh, defects) = (candidate, candidate_defects);
                    break;
                }
            }
        }
        // Welding a clean, compact source changes only its vertex order:
        // keep the session's copy rather than a second one.
        let same = mesh.triangles.len() == source.triangles.len()
            && mesh.positions.len() == source.positions.len()
            && defects.is_clean()
            && is_edge_manifold(source);
        Self {
            mesh: (!same).then_some(mesh),
            defects,
        }
    }
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
            wall: Mutex::new(None),
            cavity: Mutex::new(None),
            shell: OnceLock::new(),
        })
    }

    fn cached_cavity(&self) -> Option<(HollowSettings, Arc<ExactCavity>)> {
        self.cavity
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The cavity of `settings`: the one built last when it was for these
    /// settings, else built now (and kept in its place). Reports the wall,
    /// keep-mask and surface stages either way.
    fn cavity(
        &self,
        settings: &HollowSettings,
        ctl: Control<'_>,
    ) -> Result<Arc<ExactCavity>, Cancelled> {
        if let Some((built_for, cavity)) = self.cached_cavity()
            && built_for == *settings
        {
            for stage in [Stage::WallDistance, Stage::KeepMask, Stage::CavitySurface] {
                ctl.stage(stage)?;
            }
            return Ok(cavity);
        }
        let options = self.options(settings);
        let dist = self.wall_distances(settings.wall_thickness_mm, ctl)?;
        let cavity = Arc::new(self.inner.exact_cavity(&options, &dist, ctl)?);
        *self.cavity.lock().unwrap_or_else(PoisonError::into_inner) =
            Some((settings.clone(), cavity.clone()));
        Ok(cavity)
    }

    fn measured_wall(&self) -> Option<Arc<WallDistances>> {
        self.wall
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The wall distances a wall of `wall_mm` reads, in voxels: exact within
    /// the wall plus [`WALL_BAND_MARGIN_VOXELS`] of the surface. Measured
    /// once per session and kept: a thinner wall reuses them, a thicker one
    /// measures only the voxels it adds.
    fn wall_distances(&self, wall_mm: f32, ctl: Control<'_>) -> Result<Vec<f32>, Cancelled> {
        ctl.stage(Stage::WallDistance)?;
        let band = wall_mm.max(0.2) / self.grid.voxel_mm + WALL_BAND_MARGIN_VOXELS;
        let measured = match self.measured_wall() {
            Some(known) if known.band() >= band => known,
            previous => {
                // Whole voxels, so dragging the wall measures in fewer steps.
                let measured = Arc::new(WallDistances::measure(
                    &self.inner,
                    band.ceil(),
                    previous.as_deref(),
                    ctl,
                )?);
                let mut slot = self.wall.lock().unwrap_or_else(PoisonError::into_inner);
                if slot
                    .as_ref()
                    .is_none_or(|kept| kept.band() < measured.band())
                {
                    *slot = Some(measured.clone());
                }
                measured
            }
        };
        Ok(measured.clamped(band))
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
            // The exact wall is neither smoothed nor chamfered.
            smooth_internal_surfaces: false,
            internal_chamfer_passes: 0,
            preview_voxel_spheres: false,
            preview_surface_only: false,
            rotation_quat: [0.0, 0.0, 0.0, 1.0],
            exact_wall: true,
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
        }
    }

    /// The cavity `settings` carve, as [`Session::apply`] builds it.
    pub fn preview(
        &self,
        settings: &HollowSettings,
        ctl: Control<'_>,
    ) -> Result<HollowPreview, HollowError> {
        let cavity = self.cavity(settings, ctl)?;
        let stats = self.stats(
            settings,
            cavity.removed_voxels,
            cavity.wall.triangle_count(),
            &cavity.wall,
        );
        Ok(HollowPreview {
            cavity: cavity.wall.clone(),
            infill: cavity.infill.clone(),
            removed_voxel_centers: cavity.removed_voxel_centers.clone(),
            removed_voxel_indices: cavity.removed_voxel_indices.clone(),
            blocked_voxel_centers: cavity.blocked_voxel_centers.clone(),
            blocked_voxel_indices: cavity.blocked_voxel_indices.clone(),
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
        let cavity = self.cavity(settings, ctl)?;
        ctl.stage(Stage::Stabilize)?;
        let added = match &cavity.infill {
            Some(infill) => merge_meshes(&cavity.wall, infill),
            None => cavity.wall.clone(),
        };
        if !cavity.closed {
            return Err(HollowError::NotClosed(edge_manifold_defects(&added)));
        }
        let shell = self
            .shell
            .get_or_init(|| SourceShell::of(self.source_mesh()));
        if !shell.defects.is_clean() {
            return Err(HollowError::NotClosed(shell.defects));
        }
        // The cavity goes in as it is, on its own vertices: no weld across
        // it and the source, which could move or merge them. Two closed
        // meshes on separate vertices make a closed one.
        let mut mesh = merge_meshes(shell.mesh.as_ref().unwrap_or(self.source_mesh()), &added);
        let mut reports = Vec::with_capacity(drains.len());
        if !drains.is_empty() {
            ctl.stage(Stage::Drains)?;
            let source = self.source_mesh();
            let source_bvh = self.inner.source_bvh();
            let cavity_bvh = Bvh::build(&added);
            // Every drain is measured before any is cut, so drains do not
            // depend on their order.
            let mut cutters = Vec::with_capacity(drains.len());
            for hole in drains {
                ctl.check()?;
                let (cutter, report) = drain_cutter(
                    hole,
                    settings.wall_thickness_mm,
                    (source, source_bvh),
                    (&added, &cavity_bvh),
                );
                cutters.push(report.reaches_cavity.then_some(cutter));
                reports.push(report);
            }
            for (index, cutter) in cutters.iter().enumerate() {
                let Some(cutter) = cutter else {
                    continue;
                };
                ctl.check()?;
                // Closed: proven by the stabilization or by the previous cut.
                mesh = subtract_convex_closed(&mesh, cutter)
                    .map_err(|error| HollowError::Drain { index, error })?;
            }
        }
        // Every step that produced `mesh` checked it edge-manifold (the
        // source and the cavity on their own, then each cut); the stage
        // marks the hand-over.
        ctl.stage(Stage::Validate)?;
        debug_assert!(is_edge_manifold(&mesh));
        let output_volume = mesh.signed_volume();
        let mut stats = self.stats(
            settings,
            cavity.removed_voxels,
            mesh.triangle_count(),
            &added,
        );
        stats.output_volume_mm3 = Some(output_volume);
        stats.removed_volume_mm3 = Some(self.source_volume_mm3 - output_volume);
        Ok(HollowApplied {
            mesh,
            cavity: added,
            drains: reports,
            stats,
        })
    }
}

/// The prism a drain cuts, and what it will do. `source` (the model before
/// hollowing) and `cavity` (the cavity wall) say where the axis opens into
/// the cavity, if it does.
fn drain_cutter(
    hole: &DrainHole,
    wall_thickness_mm: f32,
    source: (&IndexedMesh, &Bvh),
    cavity: (&IndexedMesh, &Bvh),
) -> (ConvexCutter, DrainReport) {
    let axis = vec3_normalize(Vec3::new(
        hole.direction[0],
        hole.direction[1],
        hole.direction[2],
    ))
    .unwrap_or(Vec3::new(0.0, 0.0, -1.0));
    let center = Vec3::new(hole.center[0], hole.center[1], hole.center[2]);
    let opening = drain_opening(source, cavity, center, axis);
    let (depth_mm, auto_depth) = match hole.depth {
        DrainDepth::Mm(mm) => (mm.clamp(0.01, DRAIN_MAX_DEPTH_MM), false),
        DrainDepth::Auto => {
            let depth = match opening {
                Some(opening) => opening.auto_depth_mm(),
                // Reported only: a drain that misses is not cut.
                None => {
                    (wall_thickness_mm + DRAIN_DEPTH_PAST_WALL_MM).clamp(1.0, DRAIN_MAX_DEPTH_MM)
                }
            };
            (depth, true)
        }
    };
    let reaches_cavity = opening.is_some_and(|o| o.cavity_mm < depth_mm);
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
            reaches_cavity,
            segments,
        },
    )
}

/// Where a drain axis opens into the cavity, in mm from the drain's center.
#[derive(Debug, Clone, Copy, PartialEq)]
struct DrainOpening {
    /// The model's surface (0 when the axis slips between its triangles:
    /// the center is on the surface).
    skin_mm: f32,
    /// The cavity wall.
    cavity_mm: f32,
}

impl DrainOpening {
    /// DragonFruit's `computeAutoHolePunchDepthMmForGeometry`: the wall
    /// along the axis, at least 1 mm, plus 1 mm, to 0.1 mm (its panel value).
    fn auto_depth_mm(self) -> f32 {
        let depth = (self.cavity_mm.max(self.skin_mm + DRAIN_MIN_INSIDE_MM)
            + DRAIN_DEPTH_PAST_WALL_MM)
            .min(DRAIN_MAX_DEPTH_MM);
        (depth * 10.0).round() / 10.0
    }
}

/// Where the axis from `center` (on the model's surface) first meets the
/// cavity wall, if it stays inside the model until then: an axis that
/// leaves the model and enters it again (a gap between an arm and the
/// body, a sealed pocket) would drill through what it crosses.
fn drain_opening(
    source: (&IndexedMesh, &Bvh),
    cavity: (&IndexedMesh, &Bvh),
    center: Vec3,
    axis: Vec3,
) -> Option<DrainOpening> {
    // From just outside the surface, as DragonFruit measures.
    let origin = center.sub(axis.scale(DRAIN_RAY_START_MM));
    let to_cavity = *cavity.1.ray_hit_distances(cavity.0, origin, axis).first()?;
    let mut crossings: Vec<f32> = Vec::new();
    for t in source.1.ray_hit_distances(source.0, origin, axis) {
        if t >= to_cavity - DRAIN_SAME_SURFACE_MM {
            break;
        }
        if crossings
            .last()
            .is_none_or(|&last| t - last > DRAIN_SAME_SURFACE_MM)
        {
            crossings.push(t);
        }
    }
    if crossings.len() > 1 {
        return None;
    }
    let skin = crossings.first().copied().unwrap_or(DRAIN_RAY_START_MM);
    Some(DrainOpening {
        skin_mm: (skin - DRAIN_RAY_START_MM).max(0.0),
        cavity_mm: to_cavity - DRAIN_RAY_START_MM,
    })
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
