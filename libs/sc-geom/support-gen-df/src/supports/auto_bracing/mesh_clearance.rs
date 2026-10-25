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
