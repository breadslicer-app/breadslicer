//! The model's signed distance field, worked out on demand from its BVH and
//! kept per grid cell, and the collision march that runs on it.
//!
//! Positive outside the model, negative inside: the sign comes from the
//! normal of the nearest triangle, so a point deep in the solid is blocked
//! whatever its distance to the surface.
//!
//! Cells keep their distance as `f32` (the first few million cells; past
//! that, at full precision), so a cached answer is the rounded one.
//!
//! A segment is checked by sphere tracing: the field is 1-Lipschitz, so from
//! a sample at distance `d` the march can step `d - clearance` without
//! passing a point closer than the clearance. Distances there only need to be
//! known up to [`MARCH_DISTANCE_BOUND_MM`], which lets the BVH prune.

use ahash::AHashMap as HashMap;
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use rayon::prelude::*;

use super::column_clearance_map::{ColumnClearanceMap, ColumnVerdict};
use bread_support_geom::bvh::MeshBvh;
use bread_support_geom::geom::math_utils::{js_max, js_min, js_round};
use bread_support_geom::geom::{Box3, Matrix4, Vec3};
use bread_support_model::collision_avoidance::CollisionField;

/// The cell edge used when none is given, mm.
const DEFAULT_CELL_MM: f64 = 0.5;
/// How far a march ever needs to know the distance, mm: a cached distance at
/// or past this means "at least this far".
pub const MARCH_DISTANCE_BOUND_MM: f64 = 8.0;
/// Cells kept as `f32`; the rest at full precision.
const F32_CELLS: usize = (1 << 22) - 1;
/// How much the kept column maps may weigh, bytes.
const COLUMN_MAPS_BYTES: usize = 32 * 1024 * 1024;
/// A prefetch runs only with at least this many threads in the pool: it
/// guesses ahead and costs more than it saves on fewer.
const PREFETCH_THREADS: usize = 4;
/// Shards of a prefetch's shared answers.
const MEMO_SHARDS: usize = 64;

/// A cell by its indices on the grid.
type CellKey = (i64, i64, i64);

/// The cell holding a world point.
fn cell_of(x: f64, y: f64, z: f64, cell: f64) -> (f64, f64, f64) {
    (js_round(x / cell), js_round(y / cell), js_round(z / cell))
}

fn key_of((qx, qy, qz): (f64, f64, f64)) -> CellKey {
    (qx as i64, qy as i64, qz as i64)
}

/// The cached distances.
#[derive(Debug, Default)]
struct Cells {
    rounded: HashMap<CellKey, f32>,
    full: HashMap<CellKey, f64>,
}

impl Cells {
    fn get(&self, key: CellKey) -> Option<f64> {
        self.rounded
            .get(&key)
            .map(|&v| f64::from(v))
            .or_else(|| self.full.get(&key).copied())
    }

    fn put(&mut self, key: CellKey, value: f64) {
        if let Some(v) = self.rounded.get_mut(&key) {
            *v = value as f32;
        } else if self.rounded.len() >= F32_CELLS {
            self.full.insert(key, value);
        } else {
            self.rounded.insert(key, value as f32);
        }
    }

    fn clear(&mut self) {
        self.rounded.clear();
        self.full.clear();
    }

    fn len(&self) -> usize {
        self.rounded.len() + self.full.len()
    }
}

/// How busy a field was: cells read, and closest-point queries made.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SdfStats {
    pub cell_reads: u64,
    pub bvh_queries: u64,
}

/// A closest-point question by the exact bits of the local point and the
/// bound: its answer depends on nothing else while the matrix stands.
type QueryKey = [u64; 4];

fn query_key(local: Vec3, bound: f64) -> QueryKey {
    [
        local.x.to_bits(),
        local.y.to_bits(),
        local.z.to_bits(),
        bound.to_bits(),
    ]
}

/// See the module notes.
#[derive(Debug)]
pub struct SdfCache {
    pub cell_size: f64,
    bvh: Arc<MeshBvh>,
    matrix: Matrix4,
    inverse: Matrix4,
    bounds: Box3,
    scale: f64,
    cells: RefCell<Cells>,
    /// The column map in use, for its clearance.
    columns: Option<(f64, ColumnClearanceMap)>,
    /// Column maps built so far, latest used first, and their weight.
    kept_columns: Vec<(f64, ColumnClearanceMap)>,
    kept_columns_bytes: usize,
    normals: RefCell<HashMap<usize, Vec3>>,
    /// Answers a prefetch worked out ahead of the queries.
    prefetched: RefCell<HashMap<QueryKey, f64>>,
    cell_reads: Cell<u64>,
    bvh_queries: Cell<u64>,
}

