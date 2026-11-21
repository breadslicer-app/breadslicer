//! SoulCrafted: every triangle that comes within a distance of a point, the
//! surface a ball there touches (#491). Not in three-mesh-bvh: a shapecast
//! that walks every box the ball reaches, in tree order.

use super::{MeshBvh, Node, NodeKind};
use crate::three::Vec3;

#[inline]
fn box_distance_sq(bounds: &[f32; 6], point: Vec3) -> f64 {
    let min = Vec3::new(
        f64::from(bounds[0]),
        f64::from(bounds[1]),
        f64::from(bounds[2]),
    );
    let max = Vec3::new(
        f64::from(bounds[3]),
        f64::from(bounds[4]),
        f64::from(bounds[5]),
    );
    point.clamp(min, max).distance_to_squared(point)
}

impl MeshBvh {
    /// Calls `visit(face, distance)` for each triangle whose closest point
    /// lies within `radius` of `point` (face index in the BVH's reordered
    /// order), in tree order.
    pub fn faces_within(&self, point: Vec3, radius: f64, mut visit: impl FnMut(usize, f64)) {
        let reach_sq = radius * radius;
        for root in &self.roots {
            self.faces_within_node(root, 0, point, reach_sq, &mut visit);
        }
    }

    fn faces_within_node(
        &self,
        nodes: &[Node],
        node_index: usize,
        point: Vec3,
        reach_sq: f64,
        visit: &mut impl FnMut(usize, f64),
    ) {
        let node = &nodes[node_index];
        if box_distance_sq(&node.bounds, point) > reach_sq {
            return;
        }
        match node.kind {
            NodeKind::Leaf { offset, count } => {
                let start = offset as usize;
                for tri in start..start + count as usize {
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
                self.faces_within_node(nodes, node_index + 1, point, reach_sq, visit);
                self.faces_within_node(nodes, right, point, reach_sq, visit);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bumpy sheet of small triangles, a few millimetres across.
    fn sheet() -> MeshBvh {
        let n = 24;
        let h = |i: usize, j: usize| ((i * 7 + j * 13) % 5) as f32 * 0.03;
        let mut positions = Vec::new();
        for i in 0..n {
            for j in 0..n {
                let p = |a: usize, b: usize| [a as f32 * 0.2, b as f32 * 0.2, h(a, b)];
                for t in [
                    [p(i, j), p(i + 1, j), p(i + 1, j + 1)],
                    [p(i, j), p(i + 1, j + 1), p(i, j + 1)],
                ] {
                    positions.extend(t);
                }
            }
        }
        MeshBvh::new(positions, None)
    }

    #[test]
    fn the_faces_within_a_ball_are_every_face_it_touches() {
        let bvh = sheet();
        for (point, radius) in [
            (Vec3::new(2.3, 2.1, 0.05), 0.4),
            (Vec3::new(0.05, 4.7, -0.2), 0.6),
            (Vec3::new(3.0, 3.0, 0.5), 0.3),
            (Vec3::new(1.0, 1.0, 0.1), 0.0),
        ] {
            let mut found = Vec::new();
            bvh.faces_within(point, radius, |face, d| found.push((face, d)));
            found.sort_by_key(|f| f.0);
            let all: Vec<(usize, f64)> = (0..bvh.triangle_count())
                .filter_map(|f| {
                    let d = point.distance_to(bvh.triangle(f).closest_point_to_point(point));
                    (d <= radius).then_some((f, d))
                })
                .collect();
            assert_eq!(found, all, "{point:?} r {radius}");
        }
        let mut none = 0;
        bvh.faces_within(Vec3::new(2.0, 2.0, 3.0), 0.5, |_, _| none += 1);
        assert_eq!(none, 0);
    }
}
