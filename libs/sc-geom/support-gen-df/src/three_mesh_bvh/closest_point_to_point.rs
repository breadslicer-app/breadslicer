//! three-mesh-bvh 0.8.3 `src/core/cast/closestPointToPoint.js`, run through
//! `shapecast` (`src/core/cast/shapecast.js`, `MeshBVH.shapecast`).
//!
//! The shapecast is specialised to these callbacks: `boundsTraverseOrder` is the
//! squared distance to the clamped point, `intersectsBounds` never answers
//! `CONTAINED`, and each leaf walks its triangles in order with
//! `iterateOverTriangles`.
//!
//! One SoulCrafted shortcut leaves every answer as it was (SC-82): a
//! triangle reports only when it is *strictly* closer than the best so far,
//! and it is never closer than its own box. So a triangle whose box is
//! already farther than the best (by more than any rounding of the exact
//! test) cannot report, and its exact test is skipped.

use super::{MeshBvh, Node, NodeKind};
use crate::three::Vec3;

/// `MeshBVHClosestPointTarget`: the closest point, its distance and the face
/// index (in the BVH's reordered triangle order) it lies on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClosestPointTarget {
    pub point: Vec3,
    pub distance: f64,
    pub face_index: usize,
}

/// Slack on the per-triangle box pre-check, in squared millimetres. The exact
/// test's squared distance is off by at most a few `1e-12` for coordinates
/// of hundreds of millimetres; this is a thousand times that.
const SKIP_MARGIN_SQ: f64 = 1e-9;

struct Search {
    point: Vec3,
    min_threshold_sq: f64,
    max_threshold_sq: f64,
    closest_distance_sq: f64,
    closest_distance_tri_index: Option<usize>,
    closest_point: Vec3,
}

#[inline]
fn box_score(node: &Node, point: Vec3) -> f64 {
    let b = &node.bounds;
    let min = Vec3::new(f64::from(b[0]), f64::from(b[1]), f64::from(b[2]));
    let max = Vec3::new(f64::from(b[3]), f64::from(b[4]), f64::from(b[5]));
    point.clamp(min, max).distance_to_squared(point)
}

impl MeshBvh {
    /// `bvh.closestPointToPoint(point, target, minThreshold, maxThreshold)`.
    ///
    /// early out if under minThreshold; skip checking if over maxThreshold;
    /// set minThreshold = maxThreshold to quickly check if a point is within a
    /// threshold. `None` if no value found. DragonFruit's defaults are
    /// `minThreshold = 0`, `maxThreshold = Infinity`.
    ///
    /// A triangle is tested whenever its *leaf box* is within the threshold, so
    /// the reported distance may exceed `max_threshold`; callers cap it.
    pub fn closest_point_to_point(
        &self,
        point: Vec3,
        min_threshold: f64,
        max_threshold: f64,
    ) -> Option<ClosestPointTarget> {
        let mut search = Search {
            point,
            min_threshold_sq: min_threshold * min_threshold,
            max_threshold_sq: max_threshold * max_threshold,
            closest_distance_sq: f64::INFINITY,
            closest_distance_tri_index: None,
            closest_point: Vec3::ZERO,
        };

        for root in &self.roots {
            if self.traverse(root, 0, &mut search) {
                break;
            }
        }

        if search.closest_distance_sq == f64::INFINITY {
            return None;
        }
        Some(ClosestPointTarget {
            point: search.closest_point,
            distance: search.closest_distance_sq.sqrt(),
            face_index: search
                .closest_distance_tri_index
                .expect("a finite distance has a triangle"),
        })
    }

    /// `intersectsTriangle` over a leaf's range; true stops the traversal.
    fn intersects_range(&self, offset: u32, count: u16, search: &mut Search) -> bool {
        let start = offset as usize;
        for tri in start..start + count as usize {
            // Never closer than its own box: past the best, it cannot report.
            let b = &self.triangle_bounds[tri];
            let box_sq = search
                .point
                .clamp(
                    Vec3::new(f64::from(b[0]), f64::from(b[1]), f64::from(b[2])),
                    Vec3::new(f64::from(b[3]), f64::from(b[4]), f64::from(b[5])),
                )
                .distance_to_squared(search.point);
            if box_sq > search.closest_distance_sq + SKIP_MARGIN_SQ {
                continue;
            }
            let temp = self.triangle(tri).closest_point_to_point(search.point);
            let dist_sq = search.point.distance_to_squared(temp);
            if dist_sq < search.closest_distance_sq {
                search.closest_point = temp;
                search.closest_distance_sq = dist_sq;
                search.closest_distance_tri_index = Some(tri);
            }
            if dist_sq < search.min_threshold_sq {
                return true;
            }
        }
        false
    }

    /// `intersectsBounds`: `score < closestDistanceSq && score < maxThresholdSq`.
    #[inline]
    fn intersects_bounds(score: f64, search: &Search) -> bool {
        score < search.closest_distance_sq && score < search.max_threshold_sq
    }

    /// `shapecastTraverse`.
    fn traverse(&self, nodes: &[Node], node_index: usize, search: &mut Search) -> bool {
        let node = &nodes[node_index];
        match node.kind {
            NodeKind::Leaf { offset, count } => self.intersects_range(offset, count, search),
            NodeKind::Inner { right, .. } => {
                let mut c1 = node_index + 1;
                let mut c2 = right;

                let mut score1 = box_score(&nodes[c1], search.point);
                let mut score2 = box_score(&nodes[c2], search.point);

                if score2 < score1 {
                    std::mem::swap(&mut c1, &mut c2);
                    std::mem::swap(&mut score1, &mut score2);
                }

                // Check box 1 intersection
                if Self::intersects_bounds(score1, search) && self.traverse(nodes, c1, search) {
                    return true;
                }

                // Check box 2 intersection, against the distance box 1 may have shrunk.
                if Self::intersects_bounds(score2, search) && self.traverse(nodes, c2, search) {
                    return true;
                }

                false
            }
        }
    }
}
