/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Drains the raft covers (#467): a hollow standing on its raft with a
//! drain on its underside keeps the resin in when the raft is drawn over the
//! drain's mouth. Generate says so; nothing is moved.

use std::sync::Arc;

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::Vertex;
use sc_mesh_core::transform::Transform3D;
use sc_scene::{CsgPolicy, ReferencePolicy};

use crate::hollow_run::Hollowed;
use crate::scene_document::SceneDocument;

/// How far above the raft's top a drain's mouth may lie and still be shut by
/// it, mm.
const MOUTH_ABOVE_RAFT_MM: f32 = 0.3;

/// The top of a raft, as triangles seen from above: the faces of `mesh`
/// under `at`, all below `top_mm` and facing up.
pub(crate) fn raft_top(mesh: &IndexedMesh, at: &Transform3D, top_mm: f64) -> Vec<[[f32; 2]; 3]> {
    let top = top_mm as f32 + 1e-3;
    let points: Vec<[f32; 3]> = mesh
        .vertices()
        .iter()
        .map(|v| at.apply_to_vertex(*v).coords())
        .collect();
    mesh.faces()
        .iter()
        .filter_map(|f| {
            let [a, b, c] = f.vertices.map(|i| points[usize::from(i)]);
            if a[2] > top || b[2] > top || c[2] > top {
                return None;
            }
            let up = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
            (up > 1e-9).then_some([[a[0], a[1]], [b[0], b[1]], [c[0], c[1]]])
        })
        .collect()
}

fn inside(p: [f32; 2], t: &[[f32; 2]; 3]) -> bool {
    let side =
        |a: [f32; 2], b: [f32; 2]| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
    let (d0, d1, d2) = (side(t[0], t[1]), side(t[1], t[2]), side(t[2], t[0]));
    (d0 >= 0.0 && d1 >= 0.0 && d2 >= 0.0) || (d0 <= 0.0 && d1 <= 0.0 && d2 <= 0.0)
}

/// How many of the drain mouths `mouths` (world points) the raft tops
/// `rafts` (with the top heights) shut: at most a little above a raft's top,
/// over its footprint.
pub(crate) fn shut(mouths: &[[f32; 3]], rafts: &[(Vec<[[f32; 2]; 3]>, f64)]) -> usize {
    mouths
        .iter()
        .filter(|m| {
            rafts.iter().any(|(tris, top)| {
                m[2] <= *top as f32 + MOUTH_ABOVE_RAFT_MM
                    && tris.iter().any(|t| inside([m[0], m[1]], t))
            })
        })
        .count()
}

/// One drain of a hollow in `document`, in world space: its mouth, the far
/// end of its channel, and its radius.
pub(crate) struct WorldDrain {
    pub(crate) mouth: [f32; 3],
    pub(crate) end: [f32; 3],
    pub(crate) radius_mm: f32,
}

/// How far a drain's channel reaches into the model when its depth is the
/// wall's own (#316), in walls.
const CHANNEL_WALLS: f32 = 1.5;
/// How much wider than the drain no tip may land (#465), mm.
const CHANNEL_MARGIN_MM: f32 = 0.3;

/// Every drain of the hollows in `document`, in world space.
pub(crate) fn world_drains(document: &SceneDocument) -> Vec<WorldDrain> {
    let hollows: Vec<Hollowed> = document.hollows().borrow().clone();
    if hollows.iter().all(|h| h.drains.is_empty()) {
        return Vec::new();
    }
    document
        .with_scene(|spatial| {
            let scene = spatial.graph();
            let leaves = scene
                .reachable_leaves(CsgPolicy::Descend, ReferencePolicy::Skip)
                .unwrap_or_default();
            let mut drains = Vec::new();
            for id in leaves {
                let Some(mesh) = scene.leaf_mesh_arc(id) else {
                    continue;
                };
                let Some(h) = hollows.iter().find(|h| Arc::ptr_eq(&h.hollow, mesh)) else {
                    continue;
                };
                let Ok(world) = scene.world_transform_of(id) else {
                    continue;
                };
                for d in &h.drains {
                    let depth = d
                        .depth_mm
                        .unwrap_or(h.options.wall_mm * CHANNEL_WALLS + 1.0);
                    let len =
                        (d.direction[0].powi(2) + d.direction[1].powi(2) + d.direction[2].powi(2))
                            .sqrt()
                            .max(1e-6);
                    let far = [0, 1, 2].map(|k| d.center[k] + d.direction[k] / len * depth);
                    let mouth = world.apply_to_vertex(Vertex::new(d.center)).coords();
                    let end = world.apply_to_vertex(Vertex::new(far)).coords();
                    // The model's scale along the channel, for its width.
                    let world_len = ((end[0] - mouth[0]).powi(2)
                        + (end[1] - mouth[1]).powi(2)
                        + (end[2] - mouth[2]).powi(2))
                    .sqrt();
                    let scale = if depth > 0.0 { world_len / depth } else { 1.0 };
                    drains.push(WorldDrain {
                        mouth,
                        end,
                        radius_mm: d.radius_mm * scale,
                    });
                }
            }
            drains
        })
        .unwrap_or_default()
}

/// The mouths of every drain of the hollows in `document`, in world space.
pub(crate) fn drain_mouths(document: &SceneDocument) -> Vec<[f32; 3]> {
    world_drains(document).iter().map(|d| d.mouth).collect()
}

/// Where no tip may land (#465): each drain's channel, a little wider.
pub(crate) fn drain_channels(document: &SceneDocument) -> Vec<sc_geom::df_supports::KeepOut> {
    let v = |p: [f32; 3]| p.map(f64::from);
    world_drains(document)
        .iter()
        .map(|d| {
            sc_geom::df_supports::KeepOut::between(
                v(d.mouth),
                v(d.end),
                f64::from(d.radius_mm + CHANNEL_MARGIN_MM),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mouth_over_the_raft_is_shut_and_one_beside_it_or_above_it_is_not() {
        // A raft 10 x 10 mm, its top at 0.85 mm.
        let quad = [
            [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]],
            [[0.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
        ];
        let rafts = vec![(quad.to_vec(), 0.85)];
        assert_eq!(shut(&[[5.0, 5.0, 0.9]], &rafts), 1, "on the raft");
        assert_eq!(shut(&[[15.0, 5.0, 0.9]], &rafts), 0, "beside it");
        assert_eq!(shut(&[[5.0, 5.0, 5.0]], &rafts), 0, "lifted over it");
    }

    #[test]
    fn the_raft_top_is_its_low_faces_that_face_up() {
        let v = |x: f32, y: f32, z: f32| Vertex::new([x, y, z]);
        let mesh = IndexedMesh::new(
            None,
            vec![
                v(0.0, 0.0, 0.5),
                v(1.0, 0.0, 0.5),
                v(0.0, 1.0, 0.5),
                v(0.0, 0.0, 5.0),
                v(1.0, 0.0, 5.0),
                v(0.0, 1.0, 5.0),
            ],
            vec![
                sc_mesh_core::primitives::IndexedTriangle::from_raw_indices([0, 1, 2]),
                sc_mesh_core::primitives::IndexedTriangle::from_raw_indices([0, 2, 1]),
                sc_mesh_core::primitives::IndexedTriangle::from_raw_indices([3, 4, 5]),
            ],
        );
        let top = raft_top(&mesh, &Transform3D::Identity, 0.85);
        assert_eq!(top.len(), 1, "the face facing up below the top, alone");
    }
}
