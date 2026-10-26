//! Closed test shapes, shared by the tests, the benchmark and the DragonFruit
//! oracle (which builds the same shapes with the same arithmetic).

use crate::mesh::{IndexedMesh, Vec3};

/// DragonFruit's test icosphere (`test_icosphere` in its hollowing tests).
pub fn icosphere(radius: f32, subdivisions: u32) -> IndexedMesh {
    let t = (1.0 + 5.0f32.sqrt()) / 2.0;
    let mut verts: Vec<Vec3> = vec![
        Vec3::new(-1.0, t, 0.0),
        Vec3::new(1.0, t, 0.0),
        Vec3::new(-1.0, -t, 0.0),
        Vec3::new(1.0, -t, 0.0),
        Vec3::new(0.0, -1.0, t),
        Vec3::new(0.0, 1.0, t),
        Vec3::new(0.0, -1.0, -t),
        Vec3::new(0.0, 1.0, -t),
        Vec3::new(t, 0.0, -1.0),
        Vec3::new(t, 0.0, 1.0),
        Vec3::new(-t, 0.0, -1.0),
        Vec3::new(-t, 0.0, 1.0),
    ];
    let mut faces: Vec<[u32; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];

    for _ in 0..subdivisions {
        let mut midpoint_cache: std::collections::HashMap<(u32, u32), u32> =
            std::collections::HashMap::new();
        let mut midpoint = |a: u32, b: u32, verts: &mut Vec<Vec3>| -> u32 {
            let key = if a < b { (a, b) } else { (b, a) };
            if let Some(&idx) = midpoint_cache.get(&key) {
                return idx;
            }
            let pa = verts[a as usize];
            let pb = verts[b as usize];
            let mid = Vec3::new(
                (pa.x + pb.x) * 0.5,
                (pa.y + pb.y) * 0.5,
                (pa.z + pb.z) * 0.5,
            );
            let idx = verts.len() as u32;
            verts.push(mid);
            midpoint_cache.insert(key, idx);
            idx
        };
        let mut next_faces = Vec::with_capacity(faces.len() * 4);
        for [a, b, c] in faces {
            let ab = midpoint(a, b, &mut verts);
            let bc = midpoint(b, c, &mut verts);
            let ca = midpoint(c, a, &mut verts);
            next_faces.push([a, ab, ca]);
            next_faces.push([b, bc, ab]);
            next_faces.push([c, ca, bc]);
            next_faces.push([ab, bc, ca]);
        }
        faces = next_faces;
    }

    for v in &mut verts {
        let len = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt().max(1e-9);
        let s = radius / len;
        *v = Vec3::new(v.x * s, v.y * s, v.z * s);
    }

    IndexedMesh::new(verts, faces)
}

/// DragonFruit's test cube `[min, max]³`, outward (or inward when `flip`).
pub fn cube(min: f32, max: f32, flip: bool) -> IndexedMesh {
    let positions = vec![
        Vec3::new(min, min, min),
        Vec3::new(max, min, min),
        Vec3::new(max, max, min),
        Vec3::new(min, max, min),
        Vec3::new(min, min, max),
        Vec3::new(max, min, max),
        Vec3::new(max, max, max),
        Vec3::new(min, max, max),
    ];
    let mut triangles = vec![
        [0, 2, 1],
        [0, 3, 2],
        [4, 5, 6],
        [4, 6, 7],
        [0, 1, 5],
        [0, 5, 4],
        [1, 2, 6],
        [1, 6, 5],
        [2, 3, 7],
        [2, 7, 6],
        [3, 0, 4],
        [3, 4, 7],
    ];
    if flip {
        for tri in &mut triangles {
            tri.swap(1, 2);
        }
    }
    IndexedMesh::new(positions, triangles)
}

/// A box `[min, max]` subdivided into `n` squares per side edge, so cuts
/// cross many triangles. Outward.
pub fn tessellated_box(min: [f32; 3], max: [f32; 3], n: u32) -> IndexedMesh {
    let n = n.max(1);
    let mut positions = Vec::new();
    let mut index: std::collections::HashMap<[u32; 3], u32> = std::collections::HashMap::new();
    let mut triangles = Vec::new();
    let mut vertex = |g: [u32; 3], positions: &mut Vec<Vec3>| -> u32 {
        *index.entry(g).or_insert_with(|| {
            let p = |k: usize| min[k] + (max[k] - min[k]) * g[k] as f32 / n as f32;
            positions.push(Vec3::new(p(0), p(1), p(2)));
            (positions.len() - 1) as u32
        })
    };
    // Each face: axis `k` fixed at 0 or n; `u`, `v` the other two, ordered
    // so (u, v, outward) is right-handed.
    for k in 0..3 {
        for high in [false, true] {
            let (u, v) = if high {
                ((k + 1) % 3, (k + 2) % 3)
            } else {
                ((k + 2) % 3, (k + 1) % 3)
            };
            for i in 0..n {
                for j in 0..n {
                    let at = |a: u32, b: u32| {
                        let mut g = [0u32; 3];
                        g[k] = if high { n } else { 0 };
                        g[u] = a;
                        g[v] = b;
                        g
                    };
                    let q = [at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)]
                        .map(|g| vertex(g, &mut positions));
                    triangles.push([q[0], q[1], q[2]]);
                    triangles.push([q[0], q[2], q[3]]);
                }
            }
        }
    }
    IndexedMesh::new(positions, triangles)
}

/// Concatenates meshes without welding.
pub fn merged(parts: &[IndexedMesh]) -> IndexedMesh {
    let mut out = IndexedMesh::default();
    for part in parts {
        let offset = out.positions.len() as u32;
        out.positions.extend_from_slice(&part.positions);
        out.triangles
            .extend(part.triangles.iter().map(|t| t.map(|v| v + offset)));
    }
    out
}

/// Reads a binary STL (and welds exact repeats), for benchmarks.
pub fn read_binary_stl(bytes: &[u8]) -> Option<IndexedMesh> {
    if bytes.len() < 84 {
        return None;
    }
    let count = u32::from_le_bytes(bytes[80..84].try_into().ok()?) as usize;
    if bytes.len() < 84 + count * 50 {
        return None;
    }
    let mut soup = Vec::with_capacity(count * 9);
    for i in 0..count {
        let rec = &bytes[84 + i * 50..84 + i * 50 + 50];
        for k in 0..9 {
            let o = 12 + k * 4;
            soup.push(f32::from_le_bytes(rec[o..o + 4].try_into().ok()?));
        }
    }
    let mut map: ahash::AHashMap<[u32; 3], u32> = ahash::AHashMap::with_capacity(count * 2);
    let mut mesh = IndexedMesh::default();
    for tri in soup.chunks_exact(9) {
        let mut ids = [0u32; 3];
        for (c, p) in tri.chunks_exact(3).enumerate() {
            let key = [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()];
            ids[c] = *map.entry(key).or_insert_with(|| {
                mesh.positions.push(Vec3::new(p[0], p[1], p[2]));
                (mesh.positions.len() - 1) as u32
            });
        }
        mesh.triangles.push(ids);
    }
    Some(mesh)
}
