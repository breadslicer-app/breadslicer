/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! What the overhang map reads on a fine mesh (#498): the surface around each
//! vertex instead of each facet.
//!
//! A sculpted or scanned model carries facets of a few hundredths of a
//! millimetre, and many of a surface that faces up tilt down at that size: by
//! facet, the map shows them as red specks that are no overhang, and leaves
//! plain specks in a real one. Adding a support by hand already reads the
//! surface a tip covers (#491); the map reads it the same way: the mean normal
//! of the surface within [`COVERED_SURFACE_MM`] of each vertex, weighted by
//! area, which the vertex hands the fragment shader to interpolate.
//!
//! Only a vertex whose facets are all small next to that reach reads it: on a
//! plain model a facet is already its surface, and a normal interpolated
//! across a big facet would not be. Where the normals around a vertex cancel
//! (a wall or a strand thinner than the reach) it keeps its facets' too, as
//! #491 does.
//!
//! The surface is read once per mesh and size, on a worker thread. Until it
//! is ready the map paints by facet; turning or moving a model only changes
//! the matrix that carries the normals to the world.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, OnceLock, Weak};

use glam::{Mat4, Vec3};
use rayon::prelude::*;
use sc_geom::df_supports::{COVERED_AGREEMENT, COVERED_SURFACE_MM};
use sc_mesh_core::IndexedMesh;
use sc_scene::{CsgPolicy, Scene, SceneError};
use slint::wgpu_30::wgpu;

use super::device_ops::upload_static_buffer;
use super::scene_upload::{GpuVertex, ModelEntry, ObjectDraw};
use super::smooth_shading::{shades_smooth, smooth_mesh};

/// How far around a vertex the surface is read, world mm: what a hand-placed
/// tip reads (#491).
const REACH_MM: f32 = COVERED_SURFACE_MM as f32;

/// How much the normals within the reach have to agree, the length of their
/// mean over their weight, for the mean to stand for the surface.
const AGREEMENT: f32 = COVERED_AGREEMENT as f32;

/// A vertex reads the surface when no edge of its facets is longer than this
/// share of the reach: a few vertices across the ball, so their samples stand
/// for the surface in it.
const FINE_EDGE_SHARE: f32 = 0.5;

/// Steps per doubling of the reach in the leaf's frame: a size within about
/// 4 % of one already read reuses it.
const REACH_STEPS_PER_DOUBLING: f32 = 8.0;

/// Sizes of one mesh kept read at once: a model and its copies at another
/// scale, without a drag of the scale piling every step of it up.
const SIZES_PER_MESH: usize = 3;

/// Cells per axis of the grid the vertices are binned in, at most.
const MAX_CELLS_PER_AXIS: u32 = 1 << 21;

/// Per vertex of `mesh`, the surface within `reach` of it, in the mesh's
/// frame: xyz its unit normal and w 1 where the map reads it, all 0 where the
/// vertex keeps its facets. `None` when no vertex reads it (a plain model) or
/// the mesh cannot be read.
pub(super) fn read_surface(mesh: &IndexedMesh, reach: f32) -> Option<Vec<[f32; 4]>> {
    let vertices = mesh.vertices();
    let count = vertices.len();
    if count == 0 || !(reach > 0.0 && reach.is_finite()) {
        return None;
    }
    let positions: Vec<Vec3> = vertices.iter().map(|v| Vec3::from(v.coords())).collect();
    if !positions.iter().all(|p| p.is_finite()) {
        return None;
    }
    // Each facet hands a third of its area, and of its area vector, to each
    // of its corners; a vertex is fine when its longest edge is short.
    let mut area_vectors = vec![Vec3::ZERO; count];
    let mut areas = vec![0.0f32; count];
    let mut longest = vec![0.0f32; count];
    for face in mesh.faces() {
        let corners = face.vertices.map(|v| u32::from(v) as usize);
        if corners.iter().any(|&c| c >= count) {
            return None;
        }
        let [a, b, c] = corners.map(|c| positions[c]);
        let cross = (b - a).cross(c - a);
        let area = cross.length() / 2.0;
        let edge = (b - a)
            .length_squared()
            .max((c - b).length_squared())
            .max((a - c).length_squared())
            .sqrt();
        for corner in corners {
            area_vectors[corner] += cross / 6.0;
            areas[corner] += area / 3.0;
            longest[corner] = longest[corner].max(edge);
        }
    }
    let fine_edge = FINE_EDGE_SHARE * reach;
    let fine: Vec<bool> = longest
        .iter()
        .map(|&edge| edge > 0.0 && edge <= fine_edge)
        .collect();
    if !fine.contains(&true) {
        return None;
    }

    // The vertices binned in cells as wide as the reach, sorted cell by cell:
    // a ball of the reach around a point lies within the 27 cells around it.
    let low = positions
        .iter()
        .fold(Vec3::splat(f32::MAX), |m, p| m.min(*p));
    let cell_of = |p: Vec3| -> Option<[u32; 3]> {
        let c = ((p - low) / reach).floor();
        let fits = |x: f32| (0.0..MAX_CELLS_PER_AXIS as f32).contains(&x);
        (fits(c.x) && fits(c.y) && fits(c.z)).then_some([c.x as u32, c.y as u32, c.z as u32])
    };
    let cells: Vec<[u32; 3]> = positions
        .par_iter()
        .map(|&p| cell_of(p))
        .collect::<Option<Vec<_>>>()?;
    let mut order: Vec<u32> = (0..count as u32).collect();
    order.par_sort_unstable_by_key(|&v| cell_key(cells[v as usize]));
    let samples: Vec<Sample> = order
        .iter()
        .map(|&v| {
            let v = v as usize;
            Sample {
                at: positions[v],
                along: if areas[v] > 0.0 {
                    area_vectors[v] / areas[v]
                } else {
                    Vec3::ZERO
                },
                area: areas[v],
            }
        })
        .collect();
    let mut runs: HashMap<u64, (usize, usize)> = HashMap::new();
    let mut start = 0;
    while start < count {
        let key = cell_key(cells[order[start] as usize]);
        let mut end = start + 1;
        while end < count && cell_key(cells[order[end] as usize]) == key {
            end += 1;
        }
        runs.insert(key, (start, end));
        start = end;
    }

    let reach_sq = reach * reach;
    let read: Vec<(u32, [f32; 4])> = runs
        .par_iter()
        .flat_map_iter(|(&key, &(start, end))| {
            let [x, y, z] = cell_of_key(key);
            let mut around: Vec<(usize, usize)> = Vec::with_capacity(27);
            for dz in -1i64..=1 {
                for dy in -1i64..=1 {
                    for dx in -1i64..=1 {
                        let near = [i64::from(x) + dx, i64::from(y) + dy, i64::from(z) + dz];
                        if near
                            .iter()
                            .any(|&c| c < 0 || c >= i64::from(MAX_CELLS_PER_AXIS))
                        {
                            continue;
                        }
                        if let Some(&run) = runs.get(&cell_key(near.map(|c| c as u32))) {
                            around.push(run);
                        }
                    }
                }
            }
            let order = &order;
            let fine = &fine;
            let samples = &samples;
            (start..end)
                .filter(move |&j| fine[order[j] as usize])
                .map(move |j| {
                    let at = samples[j].at;
                    let mut sum = Vec3::ZERO;
                    let mut weight = 0.0f32;
                    for &(from, to) in &around {
                        for sample in &samples[from..to] {
                            let d_sq = (sample.at - at).length_squared();
                            if d_sq >= reach_sq || sample.area <= 0.0 {
                                continue;
                            }
                            // A sample counts at most the disc the ball cuts
                            // from its plane, as a facet does for #491.
                            let w = sample.area.min(std::f32::consts::PI * (reach_sq - d_sq));
                            sum += sample.along * w;
                            weight += w;
                        }
                    }
                    let surface = if weight > 0.0 && sum.length() >= AGREEMENT * weight {
                        let n = sum.normalize();
                        [n.x, n.y, n.z, 1.0]
                    } else {
                        [0.0; 4]
                    };
                    (order[j], surface)
                })
        })
        .collect();
    let mut surface = vec![[0.0f32; 4]; count];
    let mut any = false;
    for (vertex, value) in read {
        any |= value[3] > 0.0;
        surface[vertex as usize] = value;
    }
    any.then_some(surface)
}

/// A vertex as the surface is read from: where it is, its facets' mean
/// normal weighted by area (shorter than 1 where they fold), and its share of
/// their area.
struct Sample {
    at: Vec3,
    along: Vec3,
    area: f32,
}

fn cell_key([x, y, z]: [u32; 3]) -> u64 {
    u64::from(x) | (u64::from(y) << 21) | (u64::from(z) << 42)
}

fn cell_of_key(key: u64) -> [u32; 3] {
    let mask = u64::from(MAX_CELLS_PER_AXIS - 1);
    [
        (key & mask) as u32,
        ((key >> 21) & mask) as u32,
        ((key >> 42) & mask) as u32,
    ]
}

/// The surface per GPU vertex of `mesh` drawn smooth or not: a smooth-shaded
/// mesh splits a vertex at its creases, and each copy reads its source's.
fn gpu_surface(mesh: &IndexedMesh, smooth: bool, reach: f32) -> Option<Vec<[f32; 4]>> {
    let surface = read_surface(mesh, reach)?;
    if !smooth {
        return Some(surface);
    }
    let drawn = smooth_mesh(mesh);
    drawn
        .source
        .iter()
        .map(|&v| surface.get(v as usize).copied())
        .collect()
}

/// The reach in the leaf's frame for the world matrix `model`, by steps (see
/// [`REACH_STEPS_PER_DOUBLING`]), or `None` for a matrix that flattens it.
fn reach_step(model: &Mat4) -> Option<i32> {
    let det = model.determinant().abs();
    if !(det > 0.0 && det.is_finite()) {
        return None;
    }
    // The scale of a volume's cube root: one number for a scale that is not
    // the same on every axis, which bends the ball only a little.
    let local = REACH_MM / det.cbrt();
    let step = (local.log2() * REACH_STEPS_PER_DOUBLING).round();
    step.is_finite().then_some(step as i32)
}

fn reach_of_step(step: i32) -> f32 {
    (step as f32 / REACH_STEPS_PER_DOUBLING).exp2()
}

/// The reach a leaf drawn with `model` reads the surface within, in its
/// frame.
#[cfg(test)]
pub(super) fn reach_for_test(model: &Mat4) -> Option<f32> {
    reach_step(model).map(reach_of_step)
}

/// What a read depends on: the mesh, by address, whether it is drawn smooth
/// (its GPU vertices), and the size of the reach in its frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Key {
    mesh: usize,
    smooth: bool,
    step: i32,
}

