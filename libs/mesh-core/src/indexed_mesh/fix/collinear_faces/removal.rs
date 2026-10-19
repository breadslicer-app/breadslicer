/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use crate::geometry::{cross_of_edges_f64, is_degenerate_at_any_corner};
use crate::primitives::{FaceIndex, IndexedTriangle, Vertex, VertexIndex};

use super::edge_faces::EdgeFaces;
use super::slot_write::SlotWrite;

/// A face with a strictly longest edge, read in its winding as (R, P, Q): P-Q
/// is the longest edge, and R is the vertex opposite to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CollinearFace {
    face: FaceIndex,
    r: VertexIndex,
    p: VertexIndex,
    q: VertexIndex,
}

impl CollinearFace {
    /// Reads `triangle`, the face `face`, as (R, P, Q). Returns `None` when 2
    /// of its edges tie for the longest, as when 2 of its vertices are at 1
    /// position.
    pub(super) fn new(
        face: FaceIndex,
        triangle: IndexedTriangle,
        vertices: &[Vertex],
    ) -> Option<Self> {
        let [a, b, c] = triangle.vertices;
        let [pa, pb, pc] = triangle.vertices.map(|v| vertices[usize::from(v)].to_f64());
        // The squared length of the edge opposite to each vertex.
        let [la, lb, lc] =
            [pb.diff(&pc), pc.diff(&pa), pa.diff(&pb)].map(|edge| edge.squared_norm());
        let [r, p, q] = if la > lb && la > lc {
            [a, b, c]
        } else if lb > lc && lb > la {
            [b, c, a]
        } else if lc > la && lc > lb {
            [c, a, b]
        } else {
            return None;
        };
        Some(Self { face, r, p, q })
    }
}

/// The removal of a collinear face (R, P, Q). A is the face across R-P, B the
/// face across Q-R, and C the face across P-Q.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum CollinearFaceRemoval {
    /// A and B have the same 3rd vertex X: the collinear face and B
    /// (`absorbed`) go, and (P, Q, X) takes the place of A.
    MergeNeighbours {
        collinear: FaceIndex,
        absorbed: FaceIndex,
        merged: SlotWrite,
    },
    /// C, with its 3rd vertex S, is split at R: (R, P, S) takes the place of
    /// the collinear face, and C with R in the place of P stays where C was.
    SplitOpposite {
        collinear: SlotWrite,
        opposite: SlotWrite,
    },
}

/// The faces across the 3 edges of a collinear face, with their 3rd vertices.
struct Neighbours {
    a: FaceIndex,
    x: VertexIndex,
    b: FaceIndex,
    y: VertexIndex,
    c: FaceIndex,
    s: VertexIndex,
}

/// Returns the removal of `collinear`, or `None` when the removal would leave
/// an edge without exactly 2 faces, or make a face that is degenerate at a
/// corner or that faces the other way. `edge_faces` must hold the edges of
/// `faces`.
pub(super) fn choose_removal(
    collinear: &CollinearFace,
    faces: &[IndexedTriangle],
    vertices: &[Vertex],
    edge_faces: &EdgeFaces,
) -> Option<CollinearFaceRemoval> {
    let neighbours = neighbours(collinear, faces, edge_faces)?;
    // C holds R only when the collinear face and C are a closed part of 2
    // faces.
    if neighbours.s == collinear.r {
        return None;
    }
    if neighbours.x == neighbours.y {
        merge(collinear, &neighbours, faces, vertices)
    } else {
        split(collinear, &neighbours, faces, vertices, edge_faces)
    }
}

fn neighbours(
    collinear: &CollinearFace,
    faces: &[IndexedTriangle],
    edge_faces: &EdgeFaces,
) -> Option<Neighbours> {
    let CollinearFace { face, r, p, q } = *collinear;
    let across = |u, v| {
        let other = edge_faces.across(u, v, face)?;
        Some((other, apex(faces[other.get()], u, v)?))
    };
    let (a, x) = across(r, p)?;
    let (b, y) = across(q, r)?;
    let (c, s) = across(p, q)?;
    Some(Neighbours { a, x, b, y, c, s })
}

fn merge(
    collinear: &CollinearFace,
    neighbours: &Neighbours,
    faces: &[IndexedTriangle],
    vertices: &[Vertex],
) -> Option<CollinearFaceRemoval> {
    // With S = X, the 4 faces are a closed part in 1 plane, and (P, Q, X) would
    // be glued to C on all 3 edges.
    if neighbours.s == neighbours.x {
        return None;
    }
    let merged = replaced(faces[neighbours.a.get()], collinear.r, collinear.q);
    // (P, Q, X) has the outline of the 3 faces it replaces, so it faces their
    // way when it is not degenerate.
    if is_degenerate_at_any_corner(&positions(merged, vertices)) {
        return None;
    }
    Some(CollinearFaceRemoval::MergeNeighbours {
        collinear: collinear.face,
        absorbed: neighbours.b,
        merged: SlotWrite {
            face: neighbours.a,
            triangle: merged,
        },
    })
}

