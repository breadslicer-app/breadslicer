/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! A cut through the whole plane done again with Manifold's plane split
//! (`manifold-rust`, Apache-2.0), for the meshes our cut does not close: a
//! shell that runs through itself. About a second on a miniature, against
//! tens of milliseconds for ours, so it only runs when ours fails.

use manifold_rust::linalg::Vec3;
use manifold_rust::manifold::Manifold;
use manifold_rust::types::{Error, MeshGL};
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::primitives::{IndexedTriangle, Vertex};

use crate::split::Cut;
use crate::{CutError, Plane, is_closed};

/// `mesh` cut by `plane` with Manifold: the side the normal points to first.
pub(crate) fn cut(mesh: &IndexedMesh, plane: Plane) -> Result<Cut, CutError> {
    let gl = MeshGL {
        num_prop: 3,
        vert_properties: mesh
            .vertices()
            .iter()
            .flat_map(|v| v.as_array().iter().copied())
            .collect(),
        tri_verts: mesh
            .faces()
            .iter()
            .flat_map(|f| f.vertices.map(|v| v.get()))
            .collect(),
        ..Default::default()
    };
    let mut solid = Manifold::from_mesh_gl(&gl);
    if solid.status() != Error::NoError {
        solid = Manifold::from_mesh_gl_robust(&gl);
    }
    if solid.status() != Error::NoError {
        return Err(CutError::Failed(format!("{:?}", solid.status())));
    }
    let n = plane.normal();
    let (first, second) = solid.split_by_plane(Vec3::new(n[0], n[1], n[2]), plane.offset());
    let mut pieces = Vec::new();
    for side in [first, second] {
        if side.status() != Error::NoError {
            return Err(CutError::Failed(format!("{:?}", side.status())));
        }
        if side.is_empty() {
            continue;
        }
        let gl = side.get_mesh_gl(-1);
        let stride = gl.num_prop as usize;
        let piece = IndexedMesh::new(
            None,
            gl.vert_properties
                .chunks_exact(stride)
                .map(|p| Vertex::new([p[0], p[1], p[2]]))
                .collect(),
            gl.tri_verts
                .as_chunks::<3>()
                .0
                .iter()
                .map(|&t| IndexedTriangle::from_raw_indices(t))
                .collect(),
        );
        if !is_closed(&piece) {
            return Err(CutError::Failed("a piece is not closed".into()));
        }
        pieces.push(piece);
    }
    if pieces.len() < 2 {
        return Err(CutError::Misses);
    }
    // The side the normal points to comes first, as ours.
    let reach = |m: &IndexedMesh| {
        m.vertices()
            .iter()
            .map(|v| plane.distance(v.as_array().map(f64::from)))
            .sum::<f64>()
            / m.vertices().len().max(1) as f64
    };
    pieces.sort_by(|a, b| reach(b).total_cmp(&reach(a)));
    Ok(Cut {
        pieces,
        joined_by: Vec::new(),
        by_fallback: true,
    })
}
