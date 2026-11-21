//! DragonFruit `src/supports/autoSupport/supportBlockers.ts`, the query the
//! placement passes make.
//!
//! Support blockers ("nogo" paint for supports) are a per-model set of
//! model-space triangle indices whose surface must never take a support
//! contact. The paint brush, its store, stroke history and hover state are the
//! GUI's; the passes are handed the blocked set.

use std::collections::HashSet;

use crate::three::Ray;
use crate::three::vector3::Vec3;
use crate::three_mesh_bvh::{BvhMesh, Side};

/// Whether a support contact at the given world point lands on blocked
/// surface. The known face short-circuits (grid lattice path); otherwise an
/// upward ray from just below the point resolves the contact face — the same
/// underside the generator would attach to. An empty mask is `false` with no
/// raycast. `side` is the mesh material's side, which the raycast honours.
pub fn is_support_blocked_contact(
    blocked: Option<&HashSet<usize>>,
    mesh: BvhMesh<'_>,
    side: Side,
    point: Vec3,
    known_face_index: Option<usize>,
) -> bool {
    let Some(tris) = blocked.filter(|t| !t.is_empty()) else {
        return false;
    };
    if let Some(face) = known_face_index {
        return tris.contains(&face);
    }
    let ray = Ray {
        origin: Vec3::new(point.x, point.y, point.z - 2.0),
        direction: Vec3::new(0.0, 0.0, 1.0),
    };
    mesh.intersect_object(&ray, side, 0.0, f64::INFINITY, false)
        .first()
        .is_some_and(|hit| tris.contains(&hit.face_index))
}
