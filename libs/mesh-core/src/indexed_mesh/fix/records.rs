/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

use crate::primitives::{FaceIndex, IndexedTriangle};

/// Records of the repair operations that a fix routine applied to a mesh. The
/// routine changes the mesh in place and returns these records, so that callers
/// can show a summary: the CLI prints it, the GUI shows it in a dialog.
#[derive(Clone, Debug, PartialEq)]
pub enum MeshFix {
    /// A face was removed because it had a zero-length edge.
    RemovedZeroLengthEdgeFace { face_idx: FaceIndex },
    /// A face was removed because it named a vertex that does not exist.
    RemovedOutOfRangeFace { face_idx: FaceIndex },
    /// A collinear face (zero area, 3 distinct vertices, 1 edge longer than the
    /// other 2) was removed, and the 2 faces beside its short edges became 1
    /// face, which took the place of the 1st of `merged_neighbours`. The record
    /// holds triangles, not face indices, because the pass removes faces.
    RemovedCollinearFaceByMerge {
        collinear: IndexedTriangle,
        merged_neighbours: [IndexedTriangle; 2],
        merged: IndexedTriangle,
    },
    /// A collinear face was removed, and the face across its long edge was
    /// split in 2 at the middle vertex of the collinear face. The `halves` took
    /// the places of `collinear` and `opposite`, in that order.
    RemovedCollinearFaceBySplit {
        collinear: IndexedTriangle,
        opposite: IndexedTriangle,
        halves: [IndexedTriangle; 2],
    },
    /// Faces that cancelled out with a copy in the opposite winding (same 3
    /// positions: the double wall of 2 parts glued face to face) were
    /// removed, both copies.
    RemovedOppositeFaces { faces: usize },
    /// Faces that repeated an earlier face (same 3 positions, same winding:
    /// a part stored twice) were removed; 1 copy of each stays.
    RemovedDuplicateFaces { faces: usize },
    /// Boundary vertices closer than the weld tolerance were merged, which
    /// closes cracks; faces the merge collapsed were removed. The watertight
    /// pass rebuilds the face list, so its records are counts, not indices.
    WeldedBoundaryVertices {
        vertices: usize,
        collapsed_faces: usize,
    },
    /// Edges shared by more than 2 faces were resolved: the faces were paired
    /// around each edge, and its ends got 1 vertex per fan, so parts that
    /// touched at an edge or a vertex became separate closed shells.
    SplitNonManifoldEdges { edges: usize, split_vertices: usize },
    /// Faces left without a partner (on an edge with more than 2 faces after
    /// the split, or across a seam with no consistent winding) were removed;
    /// the hole fill closes what they leave.
    RemovedUnpairedFaces { faces: usize },
    /// Faces were flipped so that each connected piece winds 1 way.
    ReorientedFaces { faces: usize },
    /// Holes were filled with new faces. `largest_hole_edges` is the boundary
    /// length, in edges, of the largest one.
    FilledHoles {
        holes: usize,
        added_faces: usize,
        largest_hole_edges: usize,
    },
    /// Closed shells that were inside out, and not a cavity of another shell,
    /// were turned outward, so the slicer no longer takes them for holes.
    TurnedShellsOutward { shells: usize, faces: usize },
}
