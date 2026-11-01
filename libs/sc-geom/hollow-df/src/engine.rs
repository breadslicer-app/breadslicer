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
//!   the upper bound is a setting ([`GridRequest::max_resolution`], 512 by
//!   default) and so is the memory the grid may take
//!   ([`GridRequest::max_memory_bytes`]): a model too large for both gets a
//!   larger voxel, reported as such, instead of exhausting the RAM;
//! - solid areas can be given as spheres or boxes in millimetres, which survive
//!   a change of voxel size, besides DragonFruit's voxel indices;
//! - automatic drain depth measures the wall from the model's surface to
//!   the cavity wall itself, along the axis and across the hole's whole
//!   cross-section, with rays that cannot slip between two triangles, and
//!   goes just past it: 0.2 mm past where the whole hole opens, never into
//!   the cavity's far side (#315). DragonFruit takes the first two surfaces
//!   the axis crosses on whatever preview mesh is shown, which runs to the
//!   far side of the model when the axis misses the cavity or slips between
//!   two triangles of the surface, and adds 1 mm;
//! - a drain whose axis does not open into the cavity is not cut (#264): it
//!   would drain nothing, and at the automatic depth it drills through the
//!   model;
//! - with infill, drains are measured against the cavity wall alone (a beam
//!   reaching into the wall is not the cavity), and cut from the source and
//!   the wall, then from each beam on its own, since beams overlap the wall
//!   and one another (#321);
//! - a drain clears its channel through the infill: every member (pillar,
//!   lattice strut) that the drain's prism, grown by
//!   [`DRAIN_INFILL_CLEARANCE_MM`] and run on into the cavity by the hole's
//!   width, reaches into is removed whole, so the drain opens into empty
//!   space and no stub is left hanging; a member left without a hold goes
//!   too (#339). A preview given the drains shows that infill, the one the
//!   hollow gets ([`Session::preview_with_drains`]);
//! - a drain can be set to a depth (#316): one that stops short of the
//!   cavity is left out as any drain that misses it, and one that would run
//!   on into the cavity's far side stops where an automatic one would at
//!   the deepest, reported as capped;
//! - a drain can be a cone (#317): its radius at the surface and at its
//!   end, a frustum cut, measured, and cleared through the infill as the
//!   cylinder is;
//! - a drain can make a plug (#318): the piece its hole takes out of the
//!   wall, its radius smaller by half the clearance all along, as a closed
//!   mesh of its own ([`HollowApplied::plugs`]);
//! - the result reports volumes in mm³ from the meshes themselves.

use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use crate::bvh::Bvh;
use crate::control::{Cancelled, Control, Stage};
use crate::drain::{
    ConvexCutter, CutError, OverlappingShells, append, intersect_convex_closed,
    subtract_convex_closed, surfaces_cross,
};
use crate::hollowing::{
    ExactCavity, GridSpec, HollowMode, HollowOptions, HollowSession, InfillMode,
    MIN_VOXEL_RESOLUTION, OpenFace, merge_meshes, normalize_mesh_for_boolean,
    normalize_mesh_for_boolean_with_weld, point_inside_mesh_parity, vec3_normalize,
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
/// The most memory a session's grid may take by default
/// ([`GridRequest::max_memory_bytes`]): 1 GiB, about 38 M cells. A fixed
/// figure, not one read from the free RAM, so a model hollows on the same
/// grid on every machine and every run, the preview's and the hollow's. Any
/// grid of up to 192 voxels per side (DragonFruit's bound) fits in it; a
/// 512-voxel cube would take 3.8 GB.
pub const GRID_MEMORY_LIMIT_BYTES: usize = 1 << 30;
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
    /// Most memory the grid may take, at [`BYTES_PER_CELL`]: a grid larger
    /// gets a larger voxel, the finest that fits.
    pub max_memory_bytes: usize,
}

impl Default for GridRequest {
    fn default() -> Self {
        Self {
            voxel_size_mm: DEFAULT_VOXEL_SIZE_MM,
            max_resolution: RESOLUTION_LIMIT,
            max_memory_bytes: GRID_MEMORY_LIMIT_BYTES,
        }
    }
}

