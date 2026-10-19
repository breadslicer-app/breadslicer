/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Directed half-edge records shared by the manifold validator and the topology
//! builder.
//!
//! 1 record per directed half-edge of the mesh: the key packs the canonical
//! undirected edge, the value packs which face traverses it, through which of
//! the face's 3 edge slots, and in which direction. Sorting the records groups
//! the half-edges of each undirected edge into 1 contiguous run, ascending by
//! face; a single linear scan over the runs then answers both validation (run
//! length and direction parity) and adjacency (which faces share the edge)
//! without hash tables or per-edge allocation.

use rayon::slice::ParallelSliceMut;

use crate::primitives::{FaceIndex, IndexedTriangle, VertexIndex};

/// Packs the canonical (`lo <= hi`) vertex pair as `(lo << 32) | hi`.
#[inline]
pub(crate) fn key(a: VertexIndex, b: VertexIndex) -> u64 {
    let (a, b) = (a.get(), b.get());
    // #[gamma::skip(relational.le_to_lt, reason = "same pair when a == b")]
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    (u64::from(lo) << 32) | u64::from(hi)
}

/// Returns the canonical vertex pair a key was packed from.
#[inline]
pub(crate) fn key_vertices(key: u64) -> (VertexIndex, VertexIndex) {
    (
        VertexIndex::from((key >> 32) as u32),
        VertexIndex::from((key & 0xFFFF_FFFF) as u32),
    )
}

/// 1 of the 3 edges of a face, in the winding of the face: `V0V1` goes from
/// vertex 0 to vertex 1, `V1V2` from vertex 1 to vertex 2, and `V2V0` from
/// vertex 2 to vertex 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EdgeSlot {
    V0V1 = 0,
    V1V2 = 1,
    V2V0 = 2,
}

impl EdgeSlot {
    pub(crate) const ALL: [Self; 3] = [Self::V0V1, Self::V1V2, Self::V2V0];

    /// Returns the position of the 1st vertex of the edge in its face.
    #[inline]
    pub(crate) const fn index(self) -> usize {
        self as usize
    }

    /// Returns the slot of the edge that starts where this edge ends.
    #[inline]
    pub(crate) const fn next(self) -> Self {
        match self {
            Self::V0V1 => Self::V1V2,
            Self::V1V2 => Self::V2V0,
            Self::V2V0 => Self::V0V1,
        }
    }

    /// Returns the slot of the edge that ends where this edge starts.
    #[inline]
    pub(crate) const fn prev(self) -> Self {
        match self {
            Self::V0V1 => Self::V2V0,
            Self::V1V2 => Self::V0V1,
            Self::V2V0 => Self::V1V2,
        }
    }
}

/// Packs a record value as `face << 3 | slot << 1 | reversed`. `reversed` is
/// set when the face traverses the edge from the higher vertex index to the
/// lower. Face indices below 2^29 fit; a larger index is truncated without an
/// error.
#[inline]
fn value(face: FaceIndex, slot: EdgeSlot, reversed: bool) -> u32 {
    (face.get() as u32) << 3 | (slot as u32) << 1 | u32::from(reversed)
}

/// Returns the face packed into a record value.
#[inline]
pub(crate) fn face_of(value: u32) -> FaceIndex {
    FaceIndex::from((value >> 3) as usize)
}

/// Returns the edge slot of a record value. `value` writes only the slots 0, 1
/// and 2 into the 2 bits of the slot, so the 4th value does not occur; it reads
/// as `V2V0`, so that the decoder has no error for it.
#[inline]
pub(crate) fn slot_of(value: u32) -> EdgeSlot {
    match (value >> 1) & 0b11 {
        0 => EdgeSlot::V0V1,
        1 => EdgeSlot::V1V2,
        _ => EdgeSlot::V2V0,
    }
}

/// Reports whether the record's face traverses the edge higher-to-lower.
#[inline]
pub(crate) fn is_reversed(value: u32) -> bool {
    value & 1 == 1
}

