//! DragonFruit `geometry/generateChamferedBeam.ts`: one line-raft beam as a
//! chamfered prism from `start` to `end`.

use crate::three::buffer_geometry::BufferGeometry;
use crate::three::math_utils::{js_max, js_min, js_tan};
use crate::three::{Matrix4, Quaternion, Vec3};

pub fn generate_chamfered_beam(
    start: Vec3,
    end: Vec3,
    width_mm: f64,
    height_mm: f64,
    chamfer_angle_deg: f64,
) -> BufferGeometry {
    let width = js_max(0.01, width_mm);
    let height = js_max(0.01, height_mm);
    let angle_deg = js_min(90.0, js_max(45.0, chamfer_angle_deg));
    let dir = Vec3::sub_vectors(end, start);
    let length = dir.length();
    if !length.is_finite() || length < 0.001 {
        return BufferGeometry::default();
    }
    let inset = height * js_tan((std::f64::consts::PI / 180.0) * (90.0 - angle_deg));
    let top_half_w = width / 2.0;
    let bottom_half_w = js_max(0.001, top_half_w - inset);
    let (x0, x1) = (0.0, length);
    #[rustfmt::skip]
    let positions = [
        x0, -bottom_half_w, 0.0,
        x0, bottom_half_w, 0.0,
        x1, bottom_half_w, 0.0,
        x1, -bottom_half_w, 0.0,
        x0, -top_half_w, height,
        x0, top_half_w, height,
        x1, top_half_w, height,
        x1, -top_half_w, height,
    ];
    #[rustfmt::skip]
    let indices: Vec<u32> = vec![
        0, 2, 1, 0, 3, 2,
        4, 5, 6, 4, 6, 7,
        0, 4, 7, 0, 7, 3,
        1, 6, 2, 1, 5, 6,
        0, 1, 5, 0, 5, 4,
        3, 6, 7, 3, 2, 6,
    ];
    let mut geom = BufferGeometry::default();
    geom.set_position(&positions);
    geom.index = Some(indices);
    geom.compute_vertex_normals();
    if !positions.iter().all(|v| v.is_finite()) {
        return BufferGeometry::default();
    }
    let q = Quaternion::set_from_unit_vectors(Vec3::new(1.0, 0.0, 0.0), dir.normalize());
    let m = Matrix4::make_rotation_from_quaternion(q).set_position(start);
    geom.apply_matrix4(&m);
    geom
}
