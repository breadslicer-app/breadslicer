//! DragonFruit `src/supports/PlacementLogic/Pathfinding/SDFCache.ts`.
//!
//! SDFCache — Lazy, BVH-backed **Signed** Distance Field
//!
//! Wraps three-mesh-bvh's `closestPointToPoint` behind a spatial-hash cache.
//! Grid cells are computed on-demand and cached, so the A* pathfinder pays
//! at most one BVH query per unique cell — typically ~4000 lookups per
//! placement instead of thousands of 9-ray bundles.
//!
//! The distance is SIGNED: positive = outside the mesh, negative = inside.
//! This is critical because the unsigned BVH distance can't distinguish
//! inside from outside — a point 3mm deep inside the model reports dist=3,
//! which passes a 0.5mm clearance check. Signing the distance via the
//! nearest-triangle face normal ensures that interior points are always
//! detected as blocked.
//!
//! No precomputation required; the BVH is already built on every model mesh.
//!
//! Port notes. DragonFruit holds the `THREE.Mesh` and reads its `matrixWorld`;
//! here the caller passes the world matrix to `new` and `refresh_matrix`. The
//! cell table stores distances as `f32` (a `Float32Array` in DragonFruit) with
//! its fixed capacity and its `f64` overflow map, so cached values round exactly
//! as they do there. Queries take `&self`: the cache, the face-normal memo and
//! the counters live behind `RefCell`/`Cell`, as one placement thread owns one cache.

use ahash::AHashMap as HashMap;
use std::cell::{Cell, RefCell};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use rayon::prelude::*;

use super::column_clearance_map::{ColumnClearanceMap, ColumnVerdict};
use crate::supports::placement_logic::collision_avoidance::CollisionField;
use crate::three::math_utils::{js_max, js_min, js_round};
use crate::three::{Box3, Matrix4, Vec3};
use crate::three_mesh_bvh::MeshBvh;

/// Grid cell size in mm the pool builds with. Smaller = more precise but more lookups.
const DEFAULT_CELL_SIZE_MM: f64 = 0.5;

/// A signed distance and the nearest surface point (world space).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SdfQuery {
    pub distance: f64,
    pub nearest_point: Vec3,
}

/// `quantizeToCell` (`utils/math.ts`): `Math.round(value / cellSize)`.
#[inline]
fn quantize_to_cell(value: f64, cell_size: f64) -> f64 {
    js_round(value / cell_size)
}

/// ECMAScript `ToInt32`, which `| 0` applies.
#[inline]
fn to_int32(value: f64) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    let truncated = value.trunc();
    let two32 = 4_294_967_296.0;
    let mut m = truncated % two32;
    if m < 0.0 {
        m += two32;
    }
    if m >= 2_147_483_648.0 { m - two32 } else { m }
}

/// Cantor-style hash for three integers — faster than string keys.
/// Shift to unsigned range first (supports coords up to ±16k grid cells).
/// The key is a JavaScript number; it is compared by value, so its bits key the map.
#[inline]
fn cell_key(qx: f64, qy: f64, qz: f64) -> u64 {
    let ux = to_int32(qx + 16384.0);
    let uy = to_int32(qy + 16384.0);
    let uz = to_int32(qz + 16384.0);
    let key = (ux * 32768.0 + uy) * 32768.0 + uz;
    // `-0` and `0` are the same key, as with `===`.
    (key + 0.0).to_bits()
}

/// How far a march ever needs to know the distance.
///
/// The march takes Lipschitz steps of `distance - clearance`, so a cell farther
/// away than the step it is about to take only has to answer "at least this far",
/// and the answer can be capped here. That cap is what lets the BVH prune: an
/// *unbounded* `closestPointToPoint` has to keep every node whose box could hold
/// the nearest triangle, which on a 505k-triangle part measured ~22us per cell —
/// and the router asks for 1.6M of them per run, 36 of its 47 seconds.
///
/// Bounding it is verdict-preserving: every cell is still compared against its
/// clearance exactly, the march never steps past a point closer than `clearance`,
/// and the endpoint is always sampled. It only stops the march from taking one
/// enormous step through empty space, which costs a few more iterations and buys
/// the pruning.
pub const MARCH_DISTANCE_BOUND_MM: f64 = 8.0;
/// Slots in the cell table, as a power of two. 4M slots holds ~2.8M cells
/// before it fills; past that the cache falls back to a `Map` (correct, just slower).
const CELL_TABLE_SLOTS: usize = 1 << 22;
/// How much the reused column maps may hold, in bytes. A map is one float per XY
/// cell of the mesh, so a plate at 0.2 mm is a few megabytes; this holds several
/// of them - enough for the presets a user cycles through - at the same scale as
/// the cell table above.
const COLUMN_MAP_CACHE_BYTES: usize = 32 * 1024 * 1024;

/// The cell store: the `Float32Array` table and, once it is full, the `Map`.
#[derive(Debug, Default)]
struct Cells {
    table: HashMap<u64, f32>,
    overflow: HashMap<u64, f64>,
}

impl Cells {
    fn read(&self, key: u64) -> Option<f64> {
        self.table
            .get(&key)
            .map(|v| f64::from(*v))
            .or_else(|| self.overflow.get(&key).copied())
    }

    fn write(&mut self, key: u64, value: f64) {
        if let Some(stored) = self.table.get_mut(&key) {
            *stored = value as f32;
            return;
        }
        if self.table.len() >= CELL_TABLE_SLOTS - 1 {
            // Full: keep the answer, just on the slower path.
            self.overflow.insert(key, value);
            return;
        }
        self.table.insert(key, value as f32);
    }

