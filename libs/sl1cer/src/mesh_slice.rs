/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Naive per-Z mesh slicer.
//!
//! Walks every face of a validated manifold mesh at a single Z and
//! collects the crossing segments. O(faces) per layer, no spatial
//! acceleration: deliberately simple so the contour pipeline can be
//! debugged on analytical meshes before any indexing work lands.

use sc_mesh_core::ValidatedManifoldMesh;
use sc_mesh_core::primitives::FaceIndex;

use crate::geom2d::{EdgeId, Segment};
use crate::plane_intersect::{
    TriangleSlice, Z_CLASSIFY_EPS_MM, classify_vertex_z, intersect_triangle_z,
};
use crate::profile::LayerSchedule;

/// Crossing and through-vertex segments produced by intersecting
/// every face of `mesh` with the horizontal plane at `z_mm`.
///
/// `TriangleSlice::Edge` and `TriangleSlice::Coplanar` results are
/// dropped: the Edge case (two on-plane vertices, a whole mesh edge
/// lying on the plane) needs a topology lookup against the other
/// triangle of that edge to decide whether the on-edge actually
/// belongs to the contour; that resolution is the subject of a
/// follow-on phase. `TriangleSlice::ThroughVertex` segments are
/// collected here so that the AOB case is now handled rather than
/// dropped.
pub fn segments_at_z(mesh: &ValidatedManifoldMesh<'_>, z_mm: f64) -> Vec<Segment> {
    let vertices = mesh.vertices();
    let mut segments = Vec::new();
    for (idx, face) in mesh.faces().iter().enumerate() {
        let face_idx = FaceIndex::from(idx);
        let [i0, i1, i2] = face.vertices;
        let v = [
            vertices[usize::from(i0)],
            vertices[usize::from(i1)],
            vertices[usize::from(i2)],
        ];
        let classes = [
            classify_vertex_z(v[0][2], z_mm, Z_CLASSIFY_EPS_MM),
            classify_vertex_z(v[1][2], z_mm, Z_CLASSIFY_EPS_MM),
            classify_vertex_z(v[2][2], z_mm, Z_CLASSIFY_EPS_MM),
        ];
        let edge_ids = [
            EdgeId::new(i0, i1),
            EdgeId::new(i1, i2),
            EdgeId::new(i2, i0),
        ];
        match intersect_triangle_z(v, classes, z_mm, edge_ids, face_idx) {
            TriangleSlice::Crossing { segment } | TriangleSlice::ThroughVertex { segment, .. } => {
                segments.push(segment);
            }
            TriangleSlice::Empty | TriangleSlice::Coplanar | TriangleSlice::Edge { .. } => {}
        }
    }
    segments
}

/// Crossing segments for every layer in `schedule`, paired with the
/// centred Z of each slab. Wraps [`segments_at_z`] over
/// [`LayerSchedule::iter_z`]; the returned vector preserves the
/// schedule's strict-monotone Z ordering.
pub fn slice_mesh(
    mesh: &ValidatedManifoldMesh<'_>,
    schedule: &LayerSchedule,
) -> Vec<(f64, Vec<Segment>)> {
    schedule
        .iter_z()
        .map(|z_mm| (z_mm, segments_at_z(mesh, z_mm)))
        .collect()
}
