/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Directed half-edge records shared by the manifold validator and the
//! topology builder.
//!
//! One record per directed half-edge of the mesh: the key packs the
//! canonical undirected edge, the value packs which face traverses it,
//! through which of the face's 3 edge slots, and in which direction.
//! Sorting the records groups the half-edges of each undirected edge
//! into one contiguous run, ascending by face; a single linear scan
//! over the runs then answers both validation (run length and
//! direction parity) and adjacency (which faces share the edge)
//! without hash tables or per-edge allocation.

use rayon::slice::ParallelSliceMut;

use crate::primitives::{FaceIndex, IndexedTriangle, VertexIndex};

/// `(lo << 32) | hi` for the canonical (`lo <= hi`) vertex pair.
#[inline]
pub(crate) fn key(a: VertexIndex, b: VertexIndex) -> u64 {
    let (a, b) = (a.get(), b.get());
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    (u64::from(lo) << 32) | u64::from(hi)
}

/// The canonical vertex pair a key was packed from.
#[inline]
pub(crate) fn key_vertices(key: u64) -> (VertexIndex, VertexIndex) {
    (
        VertexIndex::from((key >> 32) as u32),
        VertexIndex::from((key & 0xFFFF_FFFF) as u32),
    )
}

/// `face << 3 | slot << 1 | reversed`. `slot` is which of the face's
/// edges this is (0: v0-v1, 1: v1-v2, 2: v2-v0); `reversed` is set
/// when the face traverses the edge from the higher vertex index to
/// the lower. Face indices up to 2^29 fit; a mesh anywhere near that
/// face count would not fit in memory to begin with.
#[inline]
fn value(face: FaceIndex, slot: usize, reversed: bool) -> u32 {
    (face.get() as u32) << 3 | (slot as u32) << 1 | u32::from(reversed)
}

/// The face packed into a record value.
#[inline]
pub(crate) fn face_of(value: u32) -> FaceIndex {
    FaceIndex::from((value >> 3) as usize)
}

/// Whether the record's face traverses the edge higher-to-lower.
#[inline]
pub(crate) fn is_reversed(value: u32) -> bool {
    value & 1 == 1
}

/// All `3 * faces.len()` records, sorted. Within one key's run the
/// values ascend by face index. Records are unique (one per face edge
/// slot, and a face free of zero-length edges never repeats an
/// undirected edge), so the unstable parallel sort is deterministic.
pub(crate) fn sorted_half_edge_records(faces: &[IndexedTriangle]) -> Vec<(u64, u32)> {
    let mut records = Vec::with_capacity(faces.len() * 3);
    for (idx, face) in faces.iter().enumerate() {
        let face_idx = FaceIndex::from(idx);
        let [v0, v1, v2] = face.vertices;
        for (slot, &(a, b)) in [(v0, v1), (v1, v2), (v2, v0)].iter().enumerate() {
            records.push((key(a, b), value(face_idx, slot, a > b)));
        }
    }
    records.par_sort_unstable();
    records
}

#[cfg(test)]
mod half_edges_tests {
    use super::*;

    fn vi(i: u32) -> VertexIndex {
        VertexIndex::from(i)
    }

    #[test]
    fn key_is_canonical_and_round_trips() {
        let forward = key(vi(3), vi(7));
        let reverse = key(vi(7), vi(3));
        assert_eq!(forward, reverse);
        assert_eq!(key_vertices(forward), (vi(3), vi(7)));
    }

    #[test]
    fn value_round_trips_face_and_direction() {
        let v = value(FaceIndex::from(42_usize), 2, true);
        assert_eq!(face_of(v), FaceIndex::from(42_usize));
        assert!(is_reversed(v));
        let v = value(FaceIndex::from(0_usize), 0, false);
        assert_eq!(face_of(v), FaceIndex::from(0_usize));
        assert!(!is_reversed(v));
    }

    #[test]
    fn records_of_a_shared_edge_form_one_run_ascending_by_face() {
        use crate::primitives::Normal;
        // Two triangles sharing edge (0, 1) with opposite traversal.
        let faces = [
            IndexedTriangle::from_raw_indices(Normal::new([0.0, 0.0, 1.0]), [0, 1, 2]),
            IndexedTriangle::from_raw_indices(Normal::new([0.0, 0.0, 1.0]), [1, 0, 3]),
        ];
        let records = sorted_half_edge_records(&faces);
        assert_eq!(records.len(), 6);
        let shared: Vec<&(u64, u32)> = records
            .iter()
            .filter(|(k, _)| *k == key(vi(0), vi(1)))
            .collect();
        assert_eq!(shared.len(), 2);
        assert_eq!(face_of(shared[0].1), FaceIndex::from(0_usize));
        assert_eq!(face_of(shared[1].1), FaceIndex::from(1_usize));
        assert_ne!(is_reversed(shared[0].1), is_reversed(shared[1].1));
    }
}