type Read = Arc<OnceLock<Option<Vec<[f32; 4]>>>>;

enum State {
    /// The worker reads it.
    Reading(Read),
    /// Uploaded, or `None`: nothing to read, the map paints by facet.
    Ready(Option<Arc<wgpu::Buffer>>),
}

struct CacheEntry {
    /// While it lives, the mesh cannot change in place nor its address go to
    /// another mesh (see `HullPointsCache`).
    mesh: Weak<IndexedMesh>,
    state: State,
    last_used: u64,
}

struct Job {
    mesh: Weak<IndexedMesh>,
    smooth: bool,
    reach: f32,
    read: Weak<OnceLock<Option<Vec<[f32; 4]>>>>,
}

/// The surfaces the overhang map reads, by mesh and size, and the one each
/// object of the frame being drawn hands its vertices.
#[derive(Default)]
pub(super) struct SurfaceNormals {
    entries: HashMap<Key, CacheEntry>,
    jobs: Option<Sender<Job>>,
    /// Per object of the frame, by draw order: the surface buffer it is drawn
    /// with, `None` by facet.
    drawn: Vec<Option<Arc<wgpu::Buffer>>>,
    pending: bool,
    frame: u64,
}

impl SurfaceNormals {
    /// The frame draws no map: every object by facet. What was read stays
    /// for when the map comes back.
    pub(super) fn clear_frame(&mut self) {
        self.drawn.clear();
        self.pending = false;
        self.entries.retain(|_, e| e.mesh.strong_count() > 0);
    }

