//! DragonFruit `src/supports/autoBracing/{meshClearance,meshGeometryStore}.ts`:
//! whether a brace-thick line clears the model, cast as nine parallel rays.
//!
//! DragonFruit reads the model from a module-level store the scene manager
//! fills; here the caller hands the meshes in, keyed by model id.

use std::collections::HashMap;

use crate::supports::types::Vec3;
use crate::three::Matrix4;
use crate::three::Ray;
use crate::three::math_utils::js_max;
use crate::three::math_utils::js_min;
use crate::three_mesh_bvh::{BvhMesh, MeshBvh, Side};

/// One model as auto-bracing registers it: its BVH and transform. DragonFruit
/// raycasts it double-sided whatever the model's own material.
#[derive(Clone, Copy)]
pub struct BraceMesh<'a> {
    pub bvh: &'a MeshBvh,
    pub matrix: Matrix4,
}

impl BraceMesh<'_> {
    pub fn rays(&self) -> BvhMesh<'_> {
        BvhMesh {
            bvh: self.bvh,
            matrix_world: &self.matrix,
        }
    }
}

/// `getAllMeshEntriesForAutoBrace()`.
pub type BraceMeshes<'a> = HashMap<String, BraceMesh<'a>>;

/// `0.7071067811865476`, the same double.
const WHISKER_DIAGONAL: f64 = std::f64::consts::FRAC_1_SQRT_2;
const WHISKER_OFFSETS: [(f64, f64); 8] = [
    (1.0, 0.0),
    (-1.0, 0.0),
    (0.0, 1.0),
    (0.0, -1.0),
    (WHISKER_DIAGONAL, WHISKER_DIAGONAL),
    (-WHISKER_DIAGONAL, WHISKER_DIAGONAL),
    (WHISKER_DIAGONAL, -WHISKER_DIAGONAL),
    (-WHISKER_DIAGONAL, -WHISKER_DIAGONAL),
];
const WORLD_UP: Vec3 = Vec3::new(0.0, 1.0, 0.0);
const WORLD_RIGHT: Vec3 = Vec3::new(1.0, 0.0, 0.0);
/// Forward offset so an origin on the surface does not hit itself.
const RAY_ORIGIN_EPS_MM: f64 = 0.02;

/// How far apart the axis of a brace is read, in millimetres.
const AXIS_STEP_MM: f64 = 0.2;

/// True when nothing of the model comes nearer the axis from `pos_a` to `pos_b`
/// than a radius of `diameter_mm` (SoulCrafted, #445): what passes between the
/// rays of [`line_passes_mesh_clearance`], a spine thinner than their gap.
pub fn line_stays_off_mesh(
    pos_a: Vec3,
    pos_b: Vec3,
    mesh: Option<&BraceMesh<'_>>,
    diameter_mm: f64,
) -> bool {
    let Some(mesh) = mesh else {
        return true;
    };
    let radius = diameter_mm / 2.0;
    let into_model = mesh.matrix.invert();
    let steps = (pos_a.distance_to(pos_b) / AXIS_STEP_MM).ceil().max(1.0);
    (0..=steps as u32).all(|k| {
        let at = pos_a.lerp(pos_b, f64::from(k) / steps);
        mesh.bvh
            .closest_point_to_point(at.apply_matrix4(&into_model), 0.0, radius)
            .is_none_or(|hit| hit.distance >= radius)
    })
}

/// True when a brace of `diameter_mm` from `pos_a` to `pos_b` clears the
/// model: a centre ray plus eight whiskers at the brace radius, none hitting.
/// No mesh registered for the model passes, as in DragonFruit.
pub fn line_passes_mesh_clearance(
    pos_a: Vec3,
    pos_b: Vec3,
    mesh: Option<&BraceMesh<'_>>,
    diameter_mm: f64,
) -> bool {
    let radius = diameter_mm / 2.0;
    let Some(mesh) = mesh else {
        return true;
    };
    let direction = Vec3::new(pos_b.x - pos_a.x, pos_b.y - pos_a.y, pos_b.z - pos_a.z);
    let length = direction.length();
    if length < 0.1 {
        return true;
    }
    let direction = direction.multiply_scalar(1.0 / length);
    let eps = js_min(RAY_ORIGIN_EPS_MM, js_max(0.0, length * 0.1));
    let max_ray_distance = js_max(0.0, length - eps);
    let rays = mesh.rays();
    let cast = |ox: f64, oy: f64, oz: f64| {
        let origin = Vec3::new(
            pos_a.x + ox + direction.x * eps,
            pos_a.y + oy + direction.y * eps,
            pos_a.z + oz + direction.z * eps,
        );
        !rays
            .intersect_object(
                &Ray { origin, direction },
                Side::Double,
                0.0,
                max_ray_distance,
                false,
            )
            .is_empty()
    };

    if cast(0.0, 0.0, 0.0) {
        return false;
    }
    let arbitrary = if direction.dot(WORLD_UP).abs() > 0.9 {
        WORLD_RIGHT
    } else {
        WORLD_UP
    };
    let perp1 = direction.cross(arbitrary).normalize();
    let perp2 = direction.cross(perp1).normalize();
    for (u, v) in WHISKER_OFFSETS {
        let ox = (perp1.x * u + perp2.x * v) * radius;
        let oy = (perp1.y * u + perp2.y * v) * radius;
        let oz = (perp1.z * u + perp2.z * v) * radius;
        if cast(ox, oy, oz) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A spine ending `reach_mm` from the X axis, between two rays of a brace
    /// along that axis.
    fn spine(reach_mm: f32) -> Arc<MeshBvh> {
        let (sin, cos) = 22.5f32.to_radians().sin_cos();
        let radial = |r: f32, x: f32| [x, r * cos, r * sin];
        let tip = radial(reach_mm, 5.0);
        let (a, b) = (radial(3.0, 4.9), radial(3.0, 5.1));
        let c = [5.0, 3.0 * cos - 0.1 * sin, 3.0 * sin + 0.1 * cos];
        let corners = vec![tip, a, b, tip, b, c, tip, c, a, a, c, b];
        Arc::new(MeshBvh::new(corners, None))
    }

    fn brace(bvh: &MeshBvh) -> (bool, bool) {
        let mesh = BraceMesh {
            bvh,
            matrix: Matrix4::IDENTITY,
        };
        let (a, b) = (Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 0.0));
        (
            line_passes_mesh_clearance(a, b, Some(&mesh), 0.7),
            line_stays_off_mesh(a, b, Some(&mesh), 0.7),
        )
    }

    #[test]
    fn a_spine_between_the_rays_of_a_brace_is_seen_by_its_axis() {
        assert_eq!(brace(&spine(0.15)), (true, false));
    }

    #[test]
    fn a_spine_short_of_the_brace_leaves_it_alone() {
        assert_eq!(brace(&spine(0.5)), (true, true));
    }
}