impl SdfCache {
    /// The field of the model `bvh` placed by `matrix_world`, with cells of
    /// `cell_size` (0.5 mm when unset).
    pub fn new(bvh: Arc<MeshBvh>, matrix_world: Matrix4, cell_size: Option<f64>) -> Self {
        let mut field = SdfCache {
            cell_size: cell_size.unwrap_or(DEFAULT_CELL_MM),
            bvh,
            matrix: matrix_world,
            inverse: Matrix4::IDENTITY,
            bounds: Box3::EMPTY,
            scale: 1.0,
            cells: RefCell::default(),
            columns: None,
            kept_columns: Vec::new(),
            kept_columns_bytes: 0,
            normals: RefCell::default(),
            prefetched: RefCell::default(),
            cell_reads: Cell::new(0),
            bvh_queries: Cell::new(0),
        };
        field.place(matrix_world);
        field
    }

    fn place(&mut self, matrix_world: Matrix4) {
        self.matrix = matrix_world;
        self.inverse = matrix_world.invert();
        let s = matrix_world.decompose_scale();
        self.scale = (s.x + s.y + s.z) / 3.0;
        self.bounds = Box3::set_from_points(
            self.bvh
                .positions()
                .iter()
                .map(|p| Vec3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2]))),
        )
        .apply_matrix4(&matrix_world);
    }

    /// Moves the field to `matrix_world`, forgetting every distance, when the
    /// model moved; whether it did.
    pub fn refresh_matrix(&mut self, matrix_world: Matrix4) -> bool {
        if matrix_world.equals(&self.matrix) {
            return false;
        }
        self.cells.borrow_mut().clear();
        self.prefetched.borrow_mut().clear();
        self.place(matrix_world);
        true
    }

    /// How busy the field was so far.
    pub fn stats(&self) -> SdfStats {
        SdfStats {
            cell_reads: self.cell_reads.get(),
            bvh_queries: self.bvh_queries.get(),
        }
    }

    /// How many cells are cached.
    pub fn size(&self) -> usize {
        self.cells.borrow().len()
    }

    /// Forgets every cached distance.
    pub fn clear(&self) {
        self.cells.borrow_mut().clear();
    }

    /// The signed distance at the centre of the cell holding the point, mm,
    /// cached. A cached value at or past the march's bound may be only a
    /// lower limit, so it is worked out again exactly.
    pub fn distance_at(&self, wx: f64, wy: f64, wz: f64) -> f64 {
        let cell = cell_of(wx, wy, wz, self.cell_size);
        if let Some(d) = self.read(cell)
            && d < MARCH_DISTANCE_BOUND_MM
        {
            return d;
        }
        let d = cell_distance(self, cell, f64::INFINITY);
        self.write(cell, d);
        d
    }

    /// The signed distance at the point itself, mm: no cell, no cache.
    pub fn exact_signed_distance_at(&self, wx: f64, wy: f64, wz: f64) -> f64 {
        let local = Vec3::new(wx, wy, wz).apply_matrix4(&self.inverse);
        self.closest(local, f64::INFINITY)
    }

    /// Whether a capsule of `clearance` round the segment reaches the model.
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
        segment_blocked(
            self,
            Vec3::new(ax, ay, az),
            Vec3::new(bx, by, bz),
            clearance,
        )
    }

    /// Settles blocked vertical columns of `clearance` from a column map
    /// (cells of `cell_mm`), built once per clearance and kept; false when
    /// the map would be too big.
    pub fn enable_column_map(&mut self, clearance_mm: f64, cell_mm: f64) -> bool {
        if self
            .columns
            .as_ref()
            .is_some_and(|(c, _)| (clearance_mm - c).abs() < 1e-9)
        {
            return true;
        }
        let map = match self.kept_column_map(clearance_mm) {
            Some(map) => map,
            None => {
                let Some(map) = ColumnClearanceMap::build(
                    self.bvh.positions(),
                    &self.matrix,
                    clearance_mm,
                    cell_mm,
                ) else {
                    return false;
                };
                self.keep_column_map(clearance_mm, map.clone());
                map
            }
        };
        self.columns = Some((clearance_mm, map));
        true
    }

    /// The kept map of `clearance_mm`, moved to the front.
    fn kept_column_map(&mut self, clearance_mm: f64) -> Option<ColumnClearanceMap> {
        let i = self
            .kept_columns
            .iter()
            .position(|(c, _)| (c - clearance_mm).abs() < 1e-9)?;
        let entry = self.kept_columns.remove(i);
        let map = entry.1.clone();
        self.kept_columns.insert(0, entry);
        Some(map)
    }

    /// Keeps `map` at the front, dropping the least used past the weight
    /// limit (never the front one).
    fn keep_column_map(&mut self, clearance_mm: f64, map: ColumnClearanceMap) {
        self.kept_columns_bytes += map.stats.bytes;
        self.kept_columns.insert(0, (clearance_mm, map));
        while self.kept_columns.len() > 1 && self.kept_columns_bytes > COLUMN_MAPS_BYTES {
            if let Some((_, gone)) = self.kept_columns.pop() {
                self.kept_columns_bytes -= gone.stats.bytes;
            }
        }
    }

    fn snapshot(&self) -> Snapshot<'_> {
        Snapshot {
            cell: self.cell_size,
            bvh: &self.bvh,
            inverse: &self.inverse,
            bounds: &self.bounds,
            scale: self.scale,
            columns: self.columns.as_ref().map(|(c, m)| (*c, m)),
        }
    }
}

