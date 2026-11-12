/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The islands an analysis found (#525), painted on their model (#542): the
//! faces looking down around where each starts, out to about the island's
//! size, walked over the surface as a brush walks it.

use glam::Vec3;
use sc_mesh_core::primitives::Vertex;
use sc_mesh_spatial::{Dab, SceneSpatial, WalkMarks, dab_faces};
use sc_scene::NodeId;

use crate::picking::{Ray, pick_face_where};
use crate::support_hints::Hint;
use crate::surface_brush::{View, aim};

/// The least an island's paint reaches from where it starts, mm.
const MIN_REACH_MM: f64 = 0.6;
/// The most, so a large island is not painted over the whole underside.
const MAX_REACH_MM: f64 = 4.0;
/// How far below an island the ray that finds its face starts, mm.
const RAY_BELOW_MM: f32 = 1.5;
/// How far the paint stands off the model, outward, so it does not fight
/// the model's own faces for the depth, mm.
const LIFT_MM: f32 = 0.02;
/// An island painted on less than this also gets a dot, mm².
pub(crate) const DOT_UNDER_MM2: f64 = 1.0;

/// An island's paint.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Paint {
    /// The faces painted: three corners each, in world millimetres, and
    /// their area, mm².
    Faces {
        corners: Vec<[f32; 3]>,
        area_mm2: f64,
    },
    /// The face of `leaf` under the island is found, but which faces touch
    /// which is still being worked out for it.
    Waiting(NodeId),
    /// No model face is under the island.
    Nowhere,
}

/// How far an island's paint reaches from where it starts: its own size and
/// a little more.
pub(crate) fn reach_mm(island: &Hint) -> f32 {
    (island.size_mm + 0.3).clamp(MIN_REACH_MM, MAX_REACH_MM) as f32
}