    fn clear(&mut self) {
        self.table.clear();
        self.overflow.clear();
    }
}

/// What the cache did, for the run report. `cell_reads` is the router's probe
/// volume (it walks a long column per probe), `bvh_queries` is how much of
/// that was new geometry work rather than a cached answer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SdfStats {
    pub cell_reads: u64,
    pub bvh_queries: u64,
}

/// How the cells are stored, for the run report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdfStoreKind {
    Table,
    TablePlusMap,
}

#[derive(Debug)]
pub struct SdfCache {
    pub cell_size: f64,

    bvh: Arc<MeshBvh>,
    inverse_matrix: Matrix4,
    world_bounds: Box3,
    world_scale: f64,
    cells: RefCell<Cells>,
    /// Opt-in exact fast path for vertical segments; see `enable_column_map`.
    column_map: Option<ColumnClearanceMap>,
    column_map_clearance: f64,
    /// Column maps built earlier, most recently used first.
    column_map_cache: Vec<(f64, ColumnClearanceMap)>,
    column_map_cache_bytes: usize,
    face_normal_cache: RefCell<HashMap<usize, Vec3>>,
    /// Closest-point answers a prefetch computed ahead of the real queries;
    /// see [`CollisionField::prefetch`].
    prefetched: RefCell<HashMap<QueryKey, f64>>,
    /// Last seen matrixWorld — used to detect stale cache.
    last_matrix: Matrix4,
    cell_reads: Cell<u64>,
    bvh_queries: Cell<u64>,
}

impl SdfCache {
    /// `new SDFCache(mesh, { cellSize })`: `bvh` is the geometry's `boundsTree`,
    /// `matrix_world` the mesh's world matrix. DragonFruit's default cell is 0.5 mm.
    pub fn new(bvh: Arc<MeshBvh>, matrix_world: Matrix4, cell_size: Option<f64>) -> Self {
        let mut cache = SdfCache {
            cell_size: cell_size.unwrap_or(DEFAULT_CELL_SIZE_MM),
            bvh,
            inverse_matrix: Matrix4::IDENTITY,
            world_bounds: Box3::EMPTY,
            world_scale: 1.0,
            cells: RefCell::new(Cells::default()),
            column_map: None,
            column_map_clearance: 0.0,
            column_map_cache: Vec::new(),
            column_map_cache_bytes: 0,
            face_normal_cache: RefCell::new(HashMap::new()),
            prefetched: RefCell::new(HashMap::new()),
            last_matrix: matrix_world,
            cell_reads: Cell::new(0),
            bvh_queries: Cell::new(0),
        };
        cache.snapshot_matrix(matrix_world);
        cache
    }

    pub fn bvh(&self) -> &Arc<MeshBvh> {
        &self.bvh
    }

    pub fn stats(&self) -> SdfStats {
        SdfStats {
            cell_reads: self.cell_reads.get(),
            bvh_queries: self.bvh_queries.get(),
        }
    }

    /// Signed distance for a march sample, bounded unconditionally.
    ///
    /// `distance_at_within` deliberately keeps the *unbounded* query for cells
    /// inside the model's bounding box, because a point deep in the solid must
    /// report a negative distance and only an unbounded traversal finds the
    /// surface to sign it against. A march does not need that: it steps by
    /// `distance - clearance`, capped at the bound, so every sample it takes is
    /// within the bound of any surface that could matter.
    ///
    /// A cached distance **at or beyond the bound means "at least that much"**,
    /// not an exact value; `distance_at` re-queries when it needs the exact value.
    pub fn bounded_distance_at(&self, wx: f64, wy: f64, wz: f64, bound_mm: f64) -> f64 {
        march_bounded_distance_at(self, wx, wy, wz, bound_mm)
    }