/// Why the effective voxel differs from the requested one beyond rounding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionBound {
    /// The request wanted more voxels than `max_resolution`: voxels are larger.
    Coarsened { wanted: u32, max: u16 },
    /// The grid would have taken more than `limit_bytes`
    /// ([`GridRequest::max_memory_bytes`]): voxels are larger, the finest
    /// that fit. `wanted` is the resolution the voxel asked for.
    MemoryLimited { wanted: u32, limit_bytes: usize },
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
    /// Just through the wall: until the whole hole opens into the cavity,
    /// plus [`DRAIN_PAST_WALL_MM`], and never into the cavity's far side
    /// (#315). DragonFruit goes 1 mm past where the axis meets the wall.
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
    /// A cone (#317): the radius at the drain's end, where the radii above
    /// are at the surface (on the axis, at `center`). `None` is a cylinder.
    /// An oval cone keeps its proportions.
    pub inner_radius_mm: Option<f32>,
    /// Make a plug for this drain (#318), narrower than the hole by this
    /// much across (half of it all round). `None` makes none.
    pub plug_clearance_mm: Option<f32>,
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
            inner_radius_mm: None,
            plug_clearance_mm: None,
        }
    }
}

/// How much narrower than its hole a plug is by default, across (#318):
/// 0.1 mm, 0.05 mm all round.
pub const DEFAULT_PLUG_CLEARANCE_MM: f32 = 0.1;

/// DragonFruit's `HOLE_PUNCH_OUTSIDE_PROTRUSION_MM`.
pub const DRAIN_OUTSIDE_PROTRUSION_MM: f32 = 3.0;
/// How far an automatic drain goes past the cavity wall (#315; DragonFruit's
/// `HOLE_PUNCH_DEPTH_OFFSET_FROM_SHELL_MM` is 1 mm, past its first hit
/// only).
pub const DRAIN_PAST_WALL_MM: f32 = 0.2;
/// How far short of the cavity's far side an automatic drain stops: it
/// never reaches into the opposite wall (#264).
const DRAIN_FAR_SIDE_CLEARANCE_MM: f32 = 0.1;
/// DragonFruit's `HOLE_PUNCH_AUTO_DEPTH_RAY_START_OFFSET_MM`.
const DRAIN_RAY_START_MM: f32 = 0.3;
/// Where across the hole its rim is measured, as a fraction of its radius:
/// just inside the prism's corners.
const DRAIN_RIM_SAMPLE: f32 = 0.99;
/// How much deeper than the axis, per millimetre of radius, a ray across
/// the hole may meet the cavity wall and still be the same wall, sloping
/// (up to about 63°): one deeper is beside the cavity, behind solid.
const DRAIN_RIM_SLOPE: f32 = 2.0;
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
    /// A set depth (#316) that would have run on into the cavity's far side
    /// was cut shorter, to `depth_mm`.
    pub depth_capped: bool,
    /// Sides of the prism the cylinder was cut as.
    pub segments: usize,
}

/// The plug of one drain (#318).
#[derive(Debug, Clone)]
pub struct DrainPlug {
    /// Which drain, by its index in the drains given.
    pub drain: usize,
    /// The piece the drain's hole takes out of the wall, narrower by the
    /// clearance, closed, in the session frame where the drain is: its
    /// outer end follows the model's surface, its inner end the cavity's.
    /// `None` when it could not be made closed.
    pub mesh: Option<IndexedMesh>,
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
    /// One per drain cut that asked for a plug, in drain order.
    pub plugs: Vec<DrainPlug>,
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
    cavity: Mutex<Option<(HollowSettings, Arc<BuiltCavity>)>>,
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

/// A cavity as the session keeps it, with what drains are measured
/// against, built on the first drain measured (a hover, an apply) and kept
/// with it: every drain on this cavity reads the same wall and BVH.
#[derive(Debug)]
struct BuiltCavity {
    exact: ExactCavity,
    /// The wall and the infill beams as one mesh; only with infill.
    merged: OnceLock<IndexedMesh>,
    /// The wall's BVH.
    bvh: OnceLock<Bvh>,
    /// The infill with the channels of the drains asked last cleared
    /// through it (#339), and those drains: a preview with drains, which
    /// the apply that follows it reuses.
    drained: Mutex<Option<(Vec<DrainHole>, Arc<IndexedMesh>)>>,
}

impl BuiltCavity {
    fn new(exact: ExactCavity) -> Self {
        Self {
            exact,
            merged: OnceLock::new(),
            bvh: OnceLock::new(),
            drained: Mutex::new(None),
        }
    }