    /// Finds the surface each mapped object of the frame is drawn with:
    /// `entries` and `objects` are the frame's, in the order of `graph`'s
    /// leaves. Asks the worker for what is not read yet.
    pub(super) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        graph: &Scene,
        entries: &[ModelEntry],
        objects: &[ObjectDraw],
    ) {
        self.frame += 1;
        self.drawn.clear();
        self.pending = false;
        let mut index = 0usize;
        let walked = graph.for_each_expanded_leaf::<SceneError, _>(
            CsgPolicy::Descend,
            |node_id, _, mesh| {
                let i = index;
                index += 1;
                let (Some(entry), Some(object)) = (entries.get(i), objects.get(i)) else {
                    self.drawn.push(None);
                    return Ok(());
                };
                let shared = graph
                    .leaf_mesh_arc(node_id)
                    .filter(|shared| std::ptr::eq(shared.as_ref(), mesh));
                let drawn = match shared {
                    Some(shared) if entry.is_mapped() && !mesh.faces().is_empty() => {
                        self.surface_of(device, queue, shared, &entry.matrix(), object)
                    }
                    _ => None,
                };
                self.drawn.push(drawn);
                Ok(())
            },
        );
        if walked.is_err() {
            self.drawn.clear();
        }
        self.prune();
    }

    /// The surface buffer object `i` of the frame is drawn with.
    pub(super) fn drawn(&self, i: usize) -> Option<&wgpu::Buffer> {
        self.drawn.get(i).and_then(|b| b.as_deref())
    }

    /// Whether a mapped object of the frame waits for its surface: the map
    /// paints it by facet until then.
    pub(super) fn pending(&self) -> bool {
        self.pending
    }

    /// Whether the worker has read a surface an object of the last frame
    /// waited for: the next frame shows it.
    pub(super) fn arrived(&self) -> bool {
        self.entries.values().any(|entry| {
            entry.last_used == self.frame
                && matches!(&entry.state, State::Reading(read) if read.get().is_some())
        })
    }

    fn surface_of(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mesh: &Arc<IndexedMesh>,
        model: &Mat4,
        object: &ObjectDraw,
    ) -> Option<Arc<wgpu::Buffer>> {
        let step = reach_step(model)?;
        let smooth = shades_smooth(mesh);
        let key = Key {
            mesh: Arc::as_ptr(mesh) as usize,
            smooth,
            step,
        };
        let frame = self.frame;
        let entry = match self.entries.entry(key) {
            Entry::Occupied(slot) if slot.get().mesh.strong_count() > 0 => slot.into_mut(),
            Entry::Occupied(mut slot) => {
                slot.insert(Self::request(&mut self.jobs, mesh, smooth, step));
                slot.into_mut()
            }
            Entry::Vacant(slot) => slot.insert(Self::request(&mut self.jobs, mesh, smooth, step)),
        };
        entry.last_used = frame;
        if let State::Reading(read) = &entry.state {
            let Some(done) = read.get() else {
                self.pending = true;
                return None;
            };
            let buffer = done.as_ref().and_then(|surface| {
                upload_static_buffer(
                    device,
                    queue,
                    "mesh-viewer overhang map surface",
                    bytemuck::cast_slice(surface),
                    wgpu::BufferUsages::VERTEX,
                )
                .map(Arc::new)
                .ok()
            });
            entry.state = State::Ready(buffer);
        }
        let State::Ready(Some(buffer)) = &entry.state else {
            return None;
        };
        // The buffers of the object were uploaded with the shading of the
        // time; a change of it not uploaded yet draws by facet meanwhile.
        let vertices = object.vertex_buffer().size() / size_of::<GpuVertex>() as u64;
        (buffer.size() / size_of::<[f32; 4]>() as u64 == vertices).then(|| buffer.clone())
    }

    fn request(
        jobs: &mut Option<Sender<Job>>,
        mesh: &Arc<IndexedMesh>,
        smooth: bool,
        step: i32,
    ) -> CacheEntry {
        let read: Read = Arc::new(OnceLock::new());
        let job = Job {
            mesh: Arc::downgrade(mesh),
            smooth,
            reach: reach_of_step(step),
            read: Arc::downgrade(&read),
        };
        if jobs.get_or_insert_with(spawn_worker).send(job).is_err() {
            // No worker: the map keeps painting this mesh by facet.
            let _ = read.set(None);
        }
        CacheEntry {
            mesh: Arc::downgrade(mesh),
            state: State::Reading(read),
            last_used: 0,
        }
    }

    /// Drops what belongs to meshes that are gone, and the sizes of a mesh
    /// past the last [`SIZES_PER_MESH`] drawn, never one the frame draws: a
    /// read they wait for is skipped.
    fn prune(&mut self) {
        self.entries.retain(|_, e| e.mesh.strong_count() > 0);
        let mut by_mesh: HashMap<usize, Vec<(u64, Key)>> = HashMap::new();
        for (key, entry) in &self.entries {
            by_mesh
                .entry(key.mesh)
                .or_default()
                .push((entry.last_used, *key));
        }
        for mut sizes in by_mesh.into_values() {
            if sizes.len() <= SIZES_PER_MESH {
                continue;
            }
            sizes.sort_unstable_by_key(|&(last_used, _)| std::cmp::Reverse(last_used));
            for (last_used, key) in &sizes[SIZES_PER_MESH..] {
                if *last_used != self.frame {
                    self.entries.remove(key);
                }
            }
        }
    }
}