    fn snapshot_matrix(&mut self, matrix_world: Matrix4) {
        self.last_matrix = matrix_world;
        self.inverse_matrix = matrix_world.invert();
        let scale = matrix_world.decompose_scale();
        self.world_scale = (scale.x + scale.y + scale.z) / 3.0;

        // geometry.boundingBox: every position, then into world space.
        let local = Box3::set_from_points(
            self.bvh
                .positions()
                .iter()
                .map(|p| Vec3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2]))),
        );
        self.world_bounds = local.apply_matrix4(&matrix_world);
    }

    /// Call once at the start of each placement query.
    /// If the mesh's world transform has changed since last call
    /// (e.g. model was moved on the build plate), all cached distances
    /// are invalidated and the matrix is updated.
    ///
    /// Returns true when drift was detected. DragonFruit also notifies its
    /// `onSDFMatrixDrift` listeners here; the pool does that for its caches
    /// (`SdfCachePool::refresh_matrix`).
    pub fn refresh_matrix(&mut self, matrix_world: Matrix4) -> bool {
        if !matrix_world.equals(&self.last_matrix) {
            self.cells.borrow_mut().clear();
            self.prefetched.borrow_mut().clear();
            self.snapshot_matrix(matrix_world);
            return true;
        }
        false
    }

    // ---- Public API ----

    /// Returns the **signed** distance from `(wx, wy, wz)` (world-space, mm)
    /// to the nearest mesh surface. Cached per grid cell.
    ///
    /// Positive = outside the mesh.
    /// Negative = inside the mesh (the point is embedded in the model).
    /// Near-zero = on or very close to the surface.
    ///
    /// The sign is determined by comparing the direction from the closest
    /// surface point to the query point against the geometric face normal
    /// of the closest triangle. If the dot product is negative, the query
    /// point is on the interior side of the surface.
    pub fn distance_at(&self, wx: f64, wy: f64, wz: f64) -> f64 {
        let cs = self.cell_size;
        let qx = quantize_to_cell(wx, cs);
        let qy = quantize_to_cell(wy, cs);
        let qz = quantize_to_cell(wz, cs);
        // A value at or beyond the march's bound may be capped rather than
        // exact (see `bounded_distance_at`), so it cannot answer an unbounded
        // question: re-query and replace it with the true value.
        if let Some(cached) = self.read_cell(qx, qy, qz)
            && cached < MARCH_DISTANCE_BOUND_MM
        {
            return cached;
        }

        let dist = self.compute_signed_distance_at_quantized_cell(qx, qy, qz, f64::INFINITY);
        self.write_cell(qx, qy, qz, dist);
        dist
    }

    fn get_or_create_quantized_distance(
        &self,
        qx: f64,
        qy: f64,
        qz: f64,
        max_distance: f64,
    ) -> f64 {
        if let Some(cached) = self.read_cell(qx, qy, qz) {
            return cached;
        }

        let cs = self.cell_size;
        let (cx, cy, cz) = (qx * cs, qy * cs, qz * cs);

        if max_distance != f64::INFINITY
            && !self.expanded_world_bounds_contains(cx, cy, cz, max_distance)
        {
            return f64::INFINITY;
        }

        let can_be_interior = self.expanded_world_bounds_contains(cx, cy, cz, 0.0);
        let dist = self.compute_signed_distance_at_quantized_cell(
            qx,
            qy,
            qz,
            if can_be_interior {
                f64::INFINITY
            } else {
                max_distance
            },
        );
        if can_be_interior || dist != f64::INFINITY {
            self.write_cell(qx, qy, qz, dist);
        }
        dist
    }

    /// The eight cell distances around a point and its fractional offsets.
    fn corners(&self, wx: f64, wy: f64, wz: f64, max_distance: f64) -> ([f64; 8], f64, f64, f64) {
        let cs = self.cell_size;
        let (fx, fy, fz) = (wx / cs, wy / cs, wz / cs);
        let (x0, y0, z0) = (fx.floor(), fy.floor(), fz.floor());
        let (tx, ty, tz) = (fx - x0, fy - y0, fz - z0);
        let d = |dx: f64, dy: f64, dz: f64| {
            self.get_or_create_quantized_distance(x0 + dx, y0 + dy, z0 + dz, max_distance)
        };
        // d000, d100, d010, d110, d001, d101, d011, d111, queried in this order.
        let corners = [
            d(0.0, 0.0, 0.0),
            d(1.0, 0.0, 0.0),
            d(0.0, 1.0, 0.0),
            d(1.0, 1.0, 0.0),
            d(0.0, 0.0, 1.0),
            d(1.0, 0.0, 1.0),
            d(0.0, 1.0, 1.0),
            d(1.0, 1.0, 1.0),
        ];
        (corners, tx, ty, tz)
    }

    pub fn distance_at_trilinear(&self, wx: f64, wy: f64, wz: f64) -> f64 {
        let ([d000, d100, d010, d110, d001, d101, d011, d111], tx, ty, tz) =
            self.corners(wx, wy, wz, f64::INFINITY);

        if [d000, d100, d010, d110, d001, d101, d011, d111].contains(&f64::INFINITY) {
            return self.distance_at(wx, wy, wz);
        }

        // Interpolate along X
        let d00 = d000 * (1.0 - tx) + d100 * tx;
        let d10 = d010 * (1.0 - tx) + d110 * tx;
        let d01 = d001 * (1.0 - tx) + d101 * tx;
        let d11 = d011 * (1.0 - tx) + d111 * tx;

        // Interpolate along Y
        let d0 = d00 * (1.0 - ty) + d10 * ty;
        let d1 = d01 * (1.0 - ty) + d11 * ty;

        // Interpolate along Z
        d0 * (1.0 - tz) + d1 * tz
    }

    /// The trilinear distance and its normalized gradient. DragonFruit's
    /// default `max_distance` is `Infinity`.
    pub fn distance_and_gradient_at(
        &self,
        wx: f64,
        wy: f64,
        wz: f64,
        max_distance: f64,
    ) -> (f64, Vec3) {
        let cs = self.cell_size;
        if max_distance != f64::INFINITY
            && !self.expanded_world_bounds_contains(wx, wy, wz, max_distance + cs * 2.0)
        {
            return (f64::INFINITY, Vec3::ZERO);
        }

        let ([d000, d100, d010, d110, d001, d101, d011, d111], tx, ty, tz) =
            self.corners(wx, wy, wz, max_distance);

        if [d000, d100, d010, d110, d001, d101, d011, d111].contains(&f64::INFINITY) {
            let distance = self.distance_at_within(wx, wy, wz, max_distance);
            let h = 0.1;
            let dx_plus = self.distance_at_within(wx + h, wy, wz, max_distance);
            let dx_minus = self.distance_at_within(wx - h, wy, wz, max_distance);
            let dy_plus = self.distance_at_within(wx, wy + h, wz, max_distance);
            let dy_minus = self.distance_at_within(wx, wy - h, wz, max_distance);
            let dz_plus = self.distance_at_within(wx, wy, wz + h, max_distance);
            let dz_minus = self.distance_at_within(wx, wy, wz - h, max_distance);

            let (mut gx, mut gy, mut gz) = (0.0, 0.0, 0.0);
            if dx_plus != f64::INFINITY && dx_minus != f64::INFINITY {
                gx = (dx_plus - dx_minus) / (2.0 * h);
            }
            if dy_plus != f64::INFINITY && dy_minus != f64::INFINITY {
                gy = (dy_plus - dy_minus) / (2.0 * h);
            }
            if dz_plus != f64::INFINITY && dz_minus != f64::INFINITY {
                gz = (dz_plus - dz_minus) / (2.0 * h);
            }

            return (distance, normalized_gradient(gx, gy, gz));
        }

        let d00 = d000 * (1.0 - tx) + d100 * tx;
        let d10 = d010 * (1.0 - tx) + d110 * tx;
        let d01 = d001 * (1.0 - tx) + d101 * tx;
        let d11 = d011 * (1.0 - tx) + d111 * tx;

        let d0 = d00 * (1.0 - ty) + d10 * ty;
        let d1 = d01 * (1.0 - ty) + d11 * ty;

        let distance = d0 * (1.0 - tz) + d1 * tz;

        let dd_dtx = (d100 - d000) * (1.0 - ty) * (1.0 - tz)
            + (d110 - d010) * ty * (1.0 - tz)
            + (d101 - d001) * (1.0 - ty) * tz
            + (d111 - d011) * ty * tz;

        let dd_dty = (d010 - d000) * (1.0 - tx) * (1.0 - tz)
            + (d110 - d100) * tx * (1.0 - tz)
            + (d011 - d001) * (1.0 - tx) * tz
            + (d111 - d101) * tx * tz;

        let dd_dtz = (d001 - d000) * (1.0 - tx) * (1.0 - ty)
            + (d101 - d100) * tx * (1.0 - ty)
            + (d011 - d010) * (1.0 - tx) * ty
            + (d111 - d110) * tx * ty;

        let inv_cs = 1.0 / cs;
        (
            distance,
            normalized_gradient(dd_dtx * inv_cs, dd_dty * inv_cs, dd_dtz * inv_cs),
        )
    }

    /// Returns the signed distance if this cell might be within `max_distance`
    /// of the mesh, otherwise returns Infinity without traversing the BVH.
    ///
    /// This is conservative: cells outside the mesh world AABB expanded by
    /// `max_distance` cannot be close enough to affect routing clearance or
    /// clearance penalties. Cells inside the actual mesh AABB still use the
    /// full signed distance path, preserving interior-blocking behavior.
    pub fn distance_at_within(&self, wx: f64, wy: f64, wz: f64, max_distance: f64) -> f64 {
        let cs = self.cell_size;
        let qx = quantize_to_cell(wx, cs);
        let qy = quantize_to_cell(wy, cs);
        let qz = quantize_to_cell(wz, cs);
        if let Some(cached) = self.read_cell(qx, qy, qz) {
            return cached;
        }

        let (cx, cy, cz) = (qx * cs, qy * cs, qz * cs);
        if !self.expanded_world_bounds_contains(cx, cy, cz, max_distance) {
            return f64::INFINITY;
        }

        let can_be_interior = self.expanded_world_bounds_contains(cx, cy, cz, 0.0);
        let dist = self.compute_signed_distance_at_quantized_cell(
            qx,
            qy,
            qz,
            if can_be_interior {
                f64::INFINITY
            } else {
                max_distance
            },
        );
        if can_be_interior || dist != f64::INFINITY {
            self.write_cell(qx, qy, qz, dist);
        }
        dist
    }

    fn expanded_world_bounds_contains(&self, x: f64, y: f64, z: f64, margin: f64) -> bool {
        let b = &self.world_bounds;
        if b.is_empty() {
            return true;
        }
        x >= b.min.x - margin
            && x <= b.max.x + margin
            && y >= b.min.y - margin
            && y <= b.max.y + margin
            && z >= b.min.z - margin
            && z <= b.max.z + margin
    }

    /// Signed distance at the EXACT world-space point — no cell quantization,
    /// no caching (callers memoize their own outcomes). One BVH query per call.
    ///
    /// Use this for near-field gates whose safety margins are smaller than the
    /// grid substitution error: `distance_at` answers with the distance at the
    /// quantized cell CENTER, displacing the query by up to cellSize·√3/2
    /// (~0.43mm at 0.5mm cells) — fatal for checks with sub-0.1mm margins like
    /// the contact-cone gate.
    pub fn exact_signed_distance_at(&self, wx: f64, wy: f64, wz: f64) -> f64 {
        let local = Vec3::new(wx, wy, wz).apply_matrix4(&self.inverse_matrix);
        self.signed_distance_at_local_point(local, f64::INFINITY)
    }

    fn compute_signed_distance_at_quantized_cell(
        &self,
        qx: f64,
        qy: f64,
        qz: f64,
        max_distance: f64,
    ) -> f64 {
        quantized_signed_distance(self, qx, qy, qz, max_distance)
    }

    /// Returns true if the cell at `(wx,wy,wz)` is closer to the mesh
    /// surface than `clearance` mm (i.e. would collide for the given radius).
    ///
    /// Uses single-cell `distance_at` (1 BVH query, cached) rather than
    /// `distance_at_trilinear` (8 BVH queries) — this is on the hot path for
    /// A* neighbor expansion and must remain cheap.
    pub fn is_blocked(&self, wx: f64, wy: f64, wz: f64, clearance: f64) -> bool {
        self.distance_at(wx, wy, wz) < clearance
    }

    /// Turn on the exact column fast path for one clearance.
    ///
    /// The router asks "is this column clear down to the root?" once per walk
    /// step and per direction, and that vertical march is the bulk of a run's
    /// distance-field reads. `ColumnClearanceMap` answers a column from one
    /// scalar per XY cell, built from the mesh's own vertices.
    ///
    /// Opt-in and clearance-specific. Idempotent. Returns false when the grid
    /// would be too large, in which case nothing changes.
    pub fn enable_column_map(&mut self, clearance_mm: f64, cell_mm: f64) -> bool {
        if self.column_map.is_some() && (clearance_mm - self.column_map_clearance).abs() < 1e-9 {
            return true;
        }

        if let Some(reused) = self.take_cached_column_map(clearance_mm) {
            self.column_map = Some(reused);
            self.column_map_clearance = clearance_mm;
            return true;
        }

        let Some(map) = ColumnClearanceMap::build(
            self.bvh.positions(),
            &self.last_matrix,
            clearance_mm,
            cell_mm,
        ) else {
            return false;
        };
        self.column_map = Some(map.clone());
        self.column_map_clearance = clearance_mm;
        self.remember_column_map(clearance_mm, map);
        true
    }

    /// Build and keep the map for a clearance without changing the one in use.
    pub fn prewarm_column_map(&mut self, clearance_mm: f64, cell_mm: f64) {
        if self.take_cached_column_map(clearance_mm).is_some() {
            return;
        }
        let Some(map) = ColumnClearanceMap::build(
            self.bvh.positions(),
            &self.last_matrix,
            clearance_mm,
            cell_mm,
        ) else {
            return;
        };
        self.remember_column_map(clearance_mm, map);
    }

    /// A map already built for this clearance, moved to the front of the list.
    fn take_cached_column_map(&mut self, clearance_mm: f64) -> Option<ColumnClearanceMap> {
        let i = self
            .column_map_cache
            .iter()
            .position(|(c, _)| (c - clearance_mm).abs() < 1e-9)?;
        let entry = self.column_map_cache.remove(i);
        let map = entry.1.clone();
        self.column_map_cache.insert(0, entry);
        Some(map)
    }

    /// Keep the map just built, dropping the least recently used until the list
    /// fits. The front is never dropped, and the front is the map in use.
    fn remember_column_map(&mut self, clearance_mm: f64, map: ColumnClearanceMap) {
        self.column_map_cache_bytes += map.stats.bytes;
        self.column_map_cache.insert(0, (clearance_mm, map));
        while self.column_map_cache.len() > 1
            && self.column_map_cache_bytes > COLUMN_MAP_CACHE_BYTES
        {
            let Some((_, evicted)) = self.column_map_cache.pop() else {
                break;
            };
            self.column_map_cache_bytes -= evicted.stats.bytes;
        }
    }

    /// Checks an entire line segment (A→B) for clearance using **adaptive
    /// sphere tracing** driven by the signed distance field.
    ///
    /// The SDF is 1-Lipschitz: for any two points P, Q, `|d(P) - d(Q)| ≤ |P-Q|`.
    /// So from a point P with cached distance `d`, we can safely advance by
    /// up to `(d - clearance)` along the ray — no point within that radius
    /// can be closer than `clearance` to the surface. In tight regions the
    /// adaptive step degrades gracefully to the cellSize floor.
    #[allow(clippy::too_many_arguments)]
    pub fn segment_blocked_xyz(
        &self,
        ax: f64,
        ay: f64,
        az: f64,
        bx: f64,
        by: f64,
        bz: f64,
        clearance: f64,
    ) -> bool {
        march_segment_blocked(self, ax, ay, az, bx, by, bz, clearance)
    }

    fn geom(&self) -> Geom<'_> {
        Geom {
            cell_size: self.cell_size,
            bvh: &self.bvh,
            inverse_matrix: &self.inverse_matrix,
            world_bounds: &self.world_bounds,
            world_scale: self.world_scale,
            column_map: self.column_map.as_ref(),
            column_map_clearance: self.column_map_clearance,
        }
    }

    /// Number of cached cells (for diagnostics).
    pub fn size(&self) -> usize {
        let cells = self.cells.borrow();
        cells.table.len() + cells.overflow.len()
    }

    /// How the cells are stored, for the run report: kind, cells in the table, slots.
    pub fn store(&self) -> (SdfStoreKind, usize, usize) {
        let cells = self.cells.borrow();
        let kind = if cells.overflow.is_empty() {
            SdfStoreKind::Table
        } else {
            SdfStoreKind::TablePlusMap
        };
        (kind, cells.table.len(), CELL_TABLE_SLOTS)
    }

    /// Drop the cache but keep the BVH reference.
    pub fn clear(&self) {
        self.cells.borrow_mut().clear();
    }
}

