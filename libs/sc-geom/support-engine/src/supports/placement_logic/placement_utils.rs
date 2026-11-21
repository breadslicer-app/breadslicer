//! DragonFruit `src/supports/PlacementLogic/PlacementUtils.ts`: the smoothed
//! contact normal. (`findClosestMeshToPoint` picks between scene meshes and is
//! the GUI's.)

use crate::three::{Matrix4, Vec3};
use crate::three_mesh_bvh::Intersection;

/// three's `Triangle.getBarycoord`: `(1 - u - v, v, u)`, or zero for a
/// degenerate triangle.
fn barycoord(point: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let v0 = c.sub(a);
    let v1 = b.sub(a);
    let v2 = point.sub(a);
    let dot00 = v0.dot(v0);
    let dot01 = v0.dot(v1);
    let dot02 = v0.dot(v2);
    let dot11 = v1.dot(v1);
    let dot12 = v1.dot(v2);
    let denom = dot00 * dot11 - dot01 * dot01;
    if denom == 0.0 {
        return Vec3::ZERO;
    }
    let inv_denom = 1.0 / denom;
    let u = (dot11 * dot02 - dot01 * dot12) * inv_denom;
    let v = (dot00 * dot12 - dot01 * dot02) * inv_denom;
    Vec3::new(1.0 - u - v, v, u)
}

/// The interpolated vertex normal at a hit, in world space: smoother on
/// curved surfaces than the raw face normal. Without vertex normals it is the
/// (local) face normal, as DragonFruit returns it.
///
/// `positions` and `normals` are the geometry's attributes; the hit's face
/// names their vertices.
pub fn calculate_smoothed_normal(
    hit: &Intersection,
    positions: &[[f32; 3]],
    normals: Option<&[[f32; 3]]>,
    matrix_world: &Matrix4,
) -> Vec3 {
    let Some(normals) = normals else {
        return hit.face.normal;
    };
    let v = |attr: &[[f32; 3]], i: u32| {
        let p = attr[i as usize];
        Vec3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2]))
    };
    let (a, b, c) = (hit.face.a, hit.face.b, hit.face.c);
    let n_a = v(normals, a);
    let n_b = v(normals, b);
    let n_c = v(normals, c);
    let p_a = v(positions, a).apply_matrix4(matrix_world);
    let p_b = v(positions, b).apply_matrix4(matrix_world);
    let p_c = v(positions, c).apply_matrix4(matrix_world);

    let bary = barycoord(hit.point, p_a, p_b, p_c);
    Vec3::ZERO
        .add_scaled_vector(n_a, bary.x)
        .add_scaled_vector(n_b, bary.y)
        .add_scaled_vector(n_c, bary.z)
        .normalize()
        .transform_direction(matrix_world)
}