fn split(
    collinear: &CollinearFace,
    neighbours: &Neighbours,
    faces: &[IndexedTriangle],
    vertices: &[Vertex],
    edge_faces: &EdgeFaces,
) -> Option<CollinearFaceRemoval> {
    let CollinearFace { face, r, p, .. } = *collinear;
    // A 2nd edge R-S would have 4 faces.
    if edge_faces.has_edge(r, neighbours.s) {
        return None;
    }
    let halves = [
        IndexedTriangle {
            vertices: [r, p, neighbours.s],
        },
        replaced(faces[neighbours.c.get()], p, r),
    ];
    let replaced_crosses =
        [face, neighbours.c].map(|f| cross_of_edges_f64(&positions(faces[f.get()], vertices)));
    for half in halves {
        let corners = positions(half, vertices);
        let cross = cross_of_edges_f64(&corners);
        // A half can face against the faces it replaces when R is very near P
        // or Q, on the side of S.
        let facing =
            cross.dot_product(&replaced_crosses[0]) + cross.dot_product(&replaced_crosses[1]);
        if is_degenerate_at_any_corner(&corners) || facing <= 0.0 {
            return None;
        }
    }
    Some(CollinearFaceRemoval::SplitOpposite {
        collinear: SlotWrite {
            face,
            triangle: halves[0],
        },
        opposite: SlotWrite {
            face: neighbours.c,
            triangle: halves[1],
        },
    })
}

fn positions(triangle: IndexedTriangle, vertices: &[Vertex]) -> [Vertex; 3] {
    triangle.vertices.map(|v| vertices[usize::from(v)])
}

/// Returns `triangle` with `to` in the place of `from`.
fn replaced(triangle: IndexedTriangle, from: VertexIndex, to: VertexIndex) -> IndexedTriangle {
    IndexedTriangle {
        vertices: triangle.vertices.map(|v| if v == from { to } else { v }),
    }
}

/// Returns the vertex of `triangle` that is not on the edge `u`-`v`.
fn apex(triangle: IndexedTriangle, u: VertexIndex, v: VertexIndex) -> Option<VertexIndex> {
    triangle.vertices.into_iter().find(|&w| w != u && w != v)
}

#[cfg(test)]
mod collinear_face_tests {
    use super::*;

    #[test]
    fn every_rotation_of_a_face_gives_the_same_reading() {
        let vertices = [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [1.0, 0.0, 0.0]].map(Vertex::new);
        let expected = CollinearFace {
            face: FaceIndex::from(5_usize),
            r: VertexIndex::from(2_u32),
            p: VertexIndex::from(0_u32),
            q: VertexIndex::from(1_u32),
        };
        for rotation in [[0, 1, 2], [1, 2, 0], [2, 0, 1]] {
            let triangle = IndexedTriangle::from_raw_indices(rotation);
            assert_eq!(
                CollinearFace::new(FaceIndex::from(5_usize), triangle, &vertices),
                Some(expected),
                "{rotation:?}"
            );
        }
    }

    #[test]
    fn a_face_with_2_vertices_at_1_position_has_no_reading() {
        let vertices = [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 0.0, 0.0]].map(Vertex::new);
        let triangle = IndexedTriangle::from_raw_indices([0, 1, 2]);
        assert_eq!(
            CollinearFace::new(FaceIndex::from(0_usize), triangle, &vertices),
            None
        );
    }
}

#[cfg(test)]
mod choose_removal_tests {
    use super::*;
    use crate::indexed_mesh::IndexedMesh;
    use crate::indexed_mesh::fix::collinear_faces::test_meshes::{
        SPLIT_TETRAHEDRON, SQUARE_FAN_PYRAMID, blocked_split_tetrahedron, collinear_pair,
        fan_pyramid, flat_tetrahedron, split_tetrahedron,
    };

    fn fi(index: usize) -> FaceIndex {
        FaceIndex::from(index)
    }

    fn triangle(vertices: [u32; 3]) -> IndexedTriangle {
        IndexedTriangle::from_raw_indices(vertices)
    }

    /// Returns the removal of face 0 of `mesh`, which must be edge-manifold.
    fn removal_of_face_0(mesh: &IndexedMesh) -> Option<CollinearFaceRemoval> {
        let collinear = CollinearFace::new(fi(0), mesh.faces[0], &mesh.vertices)
            .expect("face 0 has a longest edge");
        let edge_faces = EdgeFaces::build(&mesh.faces).expect("edge-manifold");
        choose_removal(&collinear, &mesh.faces, &mesh.vertices, &edge_faces)
    }