fn normalized_gradient(gx: f64, gy: f64, gz: f64) -> Vec3 {
    let len = (gx * gx + gy * gy + gz * gz).sqrt();
    if len > 1e-6 {
        Vec3::new(gx / len, gy / len, gz / len)
    } else {
        Vec3::ZERO
    }
}

impl CollisionField for SdfCache {
    fn cell_size(&self) -> f64 {
        self.cell_size
    }

    fn segment_blocked(&self, start: Vec3, end: Vec3, radius: f64) -> bool {
        self.segment_blocked_xyz(start.x, start.y, start.z, end.x, end.y, end.z, radius)
    }

    fn exact_signed_distance_at(&self, point: Vec3) -> f64 {
        SdfCache::exact_signed_distance_at(self, point.x, point.y, point.z)
    }

    fn prefetch(&self, jobs: usize, job: &(dyn Fn(usize, &dyn CollisionField) + Sync)) {
        let cells = self.cells.borrow();
        let base: &Cells = &cells;
        let geom = self.geom();
        // Workers take the jobs in index order, which is the caller's order of
        // preference; a split range would hand the last jobs out first.
        let next = AtomicUsize::new(0);
        let workers = rayon::current_num_threads().min(jobs);
        let memo = SharedMemo::new();
        (0..workers).into_par_iter().for_each(|_| {
            let view = SpeculativeField {
                geom,
                base,
                memo: &memo,
                overlay: RefCell::new(HashMap::new()),
            };
            loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= jobs {
                    break;
                }
                job(i, &view);
            }
        });
        drop(cells);
        let mut prefetched = self.prefetched.borrow_mut();
        for shard in memo.shards {
            prefetched.extend(shard.into_inner().unwrap_or_else(|e| e.into_inner()));
        }
    }

    fn clear_prefetch(&self) {
        self.prefetched.borrow_mut().clear();
    }
}

