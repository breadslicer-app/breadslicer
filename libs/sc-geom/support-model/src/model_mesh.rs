//! The model as the builders ask it questions: its distance field (shafts,
//! cones and stumps against the model) and its rays (shaft and short bridge
//! collisions), together.

use crate::collision_avoidance::CollisionField;
use support_geom::bvh::BvhMesh;

#[derive(Clone, Copy, Debug)]
pub struct ModelMesh<'a> {
    /// The mesh's pooled distance field, matrix refreshed.
    pub field: &'a dyn CollisionField,
    /// The mesh's BVH under its world matrix, for ray casts.
    pub rays: BvhMesh<'a>,
}
