/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The supports a pre-supported file came with, once separated from its
//! model (#258): which model each set belongs to (#259).
//!
//! A set is one leaf whose mesh carries the imported supports' name
//! ([`is_imported_supports`]), saved with the scene like any mesh. Nothing
//! else records whose it is: it belongs to the model it stands under, the one
//! whose footprint on the plate overlaps its own the most. A model and its
//! imported supports move together, so the overlap stays; two models whose
//! supports were separated side by side stay told apart by their footprints.
//!
//! A set is one mesh with many bodies, one per closed shell: the Remove tool
//! takes them off one at a time (#260), found under the cursor through an
//! index of each set's mesh built once ([`body_at`]).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Weak};

use sc_geom::df_supports::{is_any_support_mesh, is_imported_supports};
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
use sc_mesh_core::shells::FaceShells;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_spatial::{ObjectQuery, SceneSpatial};
use sc_scene::NodeId;

/// A footprint on the plate: `[x0, y0, x1, y1]`, mm.
type Footprint = [f64; 4];

/// At least this share of the smaller of two footprints overlaps the other
/// for an imported set to belong to a model.
const MIN_OVERLAP_SHARE: f64 = 0.5;

/// The footprint of `query`'s own box carried into the world by `world`.
fn footprint(query: &ObjectQuery<'_>, world: &Transform3D) -> Option<Footprint> {
    let aabb = query.local_aabb()?;
    let (lo, hi) = (aabb.min().coords(), aabb.max().coords());
    let mut f = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for k in 0..8 {
        let corner = Vertex::new([
            if k & 1 == 0 { lo[0] } else { hi[0] },
            if k & 2 == 0 { lo[1] } else { hi[1] },
            if k & 4 == 0 { lo[2] } else { hi[2] },
        ]);
        let [x, y, _] = world.apply_to_vertex(corner).coords();
        f[0] = f[0].min(f64::from(x));
        f[1] = f[1].min(f64::from(y));
        f[2] = f[2].max(f64::from(x));
        f[3] = f[3].max(f64::from(y));
    }
    f.iter().all(|v| v.is_finite()).then_some(f)
}

fn area(f: &Footprint) -> f64 {
    (f[2] - f[0]).max(0.0) * (f[3] - f[1]).max(0.0)
}

/// How much of the smaller of `a` and `b` the other covers, 0 to 1.
fn overlap_share(a: &Footprint, b: &Footprint) -> f64 {
    let both = [
        a[0].max(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].min(b[3]),
    ];
    let smaller = area(a).min(area(b));
    if smaller <= 0.0 {
        return 0.0;
    }
    area(&both) / smaller
}

/// The leaves of `spatial` that show imported supports, standing at the
/// scene's root (their mesh in world space, as the file had it).
pub(crate) fn imported_leaves(spatial: &SceneSpatial) -> Vec<NodeId> {
    (0..)
        .map_while(|i| spatial.leaf_id_at_index(i))
        .filter(|&id| {
            spatial
                .leaf(id)
                .is_some_and(|q| is_imported_supports(q.mesh()) && q.parent_world().is_identity())
        })
        .collect()
}

/// The models of `spatial`: leaves that show neither kind of supports.
fn model_leaves(spatial: &SceneSpatial) -> Vec<NodeId> {
    (0..)
        .map_while(|i| spatial.leaf_id_at_index(i))
        .filter(|&id| {
            spatial
                .leaf(id)
                .is_some_and(|q| !is_any_support_mesh(q.mesh()) && !q.mesh().faces().is_empty())
        })
        .collect()
}

