/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

//! The point of the mesh closest to a point.

use super::{MeshBvh, NodeKind, box_distance_sq};
use crate::geom::Vec3;

/// The closest point, its distance and the face it lies on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClosestPointTarget {
    pub point: Vec3,
    pub distance: f64,
    pub face_index: usize,
}

struct Search {
    point: Vec3,
    /// Stop once a point this close (squared) is found.
    good_enough_sq: f64,
    /// Never look past this distance (squared).
    reach_sq: f64,
    best_sq: f64,
    best: Option<(Vec3, usize)>,
}

impl MeshBvh {
    /// The point of the mesh closest to `point`, `None` when none lies within
    /// `max_threshold`. The search stops at the first point closer than
    /// `min_threshold`. Of two faces equally close, the lower-numbered one.
    pub fn closest_point_to_point(
        &self,
        point: Vec3,
        min_threshold: f64,
        max_threshold: f64,
    ) -> Option<ClosestPointTarget> {
        let mut search = Search {
            point,
            good_enough_sq: min_threshold * min_threshold,
            reach_sq: max_threshold * max_threshold,
            best_sq: f64::INFINITY,
            best: None,
        };
        if !self.nodes.is_empty() {
            self.closest_below(0, &mut search);
        }
        let (point, face_index) = search.best?;
        Some(ClosestPointTarget {
            point,
            distance: search.best_sq.sqrt(),
            face_index,
        })
    }

    /// Searches below node `index`; true once a good enough point is found.
    fn closest_below(&self, index: usize, search: &mut Search) -> bool {
        match self.nodes[index].kind {
            NodeKind::Leaf { offset, count } => {
                for tri in self.leaf(offset, count) {
                    let box_sq = box_distance_sq(&self.triangle_bounds[tri], search.point);
                    if box_sq > search.best_sq || box_sq > search.reach_sq {
                        continue;
                    }
                    let on = self.triangle(tri).closest_point_to_point(search.point);
                    let d_sq = search.point.distance_to_squared(on);
                    let better = d_sq < search.best_sq
                        || (d_sq == search.best_sq && search.best.is_some_and(|(_, f)| tri < f));
                    if better && d_sq <= search.reach_sq {
                        search.best_sq = d_sq;
                        search.best = Some((on, tri));
                    }
                    if d_sq < search.good_enough_sq {
                        return true;
                    }
                }
                false
            }
            NodeKind::Inner { right, .. } => {
                let mut children = [index + 1, right]
                    .map(|c| (box_distance_sq(&self.nodes[c].bounds, search.point), c));
                if children[1].0 < children[0].0 {
                    children.swap(0, 1);
                }
                for (score, child) in children {
                    if score <= search.best_sq
                        && score <= search.reach_sq
                        && self.closest_below(child, search)
                    {
                        return true;
                    }
                }
                false
            }
        }
    }
}