/// Starts the thread that reads surfaces, one mesh at a time, each spread
/// over the cores. It holds a mesh only while it reads it, skips a read
/// nobody waits for any more, and ends when the cache is dropped.
fn spawn_worker() -> Sender<Job> {
    let (tx, rx) = mpsc::channel::<Job>();
    let spawned = std::thread::Builder::new()
        .name("viewport-surface".into())
        .spawn(move || {
            while let Ok(job) = rx.recv() {
                let Some(read) = job.read.upgrade() else {
                    continue;
                };
                let surface = job
                    .mesh
                    .upgrade()
                    .and_then(|mesh| gpu_surface(&mesh, job.smooth, job.reach));
                let _ = read.set(surface);
            }
        });
    if let Err(err) = spawned {
        // The receiver is gone with the closure: every request reports
        // nothing read, and the map paints by facet as before.
        eprintln!("Failed to spawn the viewport surface worker: {err}");
    }
    tx
}

#[cfg(test)]
mod tests {
    use sc_mesh_core::primitives::{IndexedTriangle, Vertex};

    use super::*;

    /// A grid of `n` × `n` squares of side `step` at height `z(x, y)`, two
    /// facets each, facing up; facing down when `down`.
    fn grid(n: usize, step: f32, down: bool, z: impl Fn(usize, usize) -> f32) -> IndexedMesh {
        let mut vertices = Vec::new();
        for j in 0..=n {
            for i in 0..=n {
                vertices.push(Vertex::new([i as f32 * step, j as f32 * step, z(i, j)]));
            }
        }
        let at = |i: usize, j: usize| (j * (n + 1) + i) as u32;
        let mut faces = Vec::new();
        for j in 0..n {
            for i in 0..n {
                let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
                let quad = if down {
                    [[a, c, b], [a, d, c]]
                } else {
                    [[a, b, c], [a, c, d]]
                };
                for f in quad {
                    faces.push(IndexedTriangle::from_raw_indices(f));
                }
            }
        }
        IndexedMesh {
            meta: None,
            vertices,
            faces,
        }
    }

