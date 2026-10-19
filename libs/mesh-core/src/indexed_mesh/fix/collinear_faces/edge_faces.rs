/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::collections::HashMap;

use crate::indexed_mesh::half_edges;
use crate::primitives::{FaceIndex, IndexedTriangle, VertexIndex};

use super::SlotWrite;

/// The 2 faces of each edge of a mesh, ascending, keyed by `half_edges::key`.
/// Unlike the edge table of `MeshTopology`, it follows the changes of the pass.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct EdgeFaces(HashMap<u64, [FaceIndex; 2]>);

impl EdgeFaces {
    /// Returns `None` when an edge of `faces` does not have exactly 2 faces.
    pub(super) fn build(faces: &[IndexedTriangle]) -> Option<Self> {
        let records = half_edges::sorted_half_edge_records(faces);
        let mut entries = HashMap::with_capacity(records.len() / 2);
        for run in records.chunk_by(|lhs, rhs| lhs.0 == rhs.0) {
            let [first, second] = run else {
                return None;
            };
            entries.insert(
                first.0,
                [half_edges::face_of(first.1), half_edges::face_of(second.1)],
            );
        }
        Some(Self(entries))
    }

    /// Returns the face across the edge `a`-`b` from `face`, or `None` when
    /// `face` is not 1 of the 2 faces of that edge.
    pub(super) fn across(
        &self,
        a: VertexIndex,
        b: VertexIndex,
        face: FaceIndex,
    ) -> Option<FaceIndex> {
        let [first, second] = *self.0.get(&half_edges::key(a, b))?;
        if first == face {
            Some(second)
        } else if second == face {
            Some(first)
        } else {
            None
        }
    }

    pub(super) fn has_edge(&self, a: VertexIndex, b: VertexIndex) -> bool {
        self.0.contains_key(&half_edges::key(a, b))
    }

    /// Takes the faces of the slots in `deleted` and in `written`, as `faces`
    /// holds them, out of the entries of their edges, and puts the new
    /// triangles of `written` in. All or nothing: returns `false`, with no
    /// change, when a face is not in the entry of 1 of its edges, or when an
    /// edge would end with a number of faces other than 0 or 2.
    pub(super) fn update(
        &mut self,
        faces: &[IndexedTriangle],
        deleted: &[FaceIndex],
        written: &[SlotWrite],
    ) -> bool {
        let mut touched: HashMap<u64, Vec<FaceIndex>> = HashMap::new();
        let old_slots = deleted
            .iter()
            .copied()
            .chain(written.iter().map(|write| write.face));
        for face in old_slots {
            for key in edge_keys(faces[face.get()]) {
                let entry = touched.entry(key).or_insert_with(|| self.faces_of(key));
                let Some(position) = entry.iter().position(|&other| other == face) else {
                    return false;
                };
                entry.swap_remove(position);
            }
        }
        for write in written {
            for key in edge_keys(write.triangle) {
                touched
                    .entry(key)
                    .or_insert_with(|| self.faces_of(key))
                    .push(write.face);
            }
        }
        let mut changes = Vec::with_capacity(touched.len());
        for (key, mut edge_faces) in touched {
            edge_faces.sort_unstable();
            let entry = match edge_faces[..] {
                [] => None,
                [first, second] => Some([first, second]),
                _ => return false,
            };
            changes.push((key, entry));
        }
        for (key, entry) in changes {
            match entry {
                Some(entry) => self.0.insert(key, entry),
                None => self.0.remove(&key),
            };
        }
        true
    }

    fn faces_of(&self, key: u64) -> Vec<FaceIndex> {
        self.0
            .get(&key)
            .map_or_else(Vec::new, |faces| faces.to_vec())
    }
}

fn edge_keys(triangle: IndexedTriangle) -> [u64; 3] {
    let [v0, v1, v2] = triangle.vertices;
    [
        half_edges::key(v0, v1),
        half_edges::key(v1, v2),
        half_edges::key(v2, v0),
    ]
}

#[cfg(test)]
mod edge_faces_tests {
    use super::*;
    use crate::indexed_mesh::fix::collinear_faces::test_meshes::{
        SPLIT_TETRAHEDRON, SQUARE_FAN_PYRAMID, fan_pyramid, split_tetrahedron,
    };

    const TETRAHEDRON: [[u32; 3]; 4] = [[0, 1, 2], [0, 3, 1], [0, 2, 3], [1, 3, 2]];

    fn vi(index: u32) -> VertexIndex {
        VertexIndex::from(index)
    }

    fn fi(index: usize) -> FaceIndex {
        FaceIndex::from(index)
    }

    fn triangle(vertices: [u32; 3]) -> IndexedTriangle {
        IndexedTriangle::from_raw_indices(vertices)
    }

    fn faces(raw: &[[u32; 3]]) -> Vec<IndexedTriangle> {
        raw.iter().copied().map(triangle).collect()
    }

    /// Returns the map of each edge `(a, b)` to its 2 faces.
    fn entries(raw: &[((u32, u32), [usize; 2])]) -> EdgeFaces {
        EdgeFaces(
            raw.iter()
                .map(|&((a, b), faces)| (half_edges::key(vi(a), vi(b)), faces.map(fi)))
                .collect(),
        )
    }

