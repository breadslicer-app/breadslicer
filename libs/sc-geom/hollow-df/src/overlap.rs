//! Parts of a model that overlap (#453): an arm pushed into a torso, two
//! pieces of a kit merged without a boolean. Their faces inside one another
//! are buried, and the voxel classification counted a point inside two
//! parts as outside (two crossings), so the overlap became a void and the
//! buried faces made walls of their own across the cavity.
//!
//! Such a model is joined into one solid before it is hollowed: the union
//! of its outward parts, less its inward ones (the cavities of a model
//! hollowed already). A model whose parts do not overlap goes in as it is,
//! so its result does not change.

use manifold_rust::manifold::Manifold;
use manifold_rust::types::{Error, MeshGL, OpType};

use crate::mesh::{IndexedMesh, Vec3};
use crate::topology::face_shells;

/// One part of the model: its triangles, as a closed mesh of its own.
struct Part {
    mesh: IndexedMesh,
    volume: f64,
    min: [f32; 3],
    max: [f32; 3],
}

fn parts_of(mesh: &IndexedMesh) -> Vec<Part> {
    let (labels, count) = face_shells(mesh);
    let mut triangles: Vec<Vec<[u32; 3]>> = vec![Vec::new(); count];
    for (tri, &label) in mesh.triangles.iter().zip(&labels) {
        triangles[label as usize].push(*tri);
    }
    triangles
        .into_iter()
        .map(|tris| {
            let mut index = vec![u32::MAX; mesh.positions.len()];
            let mut positions = Vec::new();
            let tris: Vec<[u32; 3]> = tris
                .iter()
                .map(|t| {
                    t.map(|v| {
                        if index[v as usize] == u32::MAX {
                            index[v as usize] = positions.len() as u32;
                            positions.push(mesh.positions[v as usize]);
                        }
                        index[v as usize]
                    })
                })
                .collect();
            let part = IndexedMesh::new(positions, tris);
            let (mut min, mut max) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
            for p in &part.positions {
                for (k, c) in [p.x, p.y, p.z].into_iter().enumerate() {
                    min[k] = min[k].min(c);
                    max[k] = max[k].max(c);
                }
            }
            Part {
                volume: part.signed_volume(),
                mesh: part,
                min,
                max,
            }
        })
        .collect()
}

/// Whether the boxes of `a` and `b` share more than a face.
fn boxes_overlap(a: &Part, b: &Part) -> bool {
    (0..3).all(|k| a.min[k] < b.max[k] && b.min[k] < a.max[k])
}

fn to_manifold(mesh: &IndexedMesh, flip: bool) -> Option<Manifold> {
    let gl = MeshGL {
        num_prop: 3,
        vert_properties: mesh
            .positions
            .iter()
            .flat_map(|p| [p.x, p.y, p.z])
            .collect(),
        tri_verts: mesh
            .triangles
            .iter()
            .flat_map(|&[a, b, c]| if flip { [a, c, b] } else { [a, b, c] })
            .collect(),
        ..Default::default()
    };
    let solid = Manifold::from_mesh_gl(&gl);
    (solid.status() == Error::NoError).then_some(solid)
}

/// The most parts joined: past it (a model that came with its supports,
/// hundreds of pieces crossing one another) joining costs minutes, and the
/// model is hollowed as it is.
pub const MAX_JOINED_PARTS: usize = 64;

/// The groups of `parts` (by index) whose boxes overlap, chained: each
/// group of two or more.
fn overlapping_groups(parts: &[&Part]) -> Vec<Vec<usize>> {
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let mut order: Vec<usize> = (0..parts.len()).collect();
    order.sort_by(|&a, &b| parts[a].min[0].total_cmp(&parts[b].min[0]));
    let mut parent: Vec<usize> = (0..parts.len()).collect();
    for (k, &a) in order.iter().enumerate() {
        for &b in &order[k + 1..] {
            if parts[b].min[0] >= parts[a].max[0] {
                break;
            }
            if boxes_overlap(parts[a], parts[b]) {
                let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
                parent[ra.max(rb)] = ra.min(rb);
            }
        }
    }
    let mut groups: Vec<Vec<usize>> = vec![Vec::new(); parts.len()];
    for i in 0..parts.len() {
        let r = root(&mut parent, i);
        groups[r].push(i);
    }
    groups.retain(|g| g.len() > 1);
    groups
}

/// The union of `group`, when its parts really overlap (it holds less
/// than they do apart).
fn joined_group(group: &[&Part]) -> Option<IndexedMesh> {
    let solids: Vec<Manifold> = group
        .iter()
        .map(|p| to_manifold(&p.mesh, false))
        .collect::<Option<_>>()?;
    let joined = Manifold::batch_boolean(&solids, OpType::Add);
    if joined.status() != Error::NoError || joined.is_empty() {
        return None;
    }
    let apart: f64 = group.iter().map(|p| p.volume).sum();
    if joined.volume() >= apart - 1e-6 * apart.max(1.0) {
        return None;
    }
    let gl = joined.get_mesh_gl(-1);
    let stride = gl.num_prop as usize;
    Some(IndexedMesh::new(
        gl.vert_properties
            .chunks_exact(stride)
            .map(|p| Vec3::new(p[0], p[1], p[2]))
            .collect(),
        gl.tri_verts.as_chunks::<3>().0.to_vec(),
    ))
}

/// `mesh` as one solid where some of its parts overlap: each group of
/// outward parts that overlap is replaced by its union, and the other parts
/// (the cavities of a model hollowed already among them) stay as they are.
/// `None` when no part overlaps another, when one that does cannot be joined
/// (a part that is not closed), or past [`MAX_JOINED_PARTS`]: the model is
/// then hollowed as it is.
pub fn joined_parts(mesh: &IndexedMesh) -> Option<IndexedMesh> {
    let parts = parts_of(mesh);
    if parts.len() < 2 {
        return None;
    }
    let outward: Vec<&Part> = parts.iter().filter(|p| p.volume > 0.0).collect();
    let groups = overlapping_groups(&outward);
    if groups.is_empty() || groups.iter().map(Vec::len).sum::<usize>() > MAX_JOINED_PARTS {
        return None;
    }
    let mut taken = vec![false; outward.len()];
    let mut pieces: Vec<IndexedMesh> = Vec::new();
    for group in &groups {
        let members: Vec<&Part> = group.iter().map(|&i| outward[i]).collect();
        if let Some(joined) = joined_group(&members) {
            for &i in group {
                taken[i] = true;
            }
            pieces.push(joined);
        }
    }
    if pieces.is_empty() {
        return None;
    }
    let mut out = IndexedMesh::new(Vec::new(), Vec::new());
    let others = parts
        .iter()
        .filter(|p| {
            !outward
                .iter()
                .zip(&taken)
                .any(|(o, &t)| t && std::ptr::eq(*o, *p))
        })
        .map(|p| &p.mesh);
    for piece in pieces.iter().chain(others) {
        let base = out.positions.len() as u32;
        out.positions.extend_from_slice(&piece.positions);
        out.triangles
            .extend(piece.triangles.iter().map(|t| t.map(|v| v + base)));
    }
    Some(out)
}