// ---- The march, shared by the cache and its prefetch views ----

/// What a march reads besides the cell store. Everything here is fixed while
/// a query runs, so a prefetch can hand it to other threads.
#[derive(Clone, Copy)]
struct Geom<'a> {
    cell_size: f64,
    bvh: &'a MeshBvh,
    inverse_matrix: &'a Matrix4,
    world_bounds: &'a Box3,
    world_scale: f64,
    column_map: Option<&'a ColumnClearanceMap>,
    column_map_clearance: f64,
}

/// A closest-point question, by the exact bits of the local point and the
/// world-space bound. Its signed answer is a pure function of these, the BVH
/// and the matrix, and a prefetch never outlives either of the last two.
type QueryKey = [u64; 4];

fn query_key(local: Vec3, max_distance: f64) -> QueryKey {
    [
        local.x.to_bits(),
        local.y.to_bits(),
        local.z.to_bits(),
        max_distance.to_bits(),
    ]
}

/// The cell store and the closest-point query a march runs on: the cache
/// itself, or one walk of a prefetch ([`SpeculativeField`]).
trait MarchField {
    fn geom(&self) -> Geom<'_>;
    /// Cached distance for a cell, or `None` when it has not been computed.
    fn read_cell(&self, qx: f64, qy: f64, qz: f64) -> Option<f64>;
    fn write_cell(&self, qx: f64, qy: f64, qz: f64, value: f64);
    /// Signed distance for a local-space point.
    fn signed_distance_at_local_point(&self, local: Vec3, max_distance: f64) -> f64;
}

