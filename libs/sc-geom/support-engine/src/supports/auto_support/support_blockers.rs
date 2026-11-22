//! Surfaces painted to take no support: the check the placement passes make.

use std::collections::HashSet;

use crate::bvh::{BvhMesh, Side};
use crate::geom::Ray;
use crate::geom::vector3::Vec3;

/// Whether a contact at `point` lands on a face of `blocked` (the model's
/// faces painted to take no support). With the face known it is that face;
/// otherwise the face a ray up from 2 mm below the point hits first, the
/// underside a support would touch. Nothing painted: never.
pub fn is_support_blocked_contact(
    blocked: Option<&HashSet<usize>>,
    mesh: BvhMesh<'_>,
    side: Side,
    point: Vec3,
    known_face_index: Option<usize>,
) -> bool {
    let Some(faces) = blocked.filter(|f| !f.is_empty()) else {
        return false;
    };
    if let Some(face) = known_face_index {
        return faces.contains(&face);
    }
    let up = Ray {
        origin: Vec3::new(point.x, point.y, point.z - 2.0),
        direction: Vec3::new(0.0, 0.0, 1.0),
    };
    mesh.intersect_object(&up, side, 0.0, f64::INFINITY, false)
        .first()
        .is_some_and(|hit| faces.contains(&hit.face_index))
}