    /// The cavity as the hollow carries it: the wall, and the infill beams.
    fn surface(&self) -> &IndexedMesh {
        match &self.exact.infill {
            Some(infill) => self
                .merged
                .get_or_init(|| merge_meshes(&self.exact.wall, infill)),
            None => &self.exact.wall,
        }
    }

    /// The cavity wall and its BVH, which drains are measured against: a
    /// drain opens where its axis meets the wall. The beams are left out:
    /// they run into the wall, and an axis that meets one there has not
    /// reached the cavity yet (#321).
    fn drain_target(&self) -> (&IndexedMesh, &Bvh) {
        let wall = &self.exact.wall;
        (wall, self.bvh.get_or_init(|| Bvh::build(wall)))
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
        let voxel_for = |resolution: u16| (max_extent / resolution as f32).max(0.05);
        let cells_at = |resolution: u16| {
            GridSpec::for_bbox_with_voxel(&bbox, voxel_for(resolution)).cell_count()
        };
        // Bound by memory before anything is allocated: the finest
        // resolution whose grid fits (cells grow with the resolution).
        let max_cells_by_memory = request.max_memory_bytes / BYTES_PER_CELL;
        let (resolution, bound) = if cells_at(resolution) > max_cells_by_memory {
            let (mut fits, mut too_big) = (MIN_VOXEL_RESOLUTION, resolution);
            while too_big - fits > 1 {
                let mid = fits + (too_big - fits) / 2;
                if cells_at(mid) > max_cells_by_memory {
                    too_big = mid;
                } else {
                    fits = mid;
                }
            }
            (
                fits,
                Some(ResolutionBound::MemoryLimited {
                    wanted,
                    limit_bytes: request.max_memory_bytes,
                }),
            )
        } else {
            (resolution, bound)
        };
        let voxel_mm = voxel_for(resolution);
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

    fn cached_cavity(&self) -> Option<(HollowSettings, Arc<BuiltCavity>)> {
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
    ) -> Result<Arc<BuiltCavity>, Cancelled> {
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
        let cavity = Arc::new(BuiltCavity::new(
            self.inner.exact_cavity(&options, &dist, ctl)?,
        ));
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

    /// What `hole` would do on the hollow `settings` make, measured exactly
    /// as [`Session::apply`] measures it before cutting: the same check on
    /// the same cavity, so a report that does not reach the cavity is a
    /// drain apply leaves out, and one that does is a drain it cuts. Cheap
    /// enough to ask per frame (a few dozen rays through kept BVHs, across
    /// the hole; the first ask on a cavity builds its BVH).
    ///
    /// `None` while this session holds no cavity for `settings` (a preview
    /// or an apply with them builds it): this never builds one.
    pub fn drain_report(&self, settings: &HollowSettings, hole: &DrainHole) -> Option<DrainReport> {
        let (built_for, cavity) = self.cached_cavity()?;
        (built_for == *settings).then(|| self.measure_drain(settings, &cavity, hole))
    }

    /// Builds what [`Session::drain_report`] reads for the cavity of
    /// `settings`, when this session holds it, so the first ask is as quick
    /// as the next ones. Returns whether that cavity is here.
    pub fn prepare_drain_reports(&self, settings: &HollowSettings) -> bool {
        match self.cached_cavity() {
            Some((built_for, cavity)) if built_for == *settings => {
                cavity.drain_target();
                true
            }
            _ => false,
        }
    }

    fn measure_drain(
        &self,
        settings: &HollowSettings,
        cavity: &BuiltCavity,
        hole: &DrainHole,
    ) -> DrainReport {
        drain_report_on(
            hole,
            settings.wall_thickness_mm,
            (self.source_mesh(), self.inner.source_bvh()),
            cavity.drain_target(),
        )
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
        self.preview_with_drains(settings, &[], ctl)
    }

    /// [`Session::preview`] with `drains` placed: the infill shows the
    /// channels they clear through it, as [`Session::apply`] with them cuts
    /// it (#339). When that cut fails, the infill shows whole (the apply
    /// reports the failure).
    pub fn preview_with_drains(
        &self,
        settings: &HollowSettings,
        drains: &[DrainHole],
        ctl: Control<'_>,
    ) -> Result<HollowPreview, HollowError> {
        let built = self.cavity(settings, ctl)?;
        let cavity = &built.exact;
        let stats = self.stats(
            settings,
            cavity.removed_voxels,
            cavity.wall.triangle_count(),
            &cavity.wall,
        );
        let mut infill = cavity.infill.clone();
        if infill.is_some() && !drains.is_empty() {
            let reports: Vec<DrainReport> = drains
                .iter()
                .map(|hole| self.measure_drain(settings, &built, hole))
                .collect();
            match self.drained_infill(&built, drains, &reports, ctl) {
                Ok(Some(drained)) => infill = Some(drained.as_ref().clone()),
                Ok(None) | Err(HollowError::Drain { .. }) => {}
                Err(other) => return Err(other),
            }
        }
        Ok(HollowPreview {
            cavity: cavity.wall.clone(),
            infill,
            removed_voxel_centers: cavity.removed_voxel_centers.clone(),
            removed_voxel_indices: cavity.removed_voxel_indices.clone(),
            blocked_voxel_centers: cavity.blocked_voxel_centers.clone(),
            blocked_voxel_indices: cavity.blocked_voxel_indices.clone(),
            stats,
        })
    }

    /// The infill of `built` with the channels of `drains` (measured as
    /// `reports`) cleared through it, kept with the cavity for the next ask
    /// with the same drains. `None` without infill or when no drain opens.
    fn drained_infill(
        &self,
        built: &BuiltCavity,
        drains: &[DrainHole],
        reports: &[DrainReport],
        ctl: Control<'_>,
    ) -> Result<Option<Arc<IndexedMesh>>, HollowError> {
        let Some(infill) = built.exact.infill.as_ref() else {
            return Ok(None);
        };
        let cutters: Vec<(usize, ConvexCutter)> = drains
            .iter()
            .zip(reports)
            .enumerate()
            .filter(|(_, (_, report))| report.reaches_cavity)
            .map(|(index, (hole, report))| (index, infill_cutter(hole, report)))
            .collect();
        if cutters.is_empty() {
            return Ok(None);
        }
        if let Some((kept_for, kept)) = built
            .drained
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            && kept_for == drains
        {
            return Ok(Some(kept.clone()));
        }
        ctl.check()?;
        let drained = Arc::new(clear_channels(infill, &cutters, built.drain_target()));
        *built.drained.lock().unwrap_or_else(PoisonError::into_inner) =
            Some((drains.to_vec(), drained.clone()));
        Ok(Some(drained))
    }

    /// The printable hollow with `drains` cut, or why it cannot be made.
    pub fn apply(
        &self,
        settings: &HollowSettings,
        drains: &[DrainHole],
        ctl: Control<'_>,
    ) -> Result<HollowApplied, HollowError> {
        let built = self.cavity(settings, ctl)?;
        let cavity = &built.exact;
        ctl.stage(Stage::Stabilize)?;
        let added = built.surface().clone();
        if !cavity.closed {
            return Err(HollowError::NotClosed(edge_manifold_defects(&added)));
        }
        let shell = self
            .shell
            .get_or_init(|| SourceShell::of(self.source_mesh()));
        if !shell.defects.is_clean() {
            return Err(HollowError::NotClosed(shell.defects));
        }
        let mut reports = Vec::with_capacity(drains.len());
        // The drains that open, as (drain index, cutter).
        let mut cutters = Vec::with_capacity(drains.len());
        if !drains.is_empty() {
            ctl.stage(Stage::Drains)?;
            // Every drain is measured before any is cut, so drains do not
            // depend on their order; as `Session::drain_report` measures them.
            for (index, hole) in drains.iter().enumerate() {
                ctl.check()?;
                let report = self.measure_drain(settings, &built, hole);
                if report.reaches_cavity {
                    cutters.push((index, drain_cutter(hole, &report)));
                }
                reports.push(report);
            }
        }
        // The cavity goes in as it is, on its own vertices: no weld across
        // it and the source, which could move or merge them. Two closed
        // meshes on separate vertices make a closed one. Beams overlap the
        // wall and one another, so when drains are cut they join after:
        // the drains are cut from the source and the wall, then from each
        // beam on its own.
        let beams = cavity.infill.as_ref().filter(|_| !cutters.is_empty());
        let mut mesh = merge_meshes(
            shell.mesh.as_ref().unwrap_or(self.source_mesh()),
            if beams.is_some() {
                &cavity.wall
            } else {
                &added
            },
        );
        // Each plug is what its drain takes out of the source and the wall
        // (the beams it clears are not part of it), from the uncut mesh.
        let mut plugs = Vec::new();
        for (index, _) in &cutters {
            let hole = &drains[*index];
            if let Some(clearance) = hole.plug_clearance_mm {
                ctl.check()?;
                let cutter = plug_cutter(hole, &reports[*index], clearance);
                plugs.push(DrainPlug {
                    drain: *index,
                    mesh: intersect_convex_closed(&mesh, &cutter)
                        .ok()
                        .filter(|plug| !plug.triangles.is_empty()),
                });
            }
        }
        for (index, cutter) in &cutters {
            ctl.check()?;
            // Closed: proven by the stabilization or by the previous cut.
            mesh = subtract_convex_closed(&mesh, cutter).map_err(|error| HollowError::Drain {
                index: *index,
                error,
            })?;
        }
        if beams.is_some()
            && let Some(drained) = self.drained_infill(&built, drains, &reports, ctl)?
        {
            append(&mut mesh, &drained);
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
            plugs,
            stats,
        })
    }
}

/// `infill` less every member (a pillar, a lattice strut: one closed shell
/// each) that a drain's channel reaches into (#339): the member goes whole,
/// not just the stretch inside the channel, which would leave the rest
/// hanging from one end (an island or an overhang to a resin printer).
/// A member left without a hold is dropped too: one that crossed a dropped
/// member and has no vertex in the wall (`wall` is the cavity surface, so a
/// vertex outside it is in the wall) nor crosses a member that holds. The
/// rest stays as it is, in place.
fn clear_channels(
    infill: &IndexedMesh,
    cutters: &[(usize, ConvexCutter)],
    wall: (&IndexedMesh, &Bvh),
) -> IndexedMesh {
    let members = OverlappingShells::new(infill);
    let only: Vec<&ConvexCutter> = cutters.iter().map(|(_, cutter)| cutter).collect();
    let mut dropped = members.reached_by(&only);
    if !dropped.contains(&true) {
        return infill.clone();
    }
    // Members beside a dropped one may have held on through it. Every
    // member reaches into the wall at both ends by construction, so this
    // finds none but for a member the generator left short.
    let gone: Vec<usize> = (0..members.len()).filter(|&s| dropped[s]).collect();
    let in_wall = |p: Vec3| !point_inside_mesh_parity(wall.0, wall.1, p, 0.01);
    let mut loose: Vec<(usize, IndexedMesh)> = (0..members.len())
        .filter(|&s| !dropped[s] && gone.iter().any(|&g| members.boxes_meet(s, g)))
        .map(|s| (s, members.shell_mesh(s)))
        .filter(|(_, mesh)| !mesh.positions.iter().any(|&p| in_wall(p)))
        .collect();
    loop {
        let unsure: Vec<usize> = loose.iter().map(|(s, _)| *s).collect();
        let before = loose.len();
        loose.retain(|(s, mesh)| {
            let holds = (0..members.len()).any(|t| {
                t != *s
                    && !dropped[t]
                    && !unsure.contains(&t)
                    && members.boxes_meet(*s, t)
                    && surfaces_cross(mesh, &members.shell_mesh(t))
            });
            !holds
        });
        if loose.len() == before {
            break;
        }
    }
    for (s, _) in loose {
        dropped[s] = true;
    }
    members.without(&dropped)
}

/// A drain's axis, normalized (straight down when it has no direction), and
/// its center.
fn drain_axis(hole: &DrainHole) -> (Vec3, Vec3) {
    let axis = vec3_normalize(Vec3::new(
        hole.direction[0],
        hole.direction[1],
        hole.direction[2],
    ))
    .unwrap_or(Vec3::new(0.0, 0.0, -1.0));
    (
        Vec3::new(hole.center[0], hole.center[1], hole.center[2]),
        axis,
    )
}

/// What a drain will do: how deep it goes and whether it opens into the
/// cavity. `source` (the model before hollowing) and `cavity` (the cavity
/// wall) say where the drain opens into the cavity, if it does.
fn drain_report_on(
    hole: &DrainHole,
    wall_thickness_mm: f32,
    source: (&IndexedMesh, &Bvh),
    cavity: (&IndexedMesh, &Bvh),
) -> DrainReport {
    let (radius_x, radius_y) = drain_radii(hole);
    let widest = radius_x.max(radius_y) * drain_taper(hole).max(1.0);
    let segments = ((std::f32::consts::TAU * widest / 0.7).ceil() as usize).clamp(16, 80);
    let opening = drain_opening(source, cavity, hole, segments);
    let mut depth_capped = false;
    let (depth_mm, auto_depth) = match hole.depth {
        DrainDepth::Mm(mm) => {
            let mm = mm.clamp(0.01, DRAIN_MAX_DEPTH_MM);
            // Never on into the far side (#316): as deep as an automatic
            // drain may go, at the most.
            match opening.map(DrainOpening::deepest_mm) {
                Some(deepest) if mm > deepest => {
                    depth_capped = true;
                    (deepest, false)
                }
                _ => (mm, false),
            }
        }
        DrainDepth::Auto => {
            let depth = match opening {
                Some(opening) => opening.auto_depth_mm(),
                // Reported only: a drain that misses is not cut.
                None => (wall_thickness_mm + DRAIN_PAST_WALL_MM).clamp(0.01, DRAIN_MAX_DEPTH_MM),
            };
            (depth, true)
        }
    };
    let reaches_cavity = opening.is_some_and(|o| o.cavity_mm < depth_mm);
    DrainReport {
        depth_mm,
        auto_depth,
        reaches_cavity,
        depth_capped,
        segments,
    }
}

/// A drain's radii at the surface.
fn drain_radii(hole: &DrainHole) -> (f32, f32) {
    (
        hole.radius_mm.max(0.02),
        hole.radius_y_mm.unwrap_or(hole.radius_mm).max(0.02),
    )
}

/// A drain's radius at its end over its radius at the surface: 1 for a
/// cylinder (#317).
fn drain_taper(hole: &DrainHole) -> f32 {
    hole.inner_radius_mm
        .map_or(1.0, |inner| inner.max(0.02) / hole.radius_mm.max(0.02))
}

/// A drain's radii `t` mm past the surface along a drain `depth_mm` deep,
/// grown by `grow` all round (shrunk when negative).
fn radii_at(hole: &DrainHole, depth_mm: f32, t: f32, grow: f32) -> [f32; 2] {
    let (radius_x, radius_y) = drain_radii(hole);
    let scale = 1.0 + (drain_taper(hole) - 1.0) * t / depth_mm.max(1e-3);
    [radius_x * scale + grow, radius_y * scale + grow].map(|r| r.max(0.01))
}

/// How far out of the surface a drain's cutter starts: `outside_mm`, less
/// for a cone widening into the model so steeply that out there it would
/// narrow to under half its opening.
fn outside_mm(hole: &DrainHole, depth_mm: f32) -> f32 {
    let outside = hole.outside_mm.max(0.0);
    let widening = drain_taper(hole) - 1.0;
    if widening > 0.0 {
        outside.min(0.5 * depth_mm.max(1e-3) / widening)
    } else {
        outside
    }
}

/// The frustum along a drain from `t0` to `t1` mm past its surface, with
/// the radii `r0` at `t0` and `r1` at `t1`. The radii keep the hole's
/// proportions (an oval grown or shrunk keeps the `x` ratio).
fn drain_frustum(
    hole: &DrainHole,
    segments: usize,
    (t0, r0): (f32, [f32; 2]),
    (t1, r1): (f32, [f32; 2]),
) -> ConvexCutter {
    let (center, axis) = drain_axis(hole);
    ConvexCutter::frustum(
        center.add(axis.scale(t0)),
        axis,
        r0,
        r1[0] / r0[0],
        t1 - t0,
        segments,
        0.0,
    )
}

/// How much wider than a drain the channel it clears through the infill is,
/// all round (#339).
pub const DRAIN_INFILL_CLEARANCE_MM: f32 = 0.5;

/// The channel a drain clears through the infill (#339): its prism (or
/// cone), wider by [`DRAIN_INFILL_CLEARANCE_MM`] all round, from the same
/// start outside through the wall and on past the drain's end into the
/// cavity by the hole's width, so the drain opens into empty space. A cone
/// narrowing inward keeps its end's width past the end. Every member of the
/// infill it reaches into goes, whole.
fn infill_cutter(hole: &DrainHole, report: &DrainReport) -> ConvexCutter {
    let depth = report.depth_mm;
    let outside = outside_mm(hole, depth);
    let end = radii_at(hole, depth, depth, 0.0);
    let past = 2.0 * end[0].max(end[1]);
    let far = depth + past;
    let grow = DRAIN_INFILL_CLEARANCE_MM;
    let far_radii = if drain_taper(hole) >= 1.0 {
        radii_at(hole, depth, far, grow)
    } else {
        radii_at(hole, depth, depth, grow)
    };
    drain_frustum(
        hole,
        report.segments,
        (-outside, radii_at(hole, depth, -outside, grow)),
        (far, far_radii),
    )
}

/// The prism (or frustum, #317) `hole` cuts, as `report` measured it: from
/// outside the surface to the drain's end.
fn drain_cutter(hole: &DrainHole, report: &DrainReport) -> ConvexCutter {
    let depth = report.depth_mm;
    let outside = outside_mm(hole, depth);
    drain_frustum(
        hole,
        report.segments,
        (-outside, radii_at(hole, depth, -outside, 0.0)),
        (depth, radii_at(hole, depth, depth, 0.0)),
    )
}

/// A drain's plug's cutter (#318): the drain's own, narrower by half of
/// `clearance` all round.
fn plug_cutter(hole: &DrainHole, report: &DrainReport, clearance: f32) -> ConvexCutter {
    let depth = report.depth_mm;
    let outside = outside_mm(hole, depth);
    let shrink = -0.5 * clearance.max(0.0);
    drain_frustum(
        hole,
        report.segments,
        (-outside, radii_at(hole, depth, -outside, shrink)),
        (depth, radii_at(hole, depth, depth, shrink)),
    )
}

/// Where a drain opens into the cavity, in mm from the drain's center
/// along its axis.
#[derive(Debug, Clone, Copy, PartialEq)]
struct DrainOpening {
    /// The cavity wall, on the axis.
    cavity_mm: f32,
    /// The far side of the cavity, on the axis.
    cavity_end_mm: f32,
    /// The cavity wall where the drain meets it last: past this the whole
    /// cross-section of the hole opens into the cavity.
    open_mm: f32,
    /// The far side of the cavity where the drain meets it first: a drain
    /// this deep would reach into the opposite wall.
    far_mm: f32,
}

impl DrainOpening {
    /// The deepest a drain may go (#316): short of the cavity's far side by
    /// [`DRAIN_FAR_SIDE_CLEARANCE_MM`], or where the cavity is thinner than
    /// that, as [`DrainOpening::auto_depth_mm`] goes; to a hundredth of a
    /// millimetre, rounded down.
    fn deepest_mm(self) -> f32 {
        let deepest = (self.far_mm - DRAIN_FAR_SIDE_CLEARANCE_MM).max(
            self.cavity_mm + DRAIN_PAST_WALL_MM.min(0.5 * (self.cavity_end_mm - self.cavity_mm)),
        );
        ((deepest * 100.0).floor() / 100.0).clamp(0.01, DRAIN_MAX_DEPTH_MM)
    }