impl MarchField for SdfCache {
    fn geom(&self) -> Geom<'_> {
        SdfCache::geom(self)
    }

    fn read_cell(&self, qx: f64, qy: f64, qz: f64) -> Option<f64> {
        self.cell_reads.set(self.cell_reads.get() + 1);
        self.cells.borrow().read(cell_key(qx, qy, qz))
    }

    fn write_cell(&self, qx: f64, qy: f64, qz: f64, value: f64) {
        self.cells.borrow_mut().write(cell_key(qx, qy, qz), value);
    }

    fn signed_distance_at_local_point(&self, local: Vec3, max_distance: f64) -> f64 {
        self.bvh_queries.set(self.bvh_queries.get() + 1);
        {
            let prefetched = self.prefetched.borrow();
            if !prefetched.is_empty()
                && let Some(&dist) = prefetched.get(&query_key(local, max_distance))
            {
                return dist;
            }
        }
        signed_distance(&self.geom(), local, max_distance, |face| {
            *self
                .face_normal_cache
                .borrow_mut()
                .entry(face)
                .or_insert_with(|| face_normal(&self.bvh, face))
        })
    }
}

/// One walk of a prefetch: reads the cache as it stood when the prefetch
/// began, keeps its own writes to itself, and records every closest-point
/// answer it computed. Its march can drift from the real one, which also sees
/// the other walks' writes; that only costs an answer it did not prefetch.
struct SpeculativeField<'a> {
    geom: Geom<'a>,
    base: &'a Cells,
    memo: &'a SharedMemo,
    overlay: RefCell<HashMap<u64, f64>>,
}

/// The answers a prefetch computed, shared by its workers so that no two
/// compute the same one; sharded so that they rarely wait on each other.
struct SharedMemo {
    shards: Vec<Mutex<HashMap<QueryKey, f64>>>,
}

const MEMO_SHARDS: usize = 64;

impl SharedMemo {
    fn new() -> Self {
        SharedMemo {
            shards: (0..MEMO_SHARDS)
                .map(|_| Mutex::new(HashMap::new()))
                .collect(),
        }
    }

    fn shard(&self, key: &QueryKey) -> &Mutex<HashMap<QueryKey, f64>> {
        let mixed = (key[0] ^ key[1].rotate_left(21) ^ key[2].rotate_left(42))
            .wrapping_mul(0x9E37_79B9_7F4A_7C15);
        &self.shards[(mixed >> 58) as usize % MEMO_SHARDS]
    }

    fn get(&self, key: &QueryKey) -> Option<f64> {
        let shard = self.shard(key).lock().unwrap_or_else(|e| e.into_inner());
        shard.get(key).copied()
    }

