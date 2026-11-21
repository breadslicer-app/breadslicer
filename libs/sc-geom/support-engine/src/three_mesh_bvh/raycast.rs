/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

//! Rays against the tree: every hit, or the nearest.

use super::{MeshBvh, NodeKind};
use crate::three::{Matrix4, Ray, Triangle, Vec3};

/// Which faces a ray hits: the ones it meets from their front, from their
/// back, or both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    Front,
    Back,
    Double,
}

/// The face a hit lies on: its vertices and its geometric normal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Face {
    pub a: u32,
    pub b: u32,
    pub c: u32,
    pub normal: Vec3,
}

/// Where a ray meets a face.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Intersection {
    pub distance: f64,
    pub point: Vec3,
    pub face: Face,
    /// The triangle's number in the geometry.
    pub face_index: usize,
}

/// Whether the ray crosses the box within `[near, far]` of its origin.
fn crosses_box(bounds: &[f32; 6], ray: &Ray, near: f64, far: f64) -> bool {
    let origin = [ray.origin.x, ray.origin.y, ray.origin.z];
    let direction = [ray.direction.x, ray.direction.y, ray.direction.z];
    let (mut enter, mut leave) = (f64::NEG_INFINITY, f64::INFINITY);
    for k in 0..3 {
        let inv = 1.0 / direction[k];
        let (lo, hi) = (f64::from(bounds[k]), f64::from(bounds[k + 3]));
        let (mut t0, mut t1) = ((lo - origin[k]) * inv, (hi - origin[k]) * inv);
        if inv < 0.0 {
            std::mem::swap(&mut t0, &mut t1);
        }
        // A ray along a face of the box gives NaN: it does not narrow.
        if t0 > enter {
            enter = t0;
        }
        if t1 < leave {
            leave = t1;
        }
        if enter > leave {
            return false;
        }
    }
    enter <= far && leave >= near
}

impl MeshBvh {
    /// The hit of `ray` on triangle `tri`, within `[near, far]`.
    fn hit(&self, side: Side, ray: &Ray, tri: usize, near: f64, far: f64) -> Option<Intersection> {
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

    fn all_hits(
        &self,
        index: usize,
        side: Side,
        ray: &Ray,
        near: f64,
        far: f64,
        out: &mut Vec<Intersection>,
    ) {
        match self.nodes[index].kind {
            NodeKind::Leaf { offset, count } => {
                out.extend(
                    self.leaf(offset, count)
                        .filter_map(|t| self.hit(side, ray, t, near, far)),
                );
            }
            NodeKind::Inner { right, .. } => {
                for child in [index + 1, right] {
                    if crosses_box(&self.nodes[child].bounds, ray, near, far) {
                        self.all_hits(child, side, ray, near, far, out);
                    }
                }
            }
        }
    }

    /// Every hit of `ray` within `[near, far]`, in no set order.
    pub fn raycast(&self, ray: &Ray, side: Side, near: f64, far: f64) -> Vec<Intersection> {
        let mut out = Vec::new();
        if let Some(root) = self.nodes.first()
            && crosses_box(&root.bounds, ray, near, far)
        {
            self.all_hits(0, side, ray, near, far, &mut out);
        }
        out
    }

    /// The nearest hit below node `index`, no farther than `far`.
    fn first_hit(
        &self,
        index: usize,
        side: Side,
        ray: &Ray,
        near: f64,
        far: f64,
    ) -> Option<Intersection> {
        let node = &self.nodes[index];
        match node.kind {
            NodeKind::Leaf { offset, count } => self
                .leaf(offset, count)
                .filter_map(|t| self.hit(side, ray, t, near, far))
                .min_by(|a, b| a.distance.total_cmp(&b.distance)),
            NodeKind::Inner { right, split_axis } => {
                // The child the ray meets first along the split axis first.
                let along =
                    [ray.direction.x, ray.direction.y, ray.direction.z][split_axis as usize];
                let (first, second) = if along >= 0.0 {
                    (index + 1, right)
                } else {
                    (right, index + 1)
                };
                let mut best: Option<Intersection> = None;
                for child in [first, second] {
                    let reach = best.map_or(far, |b| b.distance);
                    if crosses_box(&self.nodes[child].bounds, ray, near, reach)
                        && let Some(hit) = self.first_hit(child, side, ray, near, reach)
                        && best.is_none_or(|b| hit.distance < b.distance)
                    {
                        best = Some(hit);
                    }
                }
                best
            }
        }
    }

    /// The nearest hit of `ray` within `[near, far]`.
    pub fn raycast_first(
        &self,
        ray: &Ray,
        side: Side,
        near: f64,
        far: f64,
    ) -> Option<Intersection> {
        let root = self.nodes.first()?;
        if !crosses_box(&root.bounds, ray, near, far) {
            return None;
        }
        self.first_hit(0, side, ray, near, far)
    }
}

/// A mesh placed in the world: its tree and its world matrix.
#[derive(Debug, Clone, Copy)]
pub struct BvhMesh<'a> {
    pub bvh: &'a MeshBvh,
    pub matrix_world: &'a Matrix4,
}

impl BvhMesh<'_> {
    /// The hits of a world-space ray, in world space, nearest first; only the
    /// nearest with `first_hit_only`. `near` and `far` are world distances.
    pub fn intersect_object(
        &self,
        ray: &Ray,
        side: Side,
        near: f64,
        far: f64,
        first_hit_only: bool,
    ) -> Vec<Intersection> {
        let local_ray = ray.apply_matrix4(&self.matrix_world.invert());
        let scale = local_ray
            .direction
            .multiply(Vec3::from_matrix_scale(self.matrix_world))
            .length();
        let (local_near, local_far) = (near / scale, far / scale);
        let to_world = |mut hit: Intersection| {
            hit.point = hit.point.apply_matrix4(self.matrix_world);
            hit.distance = hit.point.distance_to(ray.origin);
            hit
        };
        let mut hits: Vec<Intersection> = if first_hit_only {
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
        // Equal distances keep the order of the faces in the geometry.
        hits.sort_by(|a, b| {
            a.distance
                .total_cmp(&b.distance)
                .then(a.face_index.cmp(&b.face_index))
        });
        hits
    }
}
