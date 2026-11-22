//! Whether a brace keeps clear of the model. Two tests, both needed: a
//! bundle of nine parallel rays (the axis and eight around it at the brace
//! radius) catches the model's surface crossing the brace, and a walk along
//! the axis catches a thin spike of the model reaching in between the rays.
//!
//! The caller registers the meshes, keyed by model id; a model without one
//! never blocks a brace.

use std::collections::HashMap;

use crate::types::Vec3;
use support_geom::bvh::{BvhMesh, MeshBvh, Side};
use support_geom::geom::Matrix4;
use support_geom::geom::Ray;
use support_geom::geom::math_utils::{js_max, js_min};

/// A model as bracing tests against it: its BVH and its world transform.
/// Rays hit both faces of a triangle, whatever the model's material.
#[derive(Clone, Copy, Debug)]
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

/// The meshes a bracing run tests against, by model id.
pub type BraceMeshes<'a> = HashMap<String, BraceMesh<'a>>;

/// Where the eight outer rays sit around the axis, as (u, v) on a unit
/// circle across it: the four sides, then the four diagonals.
const RING: [(f64, f64); 8] = {
    const D: f64 = std::f64::consts::FRAC_1_SQRT_2;
    [
        (1.0, 0.0),
        (-1.0, 0.0),
        (0.0, 1.0),
        (0.0, -1.0),
        (D, D),
        (-D, D),
        (D, -D),
        (-D, -D),
    ]
};

/// Rays start this far along the brace, so an end lying on the surface
/// does not hit the surface it lies on.
const START_INSET_MM: f64 = 0.02;

/// Braces shorter than this are not tested.
const SHORTEST_TESTED_MM: f64 = 0.1;

/// Spacing of the points the axis walk tests.
const AXIS_STEP_MM: f64 = 0.2;

/// True when no point of the model comes within the brace radius of the
/// axis from `pos_a` to `pos_b` (#445), tested every [`AXIS_STEP_MM`] along
/// it, both ends included.
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
    let world_to_model = mesh.matrix.invert();
    let intervals = (pos_a.distance_to(pos_b) / AXIS_STEP_MM).ceil().max(1.0);
    (0..=intervals as u32).all(|k| {
        let on_axis = pos_a.lerp(pos_b, f64::from(k) / intervals);
        let nearest =
            mesh.bvh
                .closest_point_to_point(on_axis.apply_matrix4(&world_to_model), 0.0, radius);
        nearest.is_none_or(|hit| hit.distance >= radius)
    })
}

/// True when none of the nine rays of a brace of `diameter_mm` from `pos_a`
/// to `pos_b` meets the model before reaching the far end.
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
    let run = Vec3::new(pos_b.x - pos_a.x, pos_b.y - pos_a.y, pos_b.z - pos_a.z);
    let length = run.length();
    if length < SHORTEST_TESTED_MM {
        return true;
    }
    let direction = run.multiply_scalar(1.0 / length);
    let inset = js_min(START_INSET_MM, js_max(0.0, length * 0.1));
    let reach = js_max(0.0, length - inset);
    let target = mesh.rays();
    let blocked = |offset: Vec3| {
        let origin = Vec3::new(
            pos_a.x + offset.x + direction.x * inset,
            pos_a.y + offset.y + direction.y * inset,
            pos_a.z + offset.z + direction.z * inset,
        );
        let ray = Ray { origin, direction };
        !target
            .intersect_object(&ray, Side::Double, 0.0, reach, false)
            .is_empty()
    };
    if blocked(Vec3::new(0.0, 0.0, 0.0)) {
        return false;
    }
    // Two unit vectors square to the brace and to each other.
    let helper = if direction.dot(Vec3::new(0.0, 1.0, 0.0)).abs() > 0.9 {
        Vec3::new(1.0, 0.0, 0.0)
    } else {
        Vec3::new(0.0, 1.0, 0.0)
    };
    let across = direction.cross(helper).normalize();
    let across2 = direction.cross(across).normalize();
    !RING.iter().any(|&(u, v)| {
        blocked(Vec3::new(
            (across.x * u + across2.x * v) * radius,
            (across.y * u + across2.y * v) * radius,
            (across.z * u + across2.z * v) * radius,
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A thin spike of model whose tip is `tip_mm` from the X axis, aimed
    /// between two of the outer rays of a brace laid along that axis.
    fn spike(tip_mm: f32) -> Arc<MeshBvh> {
        let (sin, cos) = 22.5f32.to_radians().sin_cos();
        let at = |r: f32, x: f32| [x, r * cos, r * sin];
        let tip = at(tip_mm, 5.0);
        let (a, b) = (at(3.0, 4.9), at(3.0, 5.1));
        let c = [5.0, 3.0 * cos - 0.1 * sin, 3.0 * sin + 0.1 * cos];
        let corners = vec![tip, a, b, tip, b, c, tip, c, a, a, c, b];
        Arc::new(MeshBvh::new(corners, None))
    }

    /// (rays clear, axis clear) for a 0.7 mm brace from the origin to X = 10.
    fn tests_of(bvh: &MeshBvh) -> (bool, bool) {
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
    fn a_spike_between_the_rays_is_caught_by_the_axis_walk() {
        assert_eq!(tests_of(&spike(0.15)), (true, false));
    }

    #[test]
    fn a_spike_outside_the_radius_blocks_nothing() {
        assert_eq!(tests_of(&spike(0.5)), (true, true));
    }
}
