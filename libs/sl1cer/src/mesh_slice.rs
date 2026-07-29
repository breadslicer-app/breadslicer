/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Per-Z mesh slicer.
//!
//! Intersects faces of a validated manifold mesh with a single Z
//! plane and collects the crossing segments. [`segments_at_z`] walks
//! every face (O(faces) per layer, the reference path);
//! [`segments_at_z_for_faces`] walks only a caller-supplied
//! candidate list, which the production pipeline feeds from the
//! per-layer buckets in [`crate::z_bins`]. Both share one scan body,
//! so for an ascending candidate list that contains every face
//! touching the plane they emit identical segments in identical
//! order.

use sc_mesh_core::ValidatedManifoldMesh;
use sc_mesh_core::indexed_mesh::MeshTopology;
use sc_mesh_core::primitives::FaceIndex;

use crate::geom2d::{EdgeId, Segment};
use crate::plane_intersect::{
    TriangleSlice, VertexClass, Z_CLASSIFY_EPS_MM, classify_vertex_z, intersect_triangle_z,
};
use crate::profile::LayerSchedule;

/// Crossing, through-vertex, and on-edge segments produced by
/// intersecting every face of `mesh` with the horizontal plane at
/// `z_mm`.
///
/// `topology` is consulted when a triangle returns
/// [`TriangleSlice::Edge`] (the on-edge case): the slicer looks up
/// the other triangle of the on-edge via [`MeshTopology::edge_faces`]
/// and decides whether the surface actually crosses the plane along
/// that edge. The on-edge contributes a contour segment iff the two
/// triangles' third vertices land on opposite sides of the plane.
/// To avoid duplicates, the segment is emitted only from the
/// higher-indexed of the two triangles.
///
/// [`TriangleSlice::Coplanar`] (an entire triangle lies in the plane)
/// is currently dropped; consumers can revisit this when a mesh in
/// the wild demands it.
pub fn segments_at_z(
    mesh: &ValidatedManifoldMesh<'_>,
    z_mm: f64,
    topology: &MeshTopology,
) -> Vec<Segment> {
    let mut segments = Vec::new();
    collect_segments_at_z(mesh, z_mm, topology, 0..mesh.faces().len(), &mut segments);
    segments
}

/// [`segments_at_z`] restricted to the faces listed in `candidates`
/// (ascending face indices). Feeding it every face touching the
/// plane, in ascending order, reproduces [`segments_at_z`]'s output
/// exactly; the production pipeline obtains such a list from the
/// per-layer buckets in [`crate::z_bins`].
pub(crate) fn segments_at_z_for_faces(
    mesh: &ValidatedManifoldMesh<'_>,
    z_mm: f64,
    topology: &MeshTopology,
    candidates: &[u32],
) -> Vec<Segment> {
    // One slot per candidate: a face emits at most one segment, so
    // the vector never grows past this and never reallocates.
    let mut segments = Vec::with_capacity(candidates.len());
    let faces = candidates.iter().map(|&idx| idx as usize);
    collect_segments_at_z(mesh, z_mm, topology, faces, &mut segments);
    segments
}

/// The one scan body behind both entry points: intersect the listed
/// faces with the plane and append the crossing segments in visit
/// order.
fn collect_segments_at_z(
    mesh: &ValidatedManifoldMesh<'_>,
    z_mm: f64,
    topology: &MeshTopology,
    face_indices: impl Iterator<Item = usize>,
    segments: &mut Vec<Segment>,
) {
    let vertices = mesh.vertices();
    let faces = mesh.faces();
    for idx in face_indices {
        let face_idx = FaceIndex::from(idx);
        let [i0, i1, i2] = faces[idx].vertices;
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
            TriangleSlice::Edge { edge_id, segment } => {
                if let Some(seg) = resolve_on_edge_contour_segment(
                    edge_id, segment, face_idx, mesh, topology, z_mm,
                ) {
                    segments.push(seg);
                }
            }
            TriangleSlice::Empty | TriangleSlice::Coplanar => {}
        }
    }
}

/// Decide whether the on-edge `edge_id` of the current `face`
/// contributes to the contour at `z_mm`, and return the segment if
/// so. The decision rule:
///
/// 1. Look up the two faces incident to `edge_id` via `topology`;
///    one is the current face, the other is its on-edge neighbour.
/// 2. Classify each face's third vertex (the one not on the on-edge)
///    against the slicing plane.
/// 3. The on-edge contributes a contour segment iff the two third
///    vertices land on opposite sides of the plane (the surface
///    crosses the plane along the on-edge). When both are above or
///    both are below, the surface only skims the plane along the
///    edge and the contour does not pass through it.
/// 4. To avoid the two on-edge triangles each emitting a copy of
///    the same segment, only the higher-indexed face actually
///    emits.
fn resolve_on_edge_contour_segment(
    edge_id: EdgeId,
    segment: Segment,
    face_idx: FaceIndex,
    mesh: &ValidatedManifoldMesh<'_>,
    topology: &MeshTopology,
    z_mm: f64,
) -> Option<Segment> {
    let edge_faces = topology.edge_faces(edge_id.lo(), edge_id.hi())?;
    let other_face_idx = if edge_faces[0] == face_idx {
        edge_faces[1]
    } else {
        edge_faces[0]
    };

    let faces = mesh.faces();
    let vertices = mesh.vertices();

    let our_third = faces[face_idx.get()]
        .vertices
        .iter()
        .copied()
        .find(|&v| v != edge_id.lo() && v != edge_id.hi())?;
    let other_third = faces[other_face_idx.get()]
        .vertices
        .iter()
        .copied()
        .find(|&v| v != edge_id.lo() && v != edge_id.hi())?;

    let our_class = classify_vertex_z(vertices[usize::from(our_third)][2], z_mm, Z_CLASSIFY_EPS_MM);
    let other_class = classify_vertex_z(
        vertices[usize::from(other_third)][2],
        z_mm,
        Z_CLASSIFY_EPS_MM,
    );

    let contour_relevant = matches!(
        (our_class, other_class),
        (VertexClass::Above, VertexClass::Below) | (VertexClass::Below, VertexClass::Above)
    );

    if contour_relevant && face_idx > other_face_idx {
        Some(segment)
    } else {
        None
    }
}

/// Crossing segments for every layer in `schedule`, paired with the
/// centred Z of each slab. Wraps [`segments_at_z`] over
/// [`LayerSchedule::iter_z`]; the returned vector preserves the
/// schedule's strict-monotone Z ordering.
pub fn slice_mesh(
    mesh: &ValidatedManifoldMesh<'_>,
    schedule: &LayerSchedule,
    topology: &MeshTopology,
) -> Vec<(f64, Vec<Segment>)> {
    schedule
        .iter_z()
        .map(|z_mm| (z_mm, segments_at_z(mesh, z_mm, topology)))
        .collect()
}