impl CollisionField for SdfCache {
    fn cell_size(&self) -> f64 {
        self.cell_size
    }

    fn segment_blocked(&self, start: Vec3, end: Vec3, radius: f64) -> bool {
        segment_blocked(self, start, end, radius)
    }

    fn exact_signed_distance_at(&self, point: Vec3) -> f64 {
        SdfCache::exact_signed_distance_at(self, point.x, point.y, point.z)
    }

    fn prefetch(&self, jobs: usize, job: &(dyn Fn(usize, &dyn CollisionField) + Sync)) {
        let threads = rayon::current_num_threads();
        if threads < PREFETCH_THREADS {
            return;
        }
        let cells = self.cells.borrow();
        let base: &Cells = &cells;
        let snapshot = self.snapshot();
        let memo = Memo::new();
        // Jobs are taken in order, the caller's order of preference.
        let next = AtomicUsize::new(0);
        (0..threads.min(jobs)).into_par_iter().for_each(|_| {
            let view = Guess {
                snapshot,
                base,
                memo: &memo,
                own: RefCell::default(),
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

/// What the march reads besides the cells; fixed while a query runs.
#[derive(Clone, Copy)]
struct Snapshot<'a> {
    cell: f64,
    bvh: &'a MeshBvh,
    inverse: &'a Matrix4,
    bounds: &'a Box3,
    scale: f64,
    columns: Option<(f64, &'a ColumnClearanceMap)>,
}

/// The cells and the closest-point query the march runs on: the field, or
/// one worker of a prefetch.
trait Store {
    fn snapshot(&self) -> Snapshot<'_>;
    fn read(&self, cell: (f64, f64, f64)) -> Option<f64>;
    fn write(&self, cell: (f64, f64, f64), value: f64);
    /// The signed distance at a local point, worked out to `bound`.
    fn closest(&self, local: Vec3, bound: f64) -> f64;
}

impl Store for SdfCache {
    fn snapshot(&self) -> Snapshot<'_> {
        SdfCache::snapshot(self)
    }

    fn read(&self, cell: (f64, f64, f64)) -> Option<f64> {
        self.cell_reads.set(self.cell_reads.get() + 1);
        self.cells.borrow().get(key_of(cell))
    }

    fn write(&self, cell: (f64, f64, f64), value: f64) {
        self.cells.borrow_mut().put(key_of(cell), value);
    }

    fn closest(&self, local: Vec3, bound: f64) -> f64 {
        self.bvh_queries.set(self.bvh_queries.get() + 1);
        {
            let kept = self.prefetched.borrow();
            if !kept.is_empty()
                && let Some(&d) = kept.get(&query_key(local, bound))
            {
                return d;
            }
        }
        signed_distance(&SdfCache::snapshot(self), local, bound, |face| {
            *self
                .normals
                .borrow_mut()
                .entry(face)
                .or_insert_with(|| face_normal(&self.bvh, face))
        })
    }
}

/// One worker of a prefetch: reads the cells as they stood when it began,
/// keeps its own writes, and shares every answer it works out. Its march may
/// drift from the real one, which only costs an answer it did not guess.
struct Guess<'a> {
    snapshot: Snapshot<'a>,
    base: &'a Cells,
    memo: &'a Memo,
    own: RefCell<HashMap<CellKey, f64>>,
}

/// A prefetch's answers, shared by its workers, in shards.
struct Memo {
    shards: Vec<Mutex<HashMap<QueryKey, f64>>>,
}

impl Memo {
    fn new() -> Self {
        Memo {
            shards: (0..MEMO_SHARDS).map(|_| Mutex::default()).collect(),
        }
    }

    fn shard(&self, key: &QueryKey) -> std::sync::MutexGuard<'_, HashMap<QueryKey, f64>> {
        let mix = (key[0] ^ key[1].rotate_left(21) ^ key[2].rotate_left(42))
            .wrapping_mul(0x9E37_79B9_7F4A_7C15);
        self.shards[(mix >> 58) as usize % MEMO_SHARDS]
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }
}

impl Store for Guess<'_> {
    fn snapshot(&self) -> Snapshot<'_> {
        self.snapshot
    }