    /// Returns [`SQUARE_FAN_PYRAMID`] with R at `r`.
    fn square_fan_pyramid_with_r(r: [f32; 3]) -> IndexedMesh {
        let mut points = SQUARE_FAN_PYRAMID;
        points[2] = r;
        fan_pyramid(points)
    }

    /// Returns a fan pyramid on P (0, 0, 0) and Q (1, 0, 0) in which C is a
    /// thin face with S at `s`, and R is 2^-21 off the line P-Q, at
    /// `r_along_pq` along it.
    fn thin_fan_pyramid(r_along_pq: f32, s: [f32; 3]) -> IndexedMesh {
        let r = [r_along_pq, 2.0_f32.powi(-21), 0.0];
        fan_pyramid([
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            r,
            s,
            [0.0, -1.0, -1.0],
            [1.0, -1.0, -1.0],
        ])
    }

    #[test]
    fn faces_beside_the_short_edges_with_1_third_vertex_merge() {
        assert_eq!(
            removal_of_face_0(&split_tetrahedron(SPLIT_TETRAHEDRON)),
            Some(CollinearFaceRemoval::MergeNeighbours {
                collinear: fi(0),
                absorbed: fi(2),
                merged: SlotWrite {
                    face: fi(1),
                    triangle: triangle([0, 1, 3]),
                },
            })
        );
    }

    #[test]
    fn faces_beside_the_short_edges_with_2_third_vertices_split_the_opposite_face() {
        assert_eq!(
            removal_of_face_0(&fan_pyramid(SQUARE_FAN_PYRAMID)),
            Some(CollinearFaceRemoval::SplitOpposite {
                collinear: SlotWrite {
                    face: fi(0),
                    triangle: triangle([2, 0, 3]),
                },
                opposite: SlotWrite {
                    face: fi(3),
                    triangle: triangle([1, 2, 3]),
                },
            })
        );
    }

    #[test]
    fn a_closed_pair_has_no_removal() {
        assert_eq!(removal_of_face_0(&collinear_pair()), None);
    }

    #[test]
    fn a_closed_flat_tetrahedron_has_no_removal() {
        assert_eq!(removal_of_face_0(&flat_tetrahedron()), None);
    }

    #[test]
    fn a_split_that_would_repeat_an_edge_is_refused() {
        assert_eq!(removal_of_face_0(&blocked_split_tetrahedron()), None);
    }

    #[test]
    fn a_merge_into_a_sliver_is_refused() {
        // (P, Q, X) has a sine of about 2.4e-7 at P.
        let sliver = split_tetrahedron([
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [4.0, 2.0_f32.powi(-20), 0.0],
            [1.0, 1.0, 1.0],
        ]);
        assert_eq!(removal_of_face_0(&sliver), None);
    }

    #[test]
    fn a_split_into_a_sliver_is_refused_for_each_half() {
        let near_p = [2.0 - 2.0_f32.powi(-20), 0.0, 0.0];
        let near_q = [2.0_f32.powi(-20), 0.0, 0.0];
        for r in [near_p, near_q] {
            assert_eq!(
                removal_of_face_0(&square_fan_pyramid_with_r(r)),
                None,
                "{r:?}"
            );
        }
    }

    #[test]
    fn a_split_that_turns_a_half_over_is_refused_for_each_half() {
        let s_height = 2.0_f32.powi(-13);
        let near_p = (2.0_f32.powi(-17), 0.125);
        let near_q = (1.0 - 2.0_f32.powi(-17), 0.875);
        for (r_along_pq, s_along_pq) in [near_p, near_q] {
            // With S on the side of R, the half at the end near R folds back.
            let folded = thin_fan_pyramid(r_along_pq, [s_along_pq, s_height, 0.0]);
            assert_eq!(removal_of_face_0(&folded), None, "{r_along_pq}");
            // With S on the other side, both halves face the way of C.
            let flat = thin_fan_pyramid(r_along_pq, [s_along_pq, -s_height, 0.0]);
            assert!(
                matches!(
                    removal_of_face_0(&flat),
                    Some(CollinearFaceRemoval::SplitOpposite { .. })
                ),
                "{r_along_pq}"
            );
        }
    }

    #[test]
    fn a_face_whose_edges_are_not_in_the_edge_faces_has_no_removal() {
        let mesh = split_tetrahedron(SPLIT_TETRAHEDRON);
        let collinear = CollinearFace::new(fi(0), mesh.faces[0], &mesh.vertices).unwrap();
        let no_edges = EdgeFaces::build(&[]).unwrap();
        assert_eq!(
            choose_removal(&collinear, &mesh.faces, &mesh.vertices, &no_edges),
            None
        );
    }
}