    #[test]
    fn build_maps_each_edge_of_a_tetrahedron_to_its_2_faces() {
        assert_eq!(
            EdgeFaces::build(&faces(&TETRAHEDRON)),
            Some(entries(&[
                ((0, 1), [0, 1]),
                ((1, 2), [0, 3]),
                ((0, 2), [0, 2]),
                ((0, 3), [1, 2]),
                ((1, 3), [1, 3]),
                ((2, 3), [2, 3]),
            ]))
        );
    }

    #[test]
    fn build_refuses_a_boundary_edge() {
        assert_eq!(EdgeFaces::build(&faces(&[[0, 1, 2], [2, 1, 3]])), None);
    }

    #[test]
    fn build_refuses_an_edge_with_4_faces() {
        // 2 tetrahedra on the edge 0-1: every other edge has 2 faces.
        let two_tetrahedra = faces(&[
            [0, 1, 2],
            [0, 3, 1],
            [0, 2, 3],
            [1, 3, 2],
            [0, 1, 4],
            [0, 5, 1],
            [0, 4, 5],
            [1, 5, 4],
        ]);
        assert_eq!(EdgeFaces::build(&two_tetrahedra), None);
    }

    #[test]
    fn across_gives_the_other_face_of_the_edge() {
        let edge_faces = EdgeFaces::build(&faces(&TETRAHEDRON)).unwrap();
        assert_eq!(edge_faces.across(vi(1), vi(0), fi(0)), Some(fi(1)));
        assert_eq!(edge_faces.across(vi(0), vi(1), fi(1)), Some(fi(0)));
        assert_eq!(edge_faces.across(vi(0), vi(1), fi(2)), None);
        assert_eq!(edge_faces.across(vi(0), vi(4), fi(0)), None);
        assert!(edge_faces.has_edge(vi(3), vi(2)));
        assert!(!edge_faces.has_edge(vi(0), vi(4)));
    }

    #[test]
    fn update_follows_the_merge_of_a_split_tetrahedron() {
        let mesh = split_tetrahedron(SPLIT_TETRAHEDRON);
        let mut edge_faces = EdgeFaces::build(&mesh.faces).unwrap();
        let merged = SlotWrite {
            face: fi(1),
            triangle: triangle([0, 1, 3]),
        };
        assert!(edge_faces.update(&mesh.faces, &[fi(0), fi(2)], &[merged]));
        assert_eq!(
            edge_faces,
            entries(&[
                ((0, 1), [1, 3]),
                ((0, 3), [1, 4]),
                ((1, 3), [1, 5]),
                ((0, 4), [3, 4]),
                ((1, 4), [3, 5]),
                ((3, 4), [4, 5]),
            ])
        );
    }

    #[test]
    fn update_follows_the_split_of_a_fan_pyramid() {
        let mesh = fan_pyramid(SQUARE_FAN_PYRAMID);
        let mut edge_faces = EdgeFaces::build(&mesh.faces).unwrap();
        let halves = [
            SlotWrite {
                face: fi(0),
                triangle: triangle([2, 0, 3]),
            },
            SlotWrite {
                face: fi(3),
                triangle: triangle([1, 2, 3]),
            },
        ];
        assert!(edge_faces.update(&mesh.faces, &[], &halves));
        assert_eq!(
            edge_faces,
            entries(&[
                ((1, 2), [2, 3]),
                ((0, 2), [0, 1]),
                ((0, 3), [0, 5]),
                ((1, 3), [3, 7]),
                ((2, 3), [0, 3]),
                ((2, 4), [1, 4]),
                ((0, 4), [1, 5]),
                ((1, 5), [2, 7]),
                ((2, 5), [2, 4]),
                ((4, 5), [4, 6]),
                ((3, 4), [5, 6]),
                ((3, 5), [6, 7]),
            ])
        );
    }

    #[test]
    fn update_refuses_a_face_that_is_not_in_the_entry_of_its_edge() {
        let mut edge_faces = EdgeFaces::build(&faces(&TETRAHEDRON)).unwrap();
        let before = edge_faces.clone();
        // No entry holds the edge 1-4 of face 0, and the entry of the edge 0-1
        // does not hold face 3.
        let other = faces(&[[0, 1, 4], [0, 3, 1], [0, 2, 3], [0, 1, 2]]);
        assert!(!edge_faces.update(&other, &[fi(0)], &[]));
        assert!(!edge_faces.update(&other, &[fi(3)], &[]));
        assert_eq!(edge_faces, before);
    }

    #[test]
    fn update_refuses_an_edge_that_would_not_have_2_faces() {
        let tetrahedron = faces(&TETRAHEDRON);
        let mut edge_faces = EdgeFaces::build(&tetrahedron).unwrap();
        let before = edge_faces.clone();
        // Without face 0, its 3 edges keep 1 face each.
        assert!(!edge_faces.update(&tetrahedron, &[fi(0)], &[]));
        // Face 0 as (0, 1, 3) would leave the edges 1-2 and 0-2 with 1 face,
        // and give the edges 1-3 and 0-3 3 faces.
        let onto_face_1 = SlotWrite {
            face: fi(0),
            triangle: triangle([0, 1, 3]),
        };
        assert!(!edge_faces.update(&tetrahedron, &[], &[onto_face_1]));
        assert_eq!(edge_faces, before);
    }
}