/// The model the imported supports of `supports` belong to, with each model
/// where the scene has it, or `posed`'s model where `posed` puts it (a model
/// a live edit has already moved, as it stood before). `None` when no
/// model's footprint overlaps the set's enough.
pub(crate) fn owner_of(
    spatial: &SceneSpatial,
    supports: NodeId,
    posed: Option<(NodeId, &Transform3D)>,
) -> Option<NodeId> {
    let set = spatial.leaf(supports)?;
    let set_print = footprint(&set, &set.world_transform())?;
    model_leaves(spatial)
        .into_iter()
        .filter_map(|model| {
            let query = spatial.leaf(model)?;
            let world = match posed {
                Some((id, world)) if id == model => *world,
                _ => query.world_transform(),
            };
            let share = overlap_share(&set_print, &footprint(&query, &world)?);
            (share >= MIN_OVERLAP_SHARE).then_some((model, share))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(model, _)| model)
}

/// The imported support sets of the model `model`, standing where the scene
/// has it or at `model_world`.
pub(crate) fn imported_of(
    spatial: &SceneSpatial,
    model: NodeId,
    model_world: Option<&Transform3D>,
) -> Vec<NodeId> {
    imported_leaves(spatial)
        .into_iter()
        .filter(|&set| owner_of(spatial, set, model_world.map(|w| (model, w))) == Some(model))
        .collect()
}

/// A point on the surface of a body is at most this far from one of its
/// faces, mm: a pick lands on the surface, give or take rounding.
const ON_BODY_MM: f64 = 0.05;
/// The cells the index sorts faces into, mm.
const INDEX_CELL_MM: f64 = 1.0;

/// The bodies of an imported set's mesh, and its faces by cell.
struct BodyIndex {
    mesh: Weak<IndexedMesh>,
    shells: FaceShells,
    cells: HashMap<[i32; 3], Vec<u32>>,
}

thread_local! {
    /// The meshes indexed so far, on the UI thread.
    static INDEXES: RefCell<Vec<Arc<BodyIndex>>> = const { RefCell::new(Vec::new()) };
}

fn cell_of(p: [f64; 3]) -> [i32; 3] {
    p.map(|c| (c / INDEX_CELL_MM).floor() as i32)
}

fn xyz(v: &Vertex) -> [f64; 3] {
    v.coords().map(f64::from)
}

fn index_of(mesh: &Arc<IndexedMesh>) -> Arc<BodyIndex> {
    INDEXES.with(|indexes| {
        let mut indexes = indexes.borrow_mut();
        indexes.retain(|i| i.mesh.strong_count() > 0);
        if let Some(found) = indexes
            .iter()
            .find(|i| i.mesh.as_ptr() == Arc::as_ptr(mesh))
        {
            return found.clone();
        }
        let mut cells: HashMap<[i32; 3], Vec<u32>> = HashMap::new();
        for (f, face) in mesh.faces().iter().enumerate() {
            let corners = face
                .vertices
                .map(|v| xyz(&mesh.vertices()[v.get() as usize]));
            let (mut lo, mut hi) = ([i32::MAX; 3], [i32::MIN; 3]);
            for c in corners {
                let cell = cell_of(c);
                for k in 0..3 {
                    lo[k] = lo[k].min(cell[k]);
                    hi[k] = hi[k].max(cell[k]);
                }
            }
            for x in lo[0]..=hi[0] {
                for y in lo[1]..=hi[1] {
                    for z in lo[2]..=hi[2] {
                        cells.entry([x, y, z]).or_default().push(f as u32);
                    }
                }
            }
        }
        let index = Arc::new(BodyIndex {
            mesh: Arc::downgrade(mesh),
            shells: FaceShells::of_mesh(mesh),
            cells,
        });
        indexes.push(index.clone());
        index
    })
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The distance from `p` to the triangle `a b c` (Ericson, Real-Time
/// Collision Detection, 5.1.5).
fn triangle_distance(p: [f64; 3], a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> f64 {
    let (ab, ac, ap) = (sub(b, a), sub(c, a), sub(p, a));
    let (d1, d2) = (dot(ab, ap), dot(ac, ap));
    let dist = |q: [f64; 3]| dot(sub(p, q), sub(p, q)).sqrt();
    let along =
        |o: [f64; 3], e: [f64; 3], t: f64| [o[0] + t * e[0], o[1] + t * e[1], o[2] + t * e[2]];
    if d1 <= 0.0 && d2 <= 0.0 {
        return dist(a);
    }
    let bp = sub(p, b);
    let (d3, d4) = (dot(ab, bp), dot(ac, bp));
    if d3 >= 0.0 && d4 <= d3 {
        return dist(b);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return dist(along(a, ab, d1 / (d1 - d3)));
    }
    let cp = sub(p, c);
    let (d5, d6) = (dot(ab, cp), dot(ac, cp));
    if d6 >= 0.0 && d5 <= d6 {
        return dist(c);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return dist(along(a, ac, d2 / (d2 - d6)));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return dist(along(b, sub(c, b), (d4 - d3) / ((d4 - d3) + (d5 - d6))));
    }
    let denom = 1.0 / (va + vb + vc);
    let (v, w) = (vb * denom, vc * denom);
    dist([
        a[0] + ab[0] * v + ac[0] * w,
        a[1] + ab[1] * v + ac[1] * w,
        a[2] + ab[2] * v + ac[2] * w,
    ])
}

fn affine(t: &Transform3D) -> AffineTransform3D {
    match t {
        Transform3D::Identity => AffineTransform3D::identity(),
        Transform3D::Affine(a) => *a,
    }
}

/// One body of an imported set: its leaf, and the faces of the leaf's mesh
/// it is made of.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ImportedBody {
    pub(crate) leaf: NodeId,
    pub(crate) faces: Vec<usize>,
}

/// The imported sets the Remove tool may take from, as the hand tools pick
/// a set (#120, #197): the one selected, or the selected model's. None
/// with nothing selected.
pub(crate) fn removable_sets(spatial: &SceneSpatial, selected: Option<NodeId>) -> Vec<NodeId> {
    let Some(leaf) = selected else {
        return Vec::new();
    };
    match spatial.leaf(leaf) {
        Some(q) if is_imported_supports(q.mesh()) => vec![leaf],
        Some(q) if !is_any_support_mesh(q.mesh()) => imported_of(spatial, leaf, None),
        _ => Vec::new(),
    }
}

/// The body of one of the imported sets `sets` whose surface `point` (world
/// mm) is on: the one nearest it. `None` when `point` is on none of them.
pub(crate) fn body_at(
    spatial: &SceneSpatial,
    sets: &[NodeId],
    at: [f64; 3],
) -> Option<ImportedBody> {
    let mut best: Option<(f64, ImportedBody)> = None;
    for &leaf in sets {
        let Some(query) = spatial.leaf(leaf) else {
            continue;
        };
        let Some(mesh) = spatial.graph().leaf_mesh_arc(leaf) else {
            continue;
        };
        let Ok(to_local) = affine(&query.world_transform()).inverse() else {
            continue;
        };
        let p = xyz(&to_local.apply_to_vertex(Vertex::new(at.map(|c| c as f32))));
        let index = index_of(mesh);
        let home = cell_of(p);
        let mut nearest: Option<(f64, usize)> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(faces) = index.cells.get(&[home[0] + dx, home[1] + dy, home[2] + dz])
                    else {
                        continue;
                    };
                    for &f in faces {
                        let face = &mesh.faces()[f as usize];
                        let [a, b, c] = face
                            .vertices
                            .map(|v| xyz(&mesh.vertices()[v.get() as usize]));
                        let d = triangle_distance(p, a, b, c);
                        if nearest.is_none_or(|(n, _)| d < n) {
                            nearest = Some((d, f as usize));
                        }
                    }
                }
            }
        }
        let Some((d, face)) = nearest else {
            continue;
        };
        if d > ON_BODY_MM || best.as_ref().is_some_and(|(b, _)| *b <= d) {
            continue;
        }
        let shell = index.shells.labels()[face];
        let faces: Vec<usize> = index
            .shells
            .labels()
            .iter()
            .enumerate()
            .filter(|(_, s)| **s == shell)
            .map(|(f, _)| f)
            .collect();
        best = Some((d, ImportedBody { leaf, faces }));
    }
    best.map(|(_, body)| body)
}