    /// Just through the wall (#315): [`DRAIN_PAST_WALL_MM`] past where the
    /// whole cross-section opens into the cavity, and short of its far side
    /// by [`DRAIN_FAR_SIDE_CLEARANCE_MM`]. Where the cavity is thinner than
    /// that, halfway across it on the axis; to a hundredth of a millimetre,
    /// rounded up.
    fn auto_depth_mm(self) -> f32 {
        let through = (self.open_mm + DRAIN_PAST_WALL_MM)
            .min(self.far_mm - DRAIN_FAR_SIDE_CLEARANCE_MM)
            .max(
                self.cavity_mm
                    + DRAIN_PAST_WALL_MM.min(0.5 * (self.cavity_end_mm - self.cavity_mm)),
            );
        ((through * 100.0).ceil() / 100.0).clamp(0.01, DRAIN_MAX_DEPTH_MM)
    }
}

/// Where `hole` opens into the cavity, if its axis stays inside the model
/// until it meets the cavity wall: an axis that leaves the model and enters
/// it again (a gap between an arm and the body, a sealed pocket) would
/// drill through what it crosses.
///
/// The wall is measured along the axis and along rays across the hole's
/// cross-section (the prism's corners, just inside, and a ring halfway
/// out), so on a curve the depth opens the whole hole. A ray that meets the
/// cavity wall much deeper than the axis ([`DRAIN_RIM_SLOPE`]) is beside
/// the cavity, behind solid, and does not count: the hole is wider than the
/// cavity there, and going on would drill along it.
fn drain_opening(
    source: (&IndexedMesh, &Bvh),
    cavity: (&IndexedMesh, &Bvh),
    hole: &DrainHole,
    segments: usize,
) -> Option<DrainOpening> {
    let (center, axis) = drain_axis(hole);
    // From just outside the surface, as DragonFruit measures.
    let start = center.sub(axis.scale(DRAIN_RAY_START_MM));
    // Where a ray from `offset` (across the axis) enters and leaves the
    // cavity, from the start plane.
    let span = |offset: Vec3| -> Option<(f32, f32)> {
        let hits = cavity
            .1
            .ray_hit_distances_closed(cavity.0, start.add(offset), axis);
        let mut walls: Vec<f32> = Vec::with_capacity(hits.len());
        for t in hits {
            if walls
                .last()
                .is_none_or(|&last| t - last > DRAIN_SAME_SURFACE_MM)
            {
                walls.push(t);
            }
        }
        // Every ray starts outside the cavity, on the start plane outside
        // the model or in its wall: the first wall is where it enters. A ray
        // grazing a wall of the cavity can count one wall too many, so the
        // count says nothing about where it starts.
        let enter = *walls.first()?;
        Some((enter, walls.get(1).copied().unwrap_or(f32::INFINITY)))
    };
    let (to_cavity, cavity_end) = span(Vec3::ZERO)?;
    let mut crossings: Vec<f32> = Vec::new();
    for t in source.1.ray_hit_distances_closed(source.0, start, axis) {
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

    // Across the drain where it opens: at its end's width, which a cone's
    // is (#317), a few tenths of a millimetre past the wall.
    let (radius_x, radius_y) = drain_radii(hole);
    let taper = drain_taper(hole);
    let (radius_x, radius_y) = (radius_x * taper, radius_y * taper);
    let helper = if axis.z.abs() < 0.95 {
        Vec3::new(0.0, 0.0, 1.0)
    } else {
        Vec3::new(0.0, 1.0, 0.0)
    };
    let u = vec3_normalize(helper.cross(axis)).unwrap_or(Vec3::new(1.0, 0.0, 0.0));
    let v = vec3_normalize(axis.cross(u)).unwrap_or(Vec3::new(0.0, 1.0, 0.0));
    let across = |angle: f32, scale: f32| {
        u.scale(angle.cos() * radius_x * scale)
            .add(v.scale(angle.sin() * radius_y * scale))
    };
    let side = std::f32::consts::TAU / segments as f32;
    let beside = to_cavity + DRAIN_RIM_SLOPE * radius_x.max(radius_y);
    let (mut open, mut far) = (to_cavity, cavity_end);
    let rim = (0..segments).map(|i| across(side * i as f32, DRAIN_RIM_SAMPLE));
    let ring = (0..segments / 2).map(|i| across(side * (2 * i) as f32 + 0.5 * side, 0.5));
    for offset in rim.chain(ring) {
        if let Some((enter, leave)) = span(offset)
            && enter <= beside
            && enter < cavity_end
        {
            open = open.max(enter);
            far = far.min(leave);
        }
    }
    Some(DrainOpening {
        cavity_mm: to_cavity - DRAIN_RAY_START_MM,
        cavity_end_mm: cavity_end - DRAIN_RAY_START_MM,
        open_mm: open - DRAIN_RAY_START_MM,
        far_mm: far - DRAIN_RAY_START_MM,
    })
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