    fn insert(&self, key: QueryKey, value: f64) {
        self.shard(&key)
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, value);
    }
}

impl MarchField for SpeculativeField<'_> {
    fn geom(&self) -> Geom<'_> {
        self.geom
    }

    fn read_cell(&self, qx: f64, qy: f64, qz: f64) -> Option<f64> {
        let key = cell_key(qx, qy, qz);
        if let Some(&v) = self.overlay.borrow().get(&key) {
            return Some(v);
        }
        self.base.read(key)
    }

    fn write_cell(&self, qx: f64, qy: f64, qz: f64, value: f64) {
        // Rounded as the table stores it. A cell past the table's capacity
        // keeps full precision in the cache, which at worst costs a miss.
        self.overlay
            .borrow_mut()
            .insert(cell_key(qx, qy, qz), f64::from(value as f32));
    }

    fn signed_distance_at_local_point(&self, local: Vec3, max_distance: f64) -> f64 {
        let key = query_key(local, max_distance);
        if let Some(dist) = self.memo.get(&key) {
            return dist;
        }
        let dist = signed_distance(&self.geom, local, max_distance, |face| {
            face_normal(self.geom.bvh, face)
        });
        self.memo.insert(key, dist);
        dist
    }
}

impl CollisionField for SpeculativeField<'_> {
    fn cell_size(&self) -> f64 {
        self.geom.cell_size
    }

    fn segment_blocked(&self, start: Vec3, end: Vec3, radius: f64) -> bool {
        march_segment_blocked(self, start.x, start.y, start.z, end.x, end.y, end.z, radius)
    }

    fn exact_signed_distance_at(&self, point: Vec3) -> f64 {
        let local = point.apply_matrix4(self.geom.inverse_matrix);
        self.signed_distance_at_local_point(local, f64::INFINITY)
    }
}

/// The geometric face normal (cross product of triangle edges) rather than
/// vertex normals, which may be smoothed and unreliable for inside/outside
/// determination.
fn face_normal(bvh: &MeshBvh, face: usize) -> Vec3 {
    // Look up vertex indices for this triangle
    let [i0, i1, i2] = bvh.face(face);
    let (v0, v1, v2) = (bvh.vertex(i0), bvh.vertex(i1), bvh.vertex(i2));

    let (e1x, e1y, e1z) = (v1.x - v0.x, v1.y - v0.y, v1.z - v0.z);
    let (e2x, e2y, e2z) = (v2.x - v0.x, v2.y - v0.y, v2.z - v0.z);

    Vec3::new(
        e1y * e2z - e1z * e2y,
        e1z * e2x - e1x * e2z,
        e1x * e2y - e1y * e2x,
    )
}

/// Signed distance for a local-space point, signed with the face normal of the
/// nearest triangle.
///
/// Without signing, a point 3mm INSIDE the mesh reports dist=3 (the
/// unsigned distance to the nearest surface). isBlocked(x,y,z, 0.75)
/// checks dist < 0.75 → false → "not blocked" → support placed through
/// geometry. With signing, that same point reports dist=-3 → always < clearance
/// → correctly blocked.
fn signed_distance(
    geom: &Geom<'_>,
    local: Vec3,
    max_distance: f64,
    face_normal: impl FnOnce(usize) -> Vec3,
) -> f64 {
    let local_max_distance = if max_distance == f64::INFINITY {
        f64::INFINITY
    } else {
        max_distance / js_max(0.000001, geom.world_scale)
    };
    let Some(hit) = geom
        .bvh
        .closest_point_to_point(local, 0.0, local_max_distance)
    else {
        return f64::INFINITY;
    };
    let mut dist = hit.distance * geom.world_scale;
    if dist > 1e-6 {
        // Inside/outside: `local` is on the interior side of the nearest
        // triangle when the direction from the closest surface point to it
        // faces against the triangle's normal.
        let fn_ = face_normal(hit.face_index);
        let rp = hit.point;
        let dx = local.x - rp.x;
        let dy = local.y - rp.y;
        let dz = local.z - rp.z;
        if (dx * fn_.x + dy * fn_.y + dz * fn_.z) < 0.0 {
            dist = -dist;
        }
    }
    dist
}

fn quantized_signed_distance<F: MarchField + ?Sized>(
    field: &F,
    qx: f64,
    qy: f64,
    qz: f64,
    max_distance: f64,
) -> f64 {
    let geom = field.geom();
    let cs = geom.cell_size;
    // Compute via BVH (local space)
    let local = Vec3::new(qx * cs, qy * cs, qz * cs).apply_matrix4(geom.inverse_matrix);
    field.signed_distance_at_local_point(local, max_distance)
}

/// See [`SdfCache::bounded_distance_at`].
fn march_bounded_distance_at<F: MarchField + ?Sized>(
    field: &F,
    wx: f64,
    wy: f64,
    wz: f64,
    bound_mm: f64,
) -> f64 {
    let cs = field.geom().cell_size;
    let qx = quantize_to_cell(wx, cs);
    let qy = quantize_to_cell(wy, cs);
    let qz = quantize_to_cell(wz, cs);
    if let Some(cached) = field.read_cell(qx, qy, qz) {
        return cached;
    }
    let dist = quantized_signed_distance(field, qx, qy, qz, bound_mm);
    let capped = if dist == f64::INFINITY {
        bound_mm
    } else {
        js_min(dist, bound_mm)
    };
    field.write_cell(qx, qy, qz, capped);
    capped
}