/// The mesh of `mesh` without the faces `faces` (sorted), its vertices
/// renumbered, keeping its name: `None` when nothing is left.
pub(crate) fn without_faces(mesh: &IndexedMesh, faces: &[usize]) -> Option<IndexedMesh> {
    let mut keep = vec![true; mesh.faces().len()];
    for &f in faces {
        if let Some(k) = keep.get_mut(f) {
            *k = false;
        }
    }
    let mut map = vec![u32::MAX; mesh.vertices().len()];
    let mut vertices = Vec::new();
    let mut kept = Vec::new();
    for (face, _) in mesh.faces().iter().zip(&keep).filter(|(_, k)| **k) {
        let v = face.vertices.map(|v| {
            let i = v.get() as usize;
            if map[i] == u32::MAX {
                map[i] = vertices.len() as u32;
                vertices.push(mesh.vertices()[i]);
            }
            map[i]
        });
        kept.push(IndexedTriangle::from_raw_indices(v));
    }
    (!kept.is_empty()).then(|| IndexedMesh::new(mesh.meta().clone(), vertices, kept))
}

/// The body `body` drawn in the world: what the Remove tool's hover shows
/// red, as positions and triangle indices.
pub(crate) fn body_in_world(
    spatial: &SceneSpatial,
    body: &ImportedBody,
) -> Option<(Vec<[f32; 3]>, Vec<u32>)> {
    let query = spatial.leaf(body.leaf)?;
    let mesh = spatial.graph().leaf_mesh_arc(body.leaf)?;
    let world = affine(&query.world_transform());
    let mut map: HashMap<u32, u32> = HashMap::new();
    let mut positions = Vec::new();
    let mut indices = Vec::new();
    for &f in &body.faces {
        for v in mesh.faces()[f].vertices {
            let i = v.get();
            let at = *map.entry(i).or_insert_with(|| {
                positions.push(world.apply_to_vertex(mesh.vertices()[i as usize]).coords());
                (positions.len() - 1) as u32
            });
            indices.push(at);
        }
    }
    Some((positions, indices))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_distance_to_a_triangle_is_to_its_nearest_point() {
        let (a, b, c) = ([0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]);
        assert!((triangle_distance([0.5, 0.5, 1.0], a, b, c) - 1.0).abs() < 1e-12);
        assert!((triangle_distance([-1.0, 0.0, 0.0], a, b, c) - 1.0).abs() < 1e-12);
        assert!((triangle_distance([2.0, 2.0, 0.0], a, b, c) - 2f64.sqrt()).abs() < 1e-12);
    }

    #[test]
    fn the_overlap_is_a_share_of_the_smaller_footprint() {
        let raft = [0.0, 0.0, 30.0, 30.0];
        let model = [5.0, 5.0, 25.0, 25.0];
        assert!((overlap_share(&raft, &model) - 1.0).abs() < 1e-12);
        let beside = [40.0, 0.0, 60.0, 20.0];
        assert_eq!(overlap_share(&raft, &beside), 0.0);
        let half = [20.0, 0.0, 40.0, 30.0];
        assert!((overlap_share(&raft, &half) - 0.5).abs() < 1e-12);
    }
}
