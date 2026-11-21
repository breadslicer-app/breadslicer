//! three-mesh-bvh 0.8.3 `src/core/cast/{raycast,raycastFirst}.generated.js`,
//! `src/core/utils/intersectUtils.js` (`intersectRay`),
//! `src/utils/ThreeRayIntersectUtilities.js` (`intersectTri`) and the
//! `acceleratedRaycast` mesh override (`src/utils/ExtensionUtilities.js`),
//! with three.js r181 `Raycaster.intersectObject`'s distance sort.
//!
//! Hits carry what DragonFruit reads: distance, point, the face's vertex
//! indices and geometric normal, and the face index. Vertex-normal and uv
//! interpolation are not ported: no support code reads them.

use super::{MeshBvh, Node, NodeKind};
use crate::three::{Matrix4, Ray, Triangle, Vec3};

/// The material side a raycast tests against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    Front,
    Back,
    Double,
}

/// `intersection.face`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Face {
    pub a: u32,
    pub b: u32,
    pub c: u32,
    pub normal: Vec3,
}

/// A three.js raycast intersection, as three-mesh-bvh builds it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Intersection {
    pub distance: f64,
    pub point: Vec3,
    pub face: Face,
    /// The triangle, in the BVH's reordered order.
    pub face_index: usize,
}

/// `intersectRay`: does the ray cross a node's box within `[near, far]`?
fn intersect_ray(node: &Node, ray: &Ray, near: f64, far: f64) -> bool {
    let b = &node.bounds;
    let (mut tmin, mut tmax);
    let (tymin, tymax, tzmin, tzmax);

    let invdirx = 1.0 / ray.direction.x;
    let invdiry = 1.0 / ray.direction.y;
    let invdirz = 1.0 / ray.direction.z;

    let (ox, oy, oz) = (ray.origin.x, ray.origin.y, ray.origin.z);

    let (minx, maxx) = (f64::from(b[0]), f64::from(b[3]));
    let (miny, maxy) = (f64::from(b[1]), f64::from(b[4]));
    let (minz, maxz) = (f64::from(b[2]), f64::from(b[5]));

    if invdirx >= 0.0 {
        tmin = (minx - ox) * invdirx;
        tmax = (maxx - ox) * invdirx;
    } else {
        tmin = (maxx - ox) * invdirx;
        tmax = (minx - ox) * invdirx;
    }

    if invdiry >= 0.0 {
        tymin = (miny - oy) * invdiry;
        tymax = (maxy - oy) * invdiry;
    } else {
        tymin = (maxy - oy) * invdiry;
        tymax = (miny - oy) * invdiry;
    }

    if (tmin > tymax) || (tymin > tmax) {
        return false;
    }

    if tymin > tmin || tmin.is_nan() {
        tmin = tymin;
    }

    if tymax < tmax || tmax.is_nan() {
        tmax = tymax;
    }

    if invdirz >= 0.0 {
        tzmin = (minz - oz) * invdirz;
        tzmax = (maxz - oz) * invdirz;
    } else {
        tzmin = (maxz - oz) * invdirz;
        tzmax = (minz - oz) * invdirz;
    }

    if (tmin > tzmax) || (tzmin > tmax) {
        return false;
    }

    if tzmin > tmin || tmin.is_nan() {
        tmin = tzmin;
    }

    if tzmax < tmax || tmax.is_nan() {
        tmax = tzmax;
    }

    //return point closest to the ray (positive side)
    tmin <= far && tmax >= near
}

impl MeshBvh {
    /// `intersectTri` (`checkBufferGeometryIntersection` / `checkIntersection`).
    fn intersect_tri(
        &self,
        side: Side,
        ray: &Ray,
        tri: usize,
        near: f64,
        far: f64,
    ) -> Option<Intersection> {
        let [a, b, c] = self.face(tri);
        let (va, vb, vc) = (self.vertex(a), self.vertex(b), self.vertex(c));

        let point = match side {
            Side::Back => ray.intersect_triangle(vc, vb, va, true),
            _ => ray.intersect_triangle(va, vb, vc, side != Side::Double),
        }?;

        let distance = ray.origin.distance_to(point);
        if distance < near || distance > far {
            return None;
        }

        Some(Intersection {
            distance,
            point,
            face: Face {
                a,
                b,
                c,
                normal: Triangle::get_normal(va, vb, vc),
            },
            face_index: tri,
        })
    }

    fn leaf_range(node: &Node) -> Option<std::ops::Range<usize>> {
        match node.kind {
            NodeKind::Leaf { offset, count } => {
                Some(offset as usize..offset as usize + count as usize)
            }
            NodeKind::Inner { .. } => None,
        }
    }

    /// `_raycast`: every hit, in traversal order.
    #[allow(clippy::too_many_arguments)]
    fn raycast_node(
        &self,
        nodes: &[Node],
        index: usize,
        side: Side,
        ray: &Ray,
        near: f64,
        far: f64,
        out: &mut Vec<Intersection>,
    ) {
        let node = &nodes[index];
        if let Some(range) = Self::leaf_range(node) {
            for tri in range {
                if let Some(hit) = self.intersect_tri(side, ray, tri, near, far) {
                    out.push(hit);
                }
            }
            return;
        }
        let NodeKind::Inner { right, .. } = node.kind else {
            unreachable!()
        };
        let left = index + 1;
        if intersect_ray(&nodes[left], ray, near, far) {
            self.raycast_node(nodes, left, side, ray, near, far, out);
        }
        if intersect_ray(&nodes[right], ray, near, far) {
            self.raycast_node(nodes, right, side, ray, near, far, out);
        }
    }