    fn face_normal(mesh: &IndexedMesh, f: usize) -> Vec3 {
        let [a, b, c] = mesh.faces()[f]
            .vertices
            .map(|v| Vec3::from(mesh.vertices()[u32::from(v) as usize].coords()));
        (b - a).cross(c - a).normalize()
    }

    /// Squares a side of the ridged grids: 2 mm, so a ball of 0.4 mm around
    /// its middle stays on it.
    const RIDGED: usize = 80;

    /// A ridge every other row: a row of vertices lifted so its facets tilt
    /// steeply on each side, on a surface that faces up (down when `down`).
    fn ridged(down: bool) -> IndexedMesh {
        let lift = if down { -0.06 } else { 0.06 };
        grid(
            RIDGED,
            0.025,
            down,
            |_, j| if j % 4 == 2 { lift } else { 0.0 },
        )
    }

    #[test]
    fn a_ridged_top_reads_as_the_surface_it_is_and_not_by_its_facets() {
        let mesh = ridged(false);
        let steep = (0..mesh.faces().len())
            .filter(|&f| face_normal(&mesh, f).z < 0.6)
            .count();
        assert!(steep > 100, "the ridges tilt facets far from up ({steep})");
        let surface = read_surface(&mesh, 0.4).expect("a fine mesh is read");
        // Away from the border, where the ball holds the whole surface.
        let n = RIDGED;
        for j in 30..=50 {
            for i in 30..=50 {
                let [x, y, z, w] = surface[j * (n + 1) + i];
                assert_eq!(w, 1.0, "vertex ({i}, {j}) reads the surface");
                assert!(
                    z > 0.95 && x.abs() < 0.2 && y.abs() < 0.3,
                    "up: {x} {y} {z}"
                );
            }
        }
    }

