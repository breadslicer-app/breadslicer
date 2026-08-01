/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Per-layer face buckets for the Z-plane scan.
//!
//! Built once per leaf: every face is assigned the contiguous run of
//! schedule planes it can interact with, and the assignments are
//! gathered into a compressed sparse row (CSR) table: one flat
//! face-index array plus one offset per layer. The per-layer scan
//! then visits only the faces bucketed for its plane instead of the
//! whole mesh, dropping the cost from O(faces) per layer to
//! O(faces touching the plane).
//!
//! Candidacy mirrors the scan's own vertex classification
//! ([`classify_vertex_z`]): a face is bucketed for a plane exactly
//! when its lowest vertex is not `Above` the plane and its highest
//! vertex is not `Below` it. These are the same comparisons the scan
//! uses to dismiss a face as entirely above or entirely below, so a
//! bucket holds exactly the faces the full scan could emit segments
//! for, in the same ascending face order; the bucketed scan
//! reproduces the full scan's output bit for bit.

use sc_mesh_core::indexed_mesh::ValidatedEdgeManifoldMesh;

use rayon::prelude::*;

use crate::plane_intersect::{VertexClass, Z_CLASSIFY_EPS_MM, classify_vertex_z};

/// CSR table of candidate faces per schedule plane.
///
/// `planes` given to [`LayerBuckets::build`] must be the exact plane
/// values the scan will slice at, in ascending order (the layer
/// schedule guarantees strict monotonicity).
pub(crate) struct LayerBuckets {
    /// `offsets[k]..offsets[k + 1]` bounds layer `k`'s slice of
    /// `entries`; length is `planes.len() + 1`.
    offsets: Vec<usize>,
    /// Face indices, ascending within each layer's run.
    entries: Vec<u32>,
}

impl LayerBuckets {
    /// Bucket every face of `mesh` over `planes`.
    ///
    /// The per-face candidate ranges are independent, so that pass
    /// runs in parallel, collected positionally. The CSR assembly
    /// (count, prefix, fill) stays sequential: the fill visits faces
    /// in index order, which is what keeps each layer's run ascending
    /// and the slice output byte-identical. Face indices are stored
    /// as `u32`; a mesh anywhere near `u32::MAX` faces would not fit
    /// in memory to begin with.
    pub(crate) fn build(mesh: &ValidatedEdgeManifoldMesh<'_>, planes: &[f64]) -> Self {
        let vertices = mesh.vertices();
        let faces = mesh.faces();

        // Candidate plane range per face, kept for the fill pass.
        // Leading planes the face floats entirely above (the plane
        // sits below the lowest vertex), and the first plane the face
        // sits entirely below (the plane clears the highest vertex).
        // classify_vertex_z is monotone in the plane for a fixed
        // vertex, so both predicates split the ascending planes in
        // two and partition_point finds the boundary.
        let ranges: Vec<(u32, u32)> = faces
            .par_iter()
            .map(|face| {
                let [i0, i1, i2] = face.vertices;
                let z0 = vertices[usize::from(i0)][2];
                let z1 = vertices[usize::from(i1)][2];
                let z2 = vertices[usize::from(i2)][2];
                let z_min = z0.min(z1).min(z2);
                let z_max = z0.max(z1).max(z2);
                let lo = planes.partition_point(|&z_mm| {
                    classify_vertex_z(z_min, z_mm, Z_CLASSIFY_EPS_MM) == VertexClass::Above
                });
                let hi = planes.partition_point(|&z_mm| {
                    classify_vertex_z(z_max, z_mm, Z_CLASSIFY_EPS_MM) != VertexClass::Below
                });
                (lo as u32, hi as u32)
            })
            .collect();

        let mut counts = vec![0usize; planes.len() + 1];
        for &(lo, hi) in &ranges {
            for count in &mut counts[lo as usize + 1..hi as usize + 1] {
                *count += 1;
            }
        }

        let mut offsets = counts;
        for k in 1..offsets.len() {
            offsets[k] += offsets[k - 1];
        }

        let mut entries = vec![0u32; offsets[planes.len()]];
        let mut cursors: Vec<usize> = offsets[..planes.len()].to_vec();
        for (face_idx, &(lo, hi)) in ranges.iter().enumerate() {
            for cursor in &mut cursors[lo as usize..hi as usize] {
                entries[*cursor] = face_idx as u32;
                *cursor += 1;
            }
        }

        Self { offsets, entries }
    }

    /// The candidate faces for `layer`, ascending.
    pub(crate) fn faces_at(&self, layer: usize) -> &[u32] {
        &self.entries[self.offsets[layer]..self.offsets[layer + 1]]
    }
}

