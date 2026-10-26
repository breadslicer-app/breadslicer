//! Taubin vertex smoothing of the cavity surface.
//!
//! Ported from DragonFruit `rust/dragonfruit-mesh-repair/src/smoothing.rs`.
//! Two passes per iteration, one shrinking and one growing (`lambda` then
//! `mu`), Taubin's volume-preserving pair. A candidate move is rejected when it
//! would flatten, invert or blow up any face it touches, and boundary vertices
//! stay where they are.

use crate::mesh::{IndexedMesh, Vec3};

#[derive(Debug, Clone, Copy, Default)]
struct TaubinPassStats {
    applied_vertices: usize,
}

/// Smooth a mesh in place: `iterations` Taubin pairs, with no vertex moving
/// further than `max_step` in one pass. `edge_scale` is the size of the
/// features the surface is made of (a voxel for a cavity) and sets how small a
/// face the guards refuse to collapse.
pub(crate) fn taubin_smooth(
    mesh: &mut IndexedMesh,
    iterations: usize,
    edge_scale: f32,
    max_step: f32,
) {
    if iterations == 0 || mesh.positions.len() < 4 || mesh.triangles.is_empty() {
        return;
    }

    let vertex_count = mesh.positions.len();
    let mut neighbors: Vec<Vec<usize>> = vec![Vec::new(); vertex_count];
    let mut vertex_faces: Vec<Vec<usize>> = vec![Vec::new(); vertex_count];
    let mut edge_counts: std::collections::HashMap<(u32, u32), u8> =
        std::collections::HashMap::with_capacity(mesh.triangles.len() * 2);

    let mut add_edge = |a: u32, b: u32| {
        let ai = a as usize;
        let bi = b as usize;
        neighbors[ai].push(bi);
        neighbors[bi].push(ai);

        let key = if a < b { (a, b) } else { (b, a) };
        let entry = edge_counts.entry(key).or_insert(0);
        *entry = entry.saturating_add(1);
    };

    for tri in &mesh.triangles {
        let [a, b, c] = *tri;
        if a == b || b == c || c == a {
            continue;
        }
        add_edge(a, b);
        add_edge(b, c);
        add_edge(c, a);
    }
    for (face_idx, tri) in mesh.triangles.iter().enumerate() {
        vertex_faces[tri[0] as usize].push(face_idx);
        vertex_faces[tri[1] as usize].push(face_idx);
        vertex_faces[tri[2] as usize].push(face_idx);
    }

    for ring in &mut neighbors {
        ring.sort_unstable();
        ring.dedup();
    }

    let mut boundary_vertex = vec![false; vertex_count];
    for ((a, b), count) in edge_counts {
        if count == 1 {
            boundary_vertex[a as usize] = true;
            boundary_vertex[b as usize] = true;
        }
    }

    let mut positions = mesh.positions.clone();
    let area_floor = (edge_scale * edge_scale * 1e-4).max(1e-8);
    let iterations = iterations.max(1);
    let max_step = max_step.max(0.01);
    let ring = Ring {
        triangles: &mesh.triangles,
        neighbors: &neighbors,
        vertex_faces: &vertex_faces,
        boundary_vertex: &boundary_vertex,
        area_floor,
        max_step,
    };

    for _ in 0..iterations {
        let forward = ring.pass(&mut positions, 0.36);
        let backward = ring.pass(&mut positions, -0.38);
        if forward.applied_vertices + backward.applied_vertices == 0 {
            break;
        }
    }

    mesh.positions = positions;
}

struct Ring<'a> {
    triangles: &'a [[u32; 3]],
    neighbors: &'a [Vec<usize>],
    vertex_faces: &'a [Vec<usize>],
    boundary_vertex: &'a [bool],
    area_floor: f32,
    max_step: f32,
}

impl Ring<'_> {
    fn pass(&self, positions: &mut [Vec3], weight: f32) -> TaubinPassStats {
        let prev = positions.to_vec();
        let mut stats = TaubinPassStats::default();

        for i in 0..positions.len() {
            if self.boundary_vertex[i] {
                continue;
            }
            let ring = &self.neighbors[i];
            if ring.len() < 3 {
                continue;
            }

            let mut centroid = Vec3::ZERO;
            for &j in ring {
                centroid = centroid.add(prev[j]);
            }
            centroid = centroid.scale(1.0 / ring.len() as f32);

            let mut delta = centroid.sub(prev[i]).scale(weight);
            let len = delta.length();
            if len > self.max_step && len > 1e-8 {
                delta = delta.scale(self.max_step / len);
            }

            for scale in [1.0_f32, 0.5, 0.25] {
                let candidate = prev[i].add(delta.scale(scale));
                if !candidate.finite() {
                    continue;
                }
                if !candidate_vertex_update_is_safe(
                    i,
                    candidate,
                    &prev,
                    self.triangles,
                    self.vertex_faces,
                    self.area_floor,
                ) {
                    continue;
                }

                positions[i] = candidate;
                stats.applied_vertices += 1;
                break;
            }
        }

        stats
    }
}

fn candidate_vertex_update_is_safe(
    vertex_index: usize,
    candidate: Vec3,
    prev_positions: &[Vec3],
    triangles: &[[u32; 3]],
    vertex_faces: &[Vec<usize>],
    area_floor: f32,
) -> bool {
    for &face_idx in &vertex_faces[vertex_index] {
        let tri = triangles[face_idx];
        let prev_tri = [
            prev_positions[tri[0] as usize],
            prev_positions[tri[1] as usize],
            prev_positions[tri[2] as usize],
        ];
        let mut next_tri = prev_tri;
        for (corner, &vertex) in tri.iter().enumerate() {
            if vertex as usize == vertex_index {
                next_tri[corner] = candidate;
            }
        }

        let prev_cross = prev_tri[1]
            .sub(prev_tri[0])
            .cross(prev_tri[2].sub(prev_tri[0]));
        let next_cross = next_tri[1]
            .sub(next_tri[0])
            .cross(next_tri[2].sub(next_tri[0]));
        let prev_area2 = prev_cross.length();
        let next_area2 = next_cross.length();

        if !next_area2.is_finite() || next_area2 <= area_floor * 2.0 {
            return false;
        }
        if prev_area2 > area_floor * 4.0 && next_area2 < prev_area2 * 0.12 {
            return false;
        }
        if prev_area2 > area_floor * 4.0 && next_cross.dot(prev_cross) <= 0.0 {
            return false;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cavity_smoothing_rejects_vertex_moves_that_flip_adjacent_triangles() {
        let positions = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(-1.0, 0.0, 0.0),
            Vec3::new(0.0, -1.0, 0.0),
        ];
        let triangles = vec![[0, 1, 2], [0, 2, 3], [0, 3, 4], [0, 4, 1]];
        let vertex_faces = vec![
            vec![0, 1, 2, 3],
            vec![0, 3],
            vec![0, 1],
            vec![1, 2],
            vec![2, 3],
        ];

        assert!(candidate_vertex_update_is_safe(
            0,
            Vec3::new(0.1, 0.1, 0.0),
            &positions,
            &triangles,
            &vertex_faces,
            1e-8,
        ));
        assert!(!candidate_vertex_update_is_safe(
            0,
            Vec3::new(1.6, 1.6, 0.0),
            &positions,
            &triangles,
            &vertex_faces,
            1e-8,
        ));
    }
}
