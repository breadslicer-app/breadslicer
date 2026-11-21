/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

//! Every triangle that comes within a distance of a point: the surface a ball
//! there touches (#491).

use super::{MeshBvh, NodeKind, box_distance_sq};
use crate::geom::Vec3;

impl MeshBvh {
    /// Calls `visit(face, distance)` for each triangle whose closest point lies
    /// within `radius` of `point`.
    pub fn faces_within(&self, point: Vec3, radius: f64, mut visit: impl FnMut(usize, f64)) {
        if !self.nodes.is_empty() {
            self.faces_within_below(0, point, radius * radius, &mut visit);
        }
    }

    fn faces_within_below(
        &self,
        index: usize,
        point: Vec3,
        reach_sq: f64,
        visit: &mut impl FnMut(usize, f64),
    ) {
        let node = &self.nodes[index];
        if box_distance_sq(&node.bounds, point) > reach_sq {
            return;
        }
        match node.kind {
            NodeKind::Leaf { offset, count } => {
                for tri in self.leaf(offset, count) {
                    if box_distance_sq(&self.triangle_bounds[tri], point) > reach_sq {
                        continue;
                    }
                    let d_sq =
                        point.distance_to_squared(self.triangle(tri).closest_point_to_point(point));
                    if d_sq <= reach_sq {
                        visit(tri, d_sq.sqrt());
                    }
                }
            }
            NodeKind::Inner { right, .. } => {
                self.faces_within_below(index + 1, point, reach_sq, visit);
                self.faces_within_below(right, point, reach_sq, visit);
            }
        }
    }
}
