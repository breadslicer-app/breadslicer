//! The surface normal at a ray hit, smoothed from the vertex normals.

use crate::bvh::Intersection;
use crate::geom::{Matrix4, Vec3};

/// The weights of `a`, `b` and `c` that make `point` (barycentric
/// coordinates), or zero for a triangle with no area.
fn weights(point: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let (ac, ab, ap) = (c.sub(a), b.sub(a), point.sub(a));
    let (cc, cb, cp, bb, bp) = (ac.dot(ac), ac.dot(ab), ac.dot(ap), ab.dot(ab), ab.dot(ap));
    let area = cc * bb - cb * cb;
    if area == 0.0 {
        return Vec3::ZERO;
    }
    let inv = 1.0 / area;
    let toward_c = (bb * cp - cb * bp) * inv;
    let toward_b = (cc * bp - cb * cp) * inv;
    Vec3::new(1.0 - toward_c - toward_b, toward_b, toward_c)
}

/// The normal at `hit` in world space, blended from the vertex normals of
/// its face (smoother on curved surfaces than the face normal); without
/// vertex normals, the face's own normal as the hit carries it.
pub fn calculate_smoothed_normal(
    hit: &Intersection,
    positions: &[[f32; 3]],
    normals: Option<&[[f32; 3]]>,
    matrix_world: &Matrix4,
) -> Vec3 {
    let Some(normals) = normals else {
        return hit.face.normal;
    };
    let read = |attr: &[[f32; 3]], i: u32| {
        let [x, y, z] = attr[i as usize];
        Vec3::new(f64::from(x), f64::from(y), f64::from(z))
    };
    let corners = [hit.face.a, hit.face.b, hit.face.c];
    let [pa, pb, pc] = corners.map(|i| read(positions, i).apply_matrix4(matrix_world));
    let [na, nb, nc] = corners.map(|i| read(normals, i));
    let w = weights(hit.point, pa, pb, pc);
    Vec3::ZERO
        .add_scaled_vector(na, w.x)
        .add_scaled_vector(nb, w.y)
        .add_scaled_vector(nc, w.z)
        .normalize()
        .transform_direction(matrix_world)
}