/// The paint of `island` on the models `keep` takes: from the face a ray
/// from just below it meets first, every face looking down within its
/// reach, over the surface.
pub(crate) fn paint(
    spatial: &SceneSpatial,
    island: &Hint,
    keep: &dyn Fn(NodeId) -> bool,
    marks: &mut WalkMarks,
) -> Paint {
    let at = island.at.map(|c| c as f32);
    let reach = reach_mm(island);
    let ray = Ray {
        origin: Vec3::new(at[0], at[1], at[2] - RAY_BELOW_MM),
        direction: Vec3::Z,
    };
    let Some(hit) = pick_face_where(&ray, spatial, keep) else {
        return Paint::Nowhere;
    };
    if hit.t > RAY_BELOW_MM + reach {
        return Paint::Nowhere;
    }
    let Some(query) = spatial.leaf(hit.leaf) else {
        return Paint::Nowhere;
    };
    if !query.has_face_adjacency() {
        return Paint::Waiting(hit.leaf);
    }
    // Seen from below: the faces looking down.
    let view = View {
        eye: [0.0; 3],
        direction: [0.0, 0.0, 1.0],
        orthographic: true,
    };
    let Some(aim) = aim(spatial, &ray, &view, hit.leaf, keep) else {
        return Paint::Nowhere;
    };
    let dab = Dab {
        center: aim.local,
        radius_mm: reach,
        metric: aim.metric,
        facing: aim.facing,
        mirrored: aim.mirrored,
    };
    let mut faces = Vec::new();
    dab_faces(
        query.mesh(),
        query.face_adjacency(),
        aim.face,
        &dab,
        marks,
        &mut faces,
    );
    let world = query.world_transform();
    let mesh = query.mesh();
    let outward = if aim.mirrored { -1.0 } else { 1.0 };
    let mut corners = Vec::with_capacity(faces.len() * 3);
    let mut area_mm2 = 0.0;
    for face in faces {
        let Some(f) = mesh.faces().get(face as usize) else {
            continue;
        };
        let Some(t) = f
            .vertices
            .iter()
            .map(|&v| {
                mesh.vertices()
                    .get(usize::from(v))
                    .map(|v| Vec3::from(world.apply_to_vertex(Vertex::new(v.coords())).coords()))
            })
            .collect::<Option<Vec<Vec3>>>()
        else {
            continue;
        };
        let n = (t[1] - t[0]).cross(t[2] - t[0]);
        let len = n.length();
        if len <= 0.0 {
            continue;
        }
        area_mm2 += f64::from(len) * 0.5;
        let lift = n / len * (LIFT_MM * outward);
        corners.extend(t.iter().map(|p| (*p + lift).to_array()));
    }
    Paint::Faces { corners, area_mm2 }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
    use sc_mesh_core::transform::Transform3D;
    use sc_scene::Scene;
    use sc_scene::node::LeafPayload;

    use super::*;
    use crate::scene_document::SceneDocument;

    /// A closed box from `min` to `max`.
    fn cuboid(min: [f32; 3], max: [f32; 3]) -> IndexedMesh {
        let [x0, y0, z0] = min;
        let [x1, y1, z1] = max;
        let vertices = [
            [x0, y0, z0],
            [x1, y0, z0],
            [x1, y1, z0],
            [x0, y1, z0],
            [x0, y0, z1],
            [x1, y0, z1],
            [x1, y1, z1],
            [x0, y1, z1],
        ]
        .map(Vertex::new)
        .to_vec();
        let faces = [
            [0, 2, 1],
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [1, 2, 6],
            [1, 6, 5],
            [2, 3, 7],
            [2, 7, 6],
            [3, 0, 4],
            [3, 4, 7],
        ]
        .map(IndexedTriangle::from_raw_indices)
        .to_vec();
        IndexedMesh::new(None, vertices, faces)
    }

    fn document_with(mesh: IndexedMesh) -> (std::rc::Rc<SceneDocument>, NodeId) {
        let mut graph = Scene::new();
        let leaf = graph
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .expect("leaf");
        graph.add_root(leaf).expect("root");
        let document = SceneDocument::new();
        document.install_graph(graph, PathBuf::from("/tmp/box.stl"));
        (document, leaf)
    }

    /// An island on a box's underside paints its bottom, looking down, and
    /// nothing of its sides or top; it waits while the adjacency is built.
    #[test]
    fn an_island_paints_the_faces_looking_down_where_it_starts() {
        let (document, leaf) = document_with(cuboid([0.0, 0.0, 5.0], [2.0, 2.0, 9.0]));
        let island = Hint {
            at: [1.0, 1.0, 5.0],
            normal: [0.0, 0.0, -1.0],
            size_mm: 1.0,
        };
        let keep = |id: NodeId| id == leaf;
        let mut marks = WalkMarks::default();
        let first = document
            .with_scene(|spatial| paint(spatial, &island, &keep, &mut marks))
            .expect("a scene");
        assert_eq!(first, Paint::Waiting(leaf), "the adjacency is built first");
        let job = document
            .with_scene(|spatial| spatial.adjacency_job(leaf))
            .flatten()
            .expect("a job");
        job.run();
        let painted = document
            .with_scene(|spatial| paint(spatial, &island, &keep, &mut marks))
            .expect("a scene");
        let Paint::Faces { corners, area_mm2 } = painted else {
            panic!("painted: {painted:?}");
        };
        assert_eq!(corners.len(), 6, "the two faces of the bottom");
        assert!((area_mm2 - 4.0).abs() < 1e-3, "its 4 mm²: {area_mm2}");
        assert!(
            corners.iter().all(|c| c[2] < 5.0 && c[2] > 4.9),
            "on the bottom, lifted off it outward: {corners:?}"
        );
        // Nothing under a place off the model.
        let away = Hint {
            at: [10.0, 10.0, 5.0],
            ..island
        };
        let none = document
            .with_scene(|spatial| paint(spatial, &away, &keep, &mut marks))
            .expect("a scene");
        assert_eq!(none, Paint::Nowhere);
    }
}