    fn read(&self, cell: (f64, f64, f64)) -> Option<f64> {
        let key = key_of(cell);
        if let Some(&v) = self.own.borrow().get(&key) {
            return Some(v);
        }
        self.base.get(key)
    }

    fn write(&self, cell: (f64, f64, f64), value: f64) {
        // Rounded as the field keeps it.
        self.own
            .borrow_mut()
            .insert(key_of(cell), f64::from(value as f32));
    }

    fn closest(&self, local: Vec3, bound: f64) -> f64 {
        let key = query_key(local, bound);
        if let Some(&d) = self.memo.shard(&key).get(&key) {
            return d;
        }
        let d = signed_distance(&self.snapshot, local, bound, |face| {
            face_normal(self.snapshot.bvh, face)
        });
        self.memo.shard(&key).insert(key, d);
        d
    }
}

impl CollisionField for Guess<'_> {
    fn cell_size(&self) -> f64 {
        self.snapshot.cell
    }

    fn segment_blocked(&self, start: Vec3, end: Vec3, radius: f64) -> bool {
        segment_blocked(self, start, end, radius)
    }

    fn exact_signed_distance_at(&self, point: Vec3) -> f64 {
        self.closest(point.apply_matrix4(self.snapshot.inverse), f64::INFINITY)
    }
}

/// A triangle's normal from its edges (vertex normals may be smoothed and
/// cannot tell inside from outside), not made unit.
fn face_normal(bvh: &MeshBvh, face: usize) -> Vec3 {
    let [i0, i1, i2] = bvh.face(face);
    let (a, b, c) = (bvh.vertex(i0), bvh.vertex(i1), bvh.vertex(i2));
    let (e1, e2) = (
        (b.x - a.x, b.y - a.y, b.z - a.z),
        (c.x - a.x, c.y - a.y, c.z - a.z),
    );
    Vec3::new(
        e1.1 * e2.2 - e1.2 * e2.1,
        e1.2 * e2.0 - e1.0 * e2.2,
        e1.0 * e2.1 - e1.1 * e2.0,
    )
}

/// The world distance from `local` to the model, negative on the inner side
/// of the nearest triangle; infinite when nothing is within `bound` (world
/// mm).
fn signed_distance(
    s: &Snapshot<'_>,
    local: Vec3,
    bound: f64,
    normal: impl FnOnce(usize) -> Vec3,
) -> f64 {
    let local_bound = if bound == f64::INFINITY {
        f64::INFINITY
    } else {
        bound / js_max(0.000001, s.scale)
    };
    let Some(hit) = s.bvh.closest_point_to_point(local, 0.0, local_bound) else {
        return f64::INFINITY;
    };
    let d = hit.distance * s.scale;
    if d > 1e-6 {
        let n = normal(hit.face_index);
        let (dx, dy, dz) = (
            local.x - hit.point.x,
            local.y - hit.point.y,
            local.z - hit.point.z,
        );
        if dx * n.x + dy * n.y + dz * n.z < 0.0 {
            return -d;
        }
    }
    d
}

/// The distance at a cell's centre, worked out to `bound`.
fn cell_distance<S: Store + ?Sized>(store: &S, (qx, qy, qz): (f64, f64, f64), bound: f64) -> f64 {
    let s = store.snapshot();
    let local = Vec3::new(qx * s.cell, qy * s.cell, qz * s.cell).apply_matrix4(s.inverse);
    store.closest(local, bound)
}