/// Exact signed distance at a world-space point, bounded.
///
/// Used by the march where the cached value is too coarse to decide: the
/// cache answers for the nearest lattice point, so its value bounds the
/// sample's distance but cannot resolve it.
fn exact_bounded_distance_at<F: MarchField + ?Sized>(
    field: &F,
    wx: f64,
    wy: f64,
    wz: f64,
    bound_mm: f64,
) -> f64 {
    let local = Vec3::new(wx, wy, wz).apply_matrix4(field.geom().inverse_matrix);
    let dist = field.signed_distance_at_local_point(local, bound_mm);
    if dist == f64::INFINITY {
        bound_mm
    } else {
        js_min(dist, bound_mm)
    }
}

/// How far a sample's true distance can be below the cached value: the cache
/// answers for the nearest lattice point (`quantizeToCell` rounds), so this
/// is the sample's distance to it.
fn lattice_gap(cs: f64, px: f64, py: f64, pz: f64) -> f64 {
    let lx = js_round(px / cs) * cs - px;
    let ly = js_round(py / cs) * cs - py;
    let lz = js_round(pz / cs) * cs - pz;
    (lx * lx + ly * ly + lz * lz).sqrt()
}

/// One march sample, as the distance it can safely advance.
///
/// Returns -1 when the sample is inside `clearance`. The cached value bounds
/// the sample's true distance to `[base - gap, base + gap]`, so above the
/// band it settles `clear` without another query, below it settles `blocked`,
/// and only the band itself is asked exactly.
fn march_sample<F: MarchField + ?Sized>(
    field: &F,
    px: f64,
    py: f64,
    pz: f64,
    clearance: f64,
) -> f64 {
    let d = march_bounded_distance_at(field, px, py, pz, MARCH_DISTANCE_BOUND_MM);
    let base = if d == f64::INFINITY {
        MARCH_DISTANCE_BOUND_MM
    } else {
        d
    };
    let gap = lattice_gap(field.geom().cell_size, px, py, pz);
    if base + gap < clearance {
        return -1.0;
    }
    if base - gap >= clearance {
        return base - gap - clearance;
    }
    let exact = exact_bounded_distance_at(field, px, py, pz, clearance + 0.001);
    if exact < clearance {
        -1.0
    } else {
        exact - clearance
    }
}

#[allow(clippy::too_many_arguments)]
fn segment_intersects_expanded_world_bounds(
    b: &Box3,
    ax: f64,
    ay: f64,
    az: f64,
    bx: f64,
    by: f64,
    bz: f64,
    margin: f64,
) -> bool {
    if b.is_empty() {
        return true;
    }
    let (min_x, max_x) = (js_min(ax, bx), js_max(ax, bx));
    let (min_y, max_y) = (js_min(ay, by), js_max(ay, by));
    let (min_z, max_z) = (js_min(az, bz), js_max(az, bz));
    max_x >= b.min.x - margin
        && min_x <= b.max.x + margin
        && max_y >= b.min.y - margin
        && min_y <= b.max.y + margin
        && max_z >= b.min.z - margin
        && min_z <= b.max.z + margin
}

/// See [`SdfCache::segment_blocked_xyz`].
#[allow(clippy::too_many_arguments)]
fn march_segment_blocked<F: MarchField + ?Sized>(
    field: &F,
    ax: f64,
    ay: f64,
    az: f64,
    bx: f64,
    by: f64,
    bz: f64,
    clearance: f64,
) -> bool {
    let geom = field.geom();
    // Exact fast path, vertical segments only: it answers `blocked` only
    // when a mesh vertex is provably within `clearance` of the segment, so
    // it can only settle what the march below would have settled anyway.
    if let Some(map) = geom.column_map
        && ax == bx
        && ay == by
        && (clearance - geom.column_map_clearance).abs() < 1e-9
        && map.column_verdict(ax, ay, az, bz) == ColumnVerdict::Blocked
    {
        return true;
    }

    let dx = bx - ax;
    let dy = by - ay;
    let dz = bz - az;
    let len = (dx * dx + dy * dy + dz * dz).sqrt();
    if len < 0.01 {
        return march_sample(field, ax, ay, az, clearance) < 0.0;
    }
    if !segment_intersects_expanded_world_bounds(
        geom.world_bounds,
        ax,
        ay,
        az,
        bx,
        by,
        bz,
        clearance,
    ) {
        return false;
    }

    let inv_len = 1.0 / len;
    let ux = dx * inv_len;
    let uy = dy * inv_len;
    let uz = dz * inv_len;

    let cs = geom.cell_size;
    // Floor step: matches the fidelity of the old fixed-cellSize sampler so
    // geometry thinner than ~cellSize is still caught. Skipping below this
    // would give up accuracy; we never want that.
    let min_step = cs * 0.9;

    let mut t = 0.0;
    // Limit iterations defensively in case a pathological SDF oscillation
    // prevents progress — shouldn't happen with the minStep floor but cheap.
    let max_iter = js_max(8.0, (len / min_step).ceil() + 2.0);
    let mut iter = 0.0;
    while iter < max_iter {
        let advance = march_sample(field, ax + ux * t, ay + uy * t, az + uz * t, clearance);
        if advance < 0.0 {
            return true;
        }
        t += if advance > min_step {
            advance
        } else {
            min_step
        };
        if t >= len {
            break;
        }
        iter += 1.0;
    }
    // Always check the exact endpoint — the adaptive loop may exit with
    // t > len before sampling the terminal cell.
    march_sample(field, bx, by, bz, clearance) < 0.0
}
