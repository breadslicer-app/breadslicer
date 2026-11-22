//! SoulCrafted (#235): whether a contact already rests on its model.
//!
//! Minis come assembled from parts (figure, base, weapons) that touch or
//! overlap, and the slicer prints their union. An overhang classified per
//! triangle, or an island found per shell, can lie on the material of
//! another part: nothing hangs there, and a support has no room. A contact
//! rests on its model when model material lies a little straight below it.
//!
//! Inside is told by the winding of the faces crossed going up (each face
//! that faces up is a way out, each that faces down a way in), not by their
//! parity: a point inside two parts that overlap crosses an even number of
//! faces, and is inside all the same.

use crate::bvh::{BvhMesh, Side};
use crate::geom::{Ray, Vec3};

/// How many layers of model material below a contact make it rest on it.
pub const RESTING_LAYERS: f64 = 2.0;

/// Whether the point `pos` lies inside the material of `mesh`: the faces
/// crossed going up from it, in two directions, leave more ways out than in.
pub fn inside_material(mesh: BvhMesh<'_>, pos: Vec3) -> bool {
    let mirrored = mesh.matrix_world.determinant() < 0.0;
    let winding = |direction: Vec3| {
        let mut hits = mesh.intersect_object(
            &Ray {
                origin: pos,
                direction,
            },
            Side::Double,
            0.0,
            f64::INFINITY,
            false,
        );
        // A ray through an edge meets both faces at one distance.
        hits.dedup_by(|a, b| (a.distance - b.distance).abs() < 1e-7);
        hits.iter()
            .map(|hit| {
                let out = hit
                    .face
                    .normal
                    .transform_direction(mesh.matrix_world)
                    .dot(direction);
                let out = if mirrored { -out } else { out };
                if out > 0.0 { 1 } else { -1 }
            })
            .sum::<i32>()
    };
    winding(Vec3::new(0.0, 0.0, 1.0)) > 0 && winding(Vec3::new(0.36, 0.48, 0.8).normalize()) > 0
}

/// Whether a contact at `pos` rests on the model: there is material
/// `below_mm` straight below it.
pub fn rests_on_model(mesh: BvhMesh<'_>, pos: Vec3, below_mm: f64) -> bool {
    inside_material(mesh, Vec3::new(pos.x, pos.y, pos.z - below_mm))
}