    /// `bvh.raycast(ray, side, near, far)`: every hit, in traversal order
    /// (local space, local distances).
    pub fn raycast(&self, ray: &Ray, side: Side, near: f64, far: f64) -> Vec<Intersection> {
        let mut out = Vec::new();
        for root in &self.roots {
            self.raycast_node(root, 0, side, ray, near, far, &mut out);
        }
        out
    }

    /// `_raycastFirst`.
    fn raycast_first_node(
        &self,
        nodes: &[Node],
        index: usize,
        side: Side,
        ray: &Ray,
        near: f64,
        far: f64,
    ) -> Option<Intersection> {
        let node = &nodes[index];
        if let Some(range) = Self::leaf_range(node) {
            // intersectClosestTri
            let mut dist = f64::INFINITY;
            let mut res = None;
            for tri in range {
                if let Some(hit) = self.intersect_tri(side, ray, tri, near, far)
                    && hit.distance < dist
                {
                    dist = hit.distance;
                    res = Some(hit);
                }
            }
            return res;
        }

        let NodeKind::Inner { right, split_axis } = node.kind else {
            unreachable!()
        };
        // consider the position of the split plane with respect to the oncoming ray; whichever direction
        // the ray is coming from, look for an intersection among that side of the tree first
        let axis = split_axis as usize;
        let ray_dir = [ray.direction.x, ray.direction.y, ray.direction.z][axis];
        let left_to_right = ray_dir >= 0.0;

        // c1 is the child to check first
        let (c1, c2) = if left_to_right {
            (index + 1, right)
        } else {
            (right, index + 1)
        };

        let c1_result = if intersect_ray(&nodes[c1], ray, near, far) {
            self.raycast_first_node(nodes, c1, side, ray, near, far)
        } else {
            None
        };

        // if we got an intersection in the first node and it's closer than the second node's bounding
        // box, we don't need to consider the second node because it couldn't possibly be a better result
        if let Some(hit) = &c1_result {
            // check if the point is within the second bounds
            // "point" is in the local frame of the bvh
            let point = [hit.point.x, hit.point.y, hit.point.z][axis];
            let is_outside = if left_to_right {
                point <= f64::from(nodes[c2].bounds[axis]) // min bounding data
            } else {
                point >= f64::from(nodes[c2].bounds[axis + 3]) // max bounding data
            };

            if is_outside {
                return c1_result;
            }
        }

        // either there was no intersection in the first node, or there could still be a closer
        // intersection in the second, so check the second node and then take the better of the two
        let c2_result = if intersect_ray(&nodes[c2], ray, near, far) {
            self.raycast_first_node(nodes, c2, side, ray, near, far)
        } else {
            None
        };

        match (c1_result, c2_result) {
            (Some(a), Some(b)) => Some(if a.distance <= b.distance { a } else { b }),
            (a, b) => a.or(b),
        }
    }

    /// `bvh.raycastFirst(ray, side, near, far)` (local space, local distance).
    pub fn raycast_first(
        &self,
        ray: &Ray,
        side: Side,
        near: f64,
        far: f64,
    ) -> Option<Intersection> {
        let mut closest: Option<Intersection> = None;
        for root in &self.roots {
            let result = self.raycast_first_node(root, 0, side, ray, near, far);
            if let Some(result) = result
                && closest.is_none_or(|c| result.distance < c.distance)
            {
                closest = Some(result);
            }
        }
        closest
    }
}

/// A `THREE.Mesh` carrying a BVH: its geometry's tree and its world matrix.
#[derive(Debug, Clone, Copy)]
pub struct BvhMesh<'a> {
    pub bvh: &'a MeshBvh,
    pub matrix_world: &'a Matrix4,
}

impl BvhMesh<'_> {
    /// `raycaster.intersectObject(mesh, false)` with `acceleratedRaycast` and
    /// the given material side: every hit in world space, sorted by distance
    /// (a stable sort, as `Array.prototype.sort`). `raycaster.firstHitOnly` is
    /// three-mesh-bvh's switch to `raycastFirst`.
    pub fn intersect_object(
        &self,
        ray: &Ray,
        side: Side,
        near: f64,
        far: f64,
        first_hit_only: bool,
    ) -> Vec<Intersection> {
        let tmp_inverse_matrix = self.matrix_world.invert();
        let local_ray = ray.apply_matrix4(&tmp_inverse_matrix);

        let world_scale = Vec3::from_matrix_scale(self.matrix_world);
        let direction = local_ray.direction.multiply(world_scale);

        let scale_factor = direction.length();
        let local_near = near / scale_factor;
        let local_far = far / scale_factor;

        // convertRaycastIntersect
        let to_world = |mut hit: Intersection| {
            hit.point = hit.point.apply_matrix4(self.matrix_world);
            hit.distance = hit.point.distance_to(ray.origin);
            hit
        };

        let mut intersects: Vec<Intersection> = if first_hit_only {
            self.bvh
                .raycast_first(&local_ray, side, local_near, local_far)
                .map(to_world)
                .into_iter()
                .collect()
        } else {
            self.bvh
                .raycast(&local_ray, side, local_near, local_far)
                .into_iter()
                .map(to_world)
                .collect()
        };

        // ascSort: a.distance - b.distance
        intersects.sort_by(|a, b| {
            (a.distance - b.distance)
                .partial_cmp(&0.0)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        intersects
    }
}