#[cfg(test)]
mod z_bins_tests {
    use sc_mesh_core::IndexedMesh;
    use sc_mesh_fixtures::{
        aob_saddle_tetrahedron, cuboid_with_y_tunnel, cuboid_with_z_tunnel, unit_cube_mesh,
    };

    use super::*;

    fn witness(mesh: &IndexedMesh) -> ValidatedEdgeManifoldMesh<'_> {
        mesh.validated_faces()
            .expect("in-range faces")
            .validated_edge_manifold()
            .expect("manifold mesh")
    }

    /// The definitionally-correct bucket: test every face against the
    /// plane with the same classification the scan uses.
    fn brute_force_bucket(w: &ValidatedEdgeManifoldMesh<'_>, z_mm: f64) -> Vec<u32> {
        let vertices = w.vertices();
        w.faces()
            .iter()
            .enumerate()
            .filter(|(_, face)| {
                let [i0, i1, i2] = face.vertices;
                let z0 = vertices[usize::from(i0)][2];
                let z1 = vertices[usize::from(i1)][2];
                let z2 = vertices[usize::from(i2)][2];
                let z_min = z0.min(z1).min(z2);
                let z_max = z0.max(z1).max(z2);
                classify_vertex_z(z_min, z_mm, Z_CLASSIFY_EPS_MM) != VertexClass::Above
                    && classify_vertex_z(z_max, z_mm, Z_CLASSIFY_EPS_MM) != VertexClass::Below
            })
            .map(|(idx, _)| idx as u32)
            .collect()
    }

    fn assert_buckets_match_brute_force(mesh: &IndexedMesh, planes: &[f64]) {
        let w = witness(mesh);
        let buckets = LayerBuckets::build(&w, planes);
        for (k, &z_mm) in planes.iter().enumerate() {
            assert_eq!(
                buckets.faces_at(k),
                brute_force_bucket(&w, z_mm),
                "bucket mismatch at plane {z_mm}",
            );
        }
    }

    /// Dense sweep across each fixture's span, plus planes exactly at
    /// vertex heights and one classification epsilon inside and
    /// outside them.
    #[test]
    fn buckets_match_the_scan_classification_on_fixtures() {
        let eps = Z_CLASSIFY_EPS_MM;
        let mut planes: Vec<f64> = (0..81).map(|k| -0.2 + 0.03 * f64::from(k)).collect();
        for anchor in [0.0, 0.6, 1.4, 2.0] {
            planes.extend([anchor - 2.0 * eps, anchor - eps, anchor, anchor + eps]);
        }
        planes.sort_by(f64::total_cmp);

        assert_buckets_match_brute_force(&unit_cube_mesh(), &planes);
        assert_buckets_match_brute_force(&cuboid_with_z_tunnel(), &planes);
        assert_buckets_match_brute_force(&cuboid_with_y_tunnel(), &planes);
        assert_buckets_match_brute_force(&aob_saddle_tetrahedron(), &planes);
    }

    #[test]
    fn planes_clear_of_the_mesh_have_empty_buckets() {
        let mesh = cuboid_with_z_tunnel();
        let w = witness(&mesh);
        // The fixture spans z in [0, 2]; the outer planes miss it.
        let planes = [-1.0, 1.0, 5.0];
        let buckets = LayerBuckets::build(&w, &planes);
        assert!(buckets.faces_at(0).is_empty());
        assert!(!buckets.faces_at(1).is_empty());
        assert!(buckets.faces_at(2).is_empty());
    }

    #[test]
    fn a_plane_through_a_vertex_band_keeps_the_touching_faces() {
        let mesh = unit_cube_mesh();
        let w = witness(&mesh);
        // The origin-centred unit cube's top sits at z = 0.5: a plane
        // inside the On band of the top vertices must keep the top
        // faces, one clearly above the band must drop every face.
        // The band edge itself (top + exactly one epsilon) is left
        // unprobed: its classification legitimately depends on the
        // rounding of the sum, on either side.
        let top = 0.5;
        let planes = [
            top,
            top + 0.5 * Z_CLASSIFY_EPS_MM,
            top + 3.0 * Z_CLASSIFY_EPS_MM,
        ];
        let buckets = LayerBuckets::build(&w, &planes);
        assert!(!buckets.faces_at(0).is_empty());
        assert!(!buckets.faces_at(1).is_empty());
        assert!(buckets.faces_at(2).is_empty());
    }

    #[test]
    fn bucket_runs_are_ascending() {
        let mesh = cuboid_with_y_tunnel();
        let w = witness(&mesh);
        let planes: Vec<f64> = (0..41).map(|k| 0.025 + 0.05 * f64::from(k)).collect();
        let buckets = LayerBuckets::build(&w, &planes);
        for k in 0..planes.len() {
            let run = buckets.faces_at(k);
            assert!(run.windows(2).all(|pair| pair[0] < pair[1]));
        }
    }
}