    #[test]
    fn a_ridged_underside_reads_down_everywhere() {
        let surface = read_surface(&ridged(true), 0.4).expect("a fine mesh is read");
        let n = RIDGED;
        for j in 30..=50 {
            for i in 30..=50 {
                let [_, _, z, w] = surface[j * (n + 1) + i];
                assert!(w == 1.0 && z < -0.95, "({i}, {j}) faces down: {z}");
            }
        }
    }

    #[test]
    fn a_plain_model_keeps_its_facets() {
        // Facets of 1 mm against a reach of 0.4 mm.
        assert!(read_surface(&grid(10, 1.0, false, |_, _| 0.0), 0.4).is_none());
    }

    #[test]
    fn the_underside_of_a_plate_thinner_than_the_reach_keeps_its_facets() {
        // Two fine sheets 0.2 mm apart, the top facing up and the bottom
        // down: around the bottom the top's normals cancel most of its own.
        let top = grid(40, 0.025, false, |_, _| 0.2);
        let bottom = grid(40, 0.025, true, |_, _| 0.0);
        let offset = top.vertices().len() as u32;
        let mut vertices = top.vertices().to_vec();
        vertices.extend_from_slice(bottom.vertices());
        let mut faces = top.faces().to_vec();
        faces.extend(
            bottom.faces().iter().map(|f| {
                IndexedTriangle::from_raw_indices(f.vertices.map(|v| u32::from(v) + offset))
            }),
        );
        let plate = IndexedMesh {
            meta: None,
            vertices,
            faces,
        };
        // Nothing reads the surface here, the top's facets neither.
        let middle = offset as usize + 20 * 41 + 20;
        assert!(read_surface(&plate, 0.4).is_none_or(|surface| surface[middle][3] == 0.0));
    }

    #[test]
    fn a_vertex_shared_by_fine_and_big_facets_keeps_its_facets() {
        // A fine strip next to one big facet: the vertices on the seam have
        // a long edge, the others read the surface.
        let mut mesh = grid(8, 0.025, false, |_, _| 0.0);
        let far = mesh.vertices().len() as u32;
        mesh.vertices.push(Vertex::new([0.1, 5.0, 0.0]));
        // The top row of the grid, j = 8, to the far vertex.
        let row = 8 * 9;
        mesh.faces
            .push(IndexedTriangle::from_raw_indices([row, row + 8, far]));
        let surface = read_surface(&mesh, 0.4).expect("the strip is read");
        assert_eq!(surface[row as usize][3], 0.0);
        assert_eq!(surface[far as usize][3], 0.0);
        assert_eq!(surface[2 * 9 + 4][3], 1.0);
    }

    #[test]
    fn the_reach_steps_with_the_scale_of_the_leaf() {
        let one = reach_step(&Mat4::IDENTITY).expect("a step");
        assert!((reach_of_step(one) - REACH_MM).abs() < 0.02);
        let ten = reach_step(&Mat4::from_scale(Vec3::splat(10.0))).expect("a step");
        assert!((reach_of_step(ten) - REACH_MM / 10.0).abs() < 0.002);
        // Turning a leaf does not change it; flattening it reads nothing.
        let turned = Mat4::from_rotation_z(0.7) * Mat4::from_rotation_x(0.3);
        assert_eq!(reach_step(&turned), Some(one));
        assert_eq!(
            reach_step(&Mat4::from_scale(Vec3::new(1.0, 1.0, 0.0))),
            None
        );
    }

    #[test]
    fn a_smooth_shaded_mesh_hands_each_copy_of_a_vertex_its_sources_surface() {
        let mesh = ridged(false);
        let per_vertex = read_surface(&mesh, 0.4).expect("read");
        let drawn = smooth_mesh(&mesh);
        let gpu = gpu_surface(&mesh, true, 0.4).expect("read");
        assert_eq!(gpu.len(), drawn.vertices.len());
        assert!(
            drawn.vertices.len() > mesh.vertices().len(),
            "creases split vertices"
        );
        for (copy, &source) in drawn.source.iter().enumerate() {
            assert_eq!(gpu[copy], per_vertex[source as usize]);
        }
    }
}
