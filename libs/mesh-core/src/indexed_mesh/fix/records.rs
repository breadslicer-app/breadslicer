/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
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
    /// A face's winding order was flipped (vertices reversed).
    FlippedFaceOrientation { face_idx: FaceIndex },
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
}
