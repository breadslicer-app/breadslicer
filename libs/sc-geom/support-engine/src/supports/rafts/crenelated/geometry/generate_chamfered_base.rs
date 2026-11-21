//! DragonFruit `geometry/generateChamferedBase.ts`: the raft plate for one
//! convex footprint, top at `thickness`, bottom inset by the chamfer.

use super::super::raft_types::RaftSettings;
use super::small::inset_convex_polygon;
use crate::three::Vector2;
use crate::three::buffer_geometry::BufferGeometry;
use crate::three::math_utils::{js_max, js_min, js_tan};

pub fn generate_chamfered_base(profile: &[Vector2], settings: &RaftSettings) -> BufferGeometry {
    if profile.len() < 3 {
        return BufferGeometry::default();
    }
    let thickness = js_max(0.0, settings.thickness);
    if thickness == 0.0 {
        return BufferGeometry::default();
    }
    let angle_deg = js_min(90.0, js_max(45.0, settings.chamfer_angle));
    let inset = thickness * js_tan((std::f64::consts::PI / 180.0) * (90.0 - angle_deg));
    let bottom_profile = inset_convex_polygon(profile, inset);

    let (top_z, bottom_z) = (thickness, 0.0);
    let n = profile.len();
    let mut positions: Vec<f64> = Vec::with_capacity(n * 6);
    let mut indices: Vec<u32> = Vec::new();
    for p in profile {
        positions.extend([p.x, p.y, top_z]);
    }
    for p in bottom_profile.iter().take(n) {
        positions.extend([p.x, p.y, bottom_z]);
    }
    let n32 = n as u32;
    for i in 0..n32 {
        let i_next = (i + 1) % n32;
        let (a, b, c, d) = (i, i_next, n32 + i_next, n32 + i);
        indices.extend([a, c, b]);
        indices.extend([a, d, c]);
    }
    for i in 1..n32.saturating_sub(1) {
        indices.extend([0, i, i + 1]);
    }
    for i in 1..n32.saturating_sub(1) {
        indices.extend([n32, n32 + i + 1, n32 + i]);
    }
    if !positions.iter().all(|v| v.is_finite()) {
        return BufferGeometry::default();
    }
    let mut geom = BufferGeometry::default();
    geom.set_position(&positions);
    geom.index = Some(indices);
    geom.compute_vertex_normals();
    geom
}
