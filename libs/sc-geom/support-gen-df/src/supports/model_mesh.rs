//! The model mesh as the builders ask it questions.
//!
//! DragonFruit hands builders a `THREE.Mesh` and each check picks its own view
//! of it: the pooled distance field (`isShaftBlocked`, the cone and stump
//! gates) or the BVH raycaster (`checkShaftCollision`, `checkShortBridgeCollision`).
//! Here both views travel together, and each builder reads the one DragonFruit
//! reads.

use crate::supports::placement_logic::collision_avoidance::CollisionField;
use crate::three_mesh_bvh::BvhMesh;

#[derive(Clone, Copy)]
pub struct ModelMesh<'a> {
    /// The mesh's pooled distance field, matrix refreshed.
    pub field: &'a dyn CollisionField,
    /// The mesh's BVH under its world matrix, for ray casts.
    pub rays: BvhMesh<'a>,
}