/// Returns all `3 * faces.len()` records, sorted. Within 1 key's run the values
/// ascend by face index. Records are unique (1 per face edge slot, and a face
/// free of zero-length edges never repeats an undirected edge), so the unstable
/// parallel sort is deterministic.
pub(crate) fn sorted_half_edge_records(faces: &[IndexedTriangle]) -> Vec<(u64, u32)> {
    // #[gamma::skip(arith.mul_to_add, reason = "capacity hint only")]
    // #[gamma::skip(arith.mul_to_div, reason = "capacity hint only")]
    // #[gamma::skip(literal.int_decrement, reason = "capacity hint only")]
    // #[gamma::skip(literal.int_increment, reason = "capacity hint only")]
    // #[gamma::skip(literal.int_to_one, reason = "capacity hint only")]
    // #[gamma::skip(literal.int_to_zero, reason = "capacity hint only")]
    let mut records = Vec::with_capacity(faces.len() * 3);
    for (idx, face) in faces.iter().enumerate() {
        let face_idx = FaceIndex::from(idx);
        let [v0, v1, v2] = face.vertices;
        for (slot, (a, b)) in EdgeSlot::ALL
            .into_iter()
            .zip([(v0, v1), (v1, v2), (v2, v0)])
        {
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
        let v = value(FaceIndex::from(42_usize), EdgeSlot::V2V0, true);
        assert_eq!(face_of(v), FaceIndex::from(42_usize));
        assert!(is_reversed(v));
        let v = value(FaceIndex::from(0_usize), EdgeSlot::V0V1, false);
        assert_eq!(face_of(v), FaceIndex::from(0_usize));
        assert!(!is_reversed(v));
    }

    #[test]
    fn value_round_trips_every_slot_in_both_directions() {
        for slot in EdgeSlot::ALL {
            for reversed in [false, true] {
                let v = value(FaceIndex::from(42_usize), slot, reversed);
                assert_eq!(slot_of(v), slot, "slot {slot:?}, reversed {reversed}");
                assert_eq!(face_of(v), FaceIndex::from(42_usize));
                assert_eq!(is_reversed(v), reversed);
            }
        }
    }

    #[test]
    fn slot_of_reads_the_unused_slot_value_as_the_3rd_slot() {
        assert_eq!(slot_of(0b110), EdgeSlot::V2V0);
    }

    #[test]
    fn the_slots_follow_the_winding_of_the_face() {
        assert_eq!(EdgeSlot::ALL.map(EdgeSlot::index), [0, 1, 2]);
        assert_eq!(
            EdgeSlot::ALL.map(EdgeSlot::next),
            [EdgeSlot::V1V2, EdgeSlot::V2V0, EdgeSlot::V0V1]
        );
        assert_eq!(
            EdgeSlot::ALL.map(EdgeSlot::prev),
            [EdgeSlot::V2V0, EdgeSlot::V0V1, EdgeSlot::V1V2]
        );
    }

    #[test]
    fn records_of_a_shared_edge_form_one_run_ascending_by_face() {
        // 2 triangles sharing edge (0, 1) with opposite traversal.
        let faces = [
            IndexedTriangle::from_raw_indices([0, 1, 2]),
            IndexedTriangle::from_raw_indices([1, 0, 3]),
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

    #[test]
    fn a_record_is_reversed_when_its_face_goes_from_the_higher_vertex() {
        let directions = |vertices: [u32; 3]| {
            let mut directions = [false; 3];
            let face = IndexedTriangle::from_raw_indices(vertices);
            for (_, v) in sorted_half_edge_records(&[face]) {
                directions[slot_of(v).index()] = is_reversed(v);
            }
            directions
        };
        // The slots go 0 to 1, 1 to 2 and 2 to 0.
        assert_eq!(directions([0, 1, 2]), [false, false, true]);
        // Slot 0 goes 0 to 0, which is not from a higher index to a lower one.
        assert_eq!(directions([0, 0, 1]), [false, false, true]);
    }
}