/// A distance worked out to `bound`, with nothing found read as `bound`.
fn capped(d: f64, bound: f64) -> f64 {
    if d == f64::INFINITY {
        bound
    } else {
        js_min(d, bound)
    }
}

/// The cached distance of the cell holding the point, worked out to `bound`
/// (and kept as at most `bound`) when not cached yet.
fn cell_distance_bounded<S: Store + ?Sized>(store: &S, p: Vec3, bound: f64) -> f64 {
    let cell = cell_of(p.x, p.y, p.z, store.snapshot().cell);
    if let Some(d) = store.read(cell) {
        return d;
    }
    let d = capped(cell_distance(store, cell, bound), bound);
    store.write(cell, d);
    d
}

/// How far a march may step from `p` past `clearance`, or a negative number
/// when `p` is within it. The cached distance belongs to the cell's centre,
/// so the point's own is within the gap to that centre of it: above that
/// band the cache settles the step, below it the block, and only inside it is
/// the point asked exactly.
fn room<S: Store + ?Sized>(store: &S, p: Vec3, clearance: f64) -> f64 {
    let d = cell_distance_bounded(store, p, MARCH_DISTANCE_BOUND_MM);
    let near = if d == f64::INFINITY {
        MARCH_DISTANCE_BOUND_MM
    } else {
        d
    };
    let s = store.snapshot();
    let gap = {
        let off = |v: f64| js_round(v / s.cell) * s.cell - v;
        let (gx, gy, gz) = (off(p.x), off(p.y), off(p.z));
        (gx * gx + gy * gy + gz * gz).sqrt()
    };
    if near + gap < clearance {
        return -1.0;
    }
    if near - gap >= clearance {
        return near - gap - clearance;
    }
    let bound = clearance + 0.001;
    let exact = capped(store.closest(p.apply_matrix4(s.inverse), bound), bound);
    if exact < clearance {
        -1.0
    } else {
        exact - clearance
    }
}

/// Whether the box round the segment, grown by `margin`, meets `bounds`.
fn near_bounds(bounds: &Box3, a: Vec3, b: Vec3, margin: f64) -> bool {
    bounds.is_empty()
        || (js_max(a.x, b.x) >= bounds.min.x - margin
            && js_min(a.x, b.x) <= bounds.max.x + margin
            && js_max(a.y, b.y) >= bounds.min.y - margin
            && js_min(a.y, b.y) <= bounds.max.y + margin
            && js_max(a.z, b.z) >= bounds.min.z - margin
            && js_min(a.z, b.z) <= bounds.max.z + margin)
}

/// Whether a capsule of `clearance` round `a..b` reaches the model: a
/// vertical one settled by the column map when it can, else marched with
/// steps of at least 0.9 cell and the far end always sampled.
fn segment_blocked<S: Store + ?Sized>(store: &S, a: Vec3, b: Vec3, clearance: f64) -> bool {
    let s = store.snapshot();
    if let Some((map_clearance, map)) = s.columns
        && a.x == b.x
        && a.y == b.y
        && (clearance - map_clearance).abs() < 1e-9
        && map.column_verdict(a.x, a.y, a.z, b.z) == ColumnVerdict::Blocked
    {
        return true;
    }
    let (dx, dy, dz) = (b.x - a.x, b.y - a.y, b.z - a.z);
    let length = (dx * dx + dy * dy + dz * dz).sqrt();
    if length < 0.01 {
        return room(store, a, clearance) < 0.0;
    }
    if !near_bounds(s.bounds, a, b, clearance) {
        return false;
    }
    let inv = 1.0 / length;
    let (ux, uy, uz) = (dx * inv, dy * inv, dz * inv);
    let least = s.cell * 0.9;
    let most_steps = js_max(8.0, (length / least).ceil() + 2.0);
    let (mut t, mut steps) = (0.0, 0.0);
    while steps < most_steps {
        let step = room(
            store,
            Vec3::new(a.x + ux * t, a.y + uy * t, a.z + uz * t),
            clearance,
        );
        if step < 0.0 {
            return true;
        }
        t += if step > least { step } else { least };
        if t >= length {
            break;
        }
        steps += 1.0;
    }
    room(store, b, clearance) < 0.0
}
