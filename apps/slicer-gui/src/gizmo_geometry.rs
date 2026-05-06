/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Procedural geometry for the gizmo handles, all in **handle-local
//! space** — pivot at origin, primary axis along +X. The renderer
//! draws each buffer three times, rotating onto +X / +Y / +Z; per-axis
//! tint uniforms colour each draw.
//!
//! Two topologies are produced:
//!
//! - `LineList` — 1-pixel handle lines:
//!   - [`build_axis_shaft_line_geometry`]: pivot → cone base, one
//!     segment.
//!   - [`build_rotate_arc_line_geometry`]: polyline approximating the
//!     90° arc around +X.
//!
//! - `TriangleList` — solid pieces that act as click targets:
//!   - [`build_axis_cone_tip_geometry`]: 12-sided cone capping the
//!     shaft (translate handle).
//!   - [`build_axis_scale_cube_geometry`]: axis-aligned cube at the
//!     shaft midpoint (per-axis scale handle).
//!   - [`build_rotate_cones_geometry`]: two-cone spindle at the arc
//!     midpoint (rotate-drag handle).

use bytemuck::{Pod, Zeroable};
use glam::Vec3;

use crate::gizmo::dimensions::{
    AXIS_SCALE_CUBE_PIXELS, HANDLE_PIXEL_LENGTH, MIDPOINT_FRACTION, ROTATE_ARC_RADIUS_FRACTION,
    ROTATE_CONE_LENGTH, ROTATE_CONE_RADIUS,
};

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
pub struct GizmoVertex {
    pub position: [f32; 3],
}

/// Handle-local x-coord at which the cone base sits. Shaft occupies
/// `[0.0, 1.0]`; cone occupies `[1.0, 1.18]`. The hit-test in
/// `gizmo::hit_test` extends the cylinder to 1.25 × shaft length so
/// a click anywhere up to and slightly past the cone tip registers
/// as a shaft hit — no separate ray-vs-cone path needed.
const SHAFT_LENGTH: f32 = 1.0;
const CONE_BASE_X: f32 = SHAFT_LENGTH;
const CONE_TIP_X: f32 = 1.18;
const CONE_BASE_RADIUS: f32 = 0.07;

const CONE_SIDES: u32 = 12;

/// Side length, in handle-local space, of the per-axis midpoint scale
/// cube. The shaft is 1.0 unit long in handle-local space and
/// `HANDLE_PIXEL_LENGTH` pixels long on screen, so the world-to-pixel
/// ratio is `1.0 / HANDLE_PIXEL_LENGTH`; multiplying by the cube's
/// screen-pixel size gives its handle-local size.
const MIDPOINT_CUBE_SIDE_LOCAL: f32 = AXIS_SCALE_CUBE_PIXELS / HANDLE_PIXEL_LENGTH;

/// Major (circle) radius of the rotate arc in handle-local space.
/// Already a fraction of the shaft length, which is 1.0 in this
/// space, so the constant maps 1:1.
const ROTATE_ARC_RADIUS_LOCAL: f32 = ROTATE_ARC_RADIUS_FRACTION;

/// Number of segments along the rotate arc's 90° major sweep. 32
/// segments is a clean compromise between visual smoothness for a
/// 1-pixel polyline and per-axis vertex count.
const ROTATE_ARC_SEGMENTS: u32 = 32;

/// Builds the axis shaft as a single `LineList` segment from the
/// gizmo origin to the cone base.
pub fn build_axis_shaft_line_geometry() -> (Vec<GizmoVertex>, Vec<u32>) {
    let vertices = vec![
        GizmoVertex {
            position: [0.0, 0.0, 0.0],
        },
        GizmoVertex {
            position: [CONE_BASE_X, 0.0, 0.0],
        },
    ];
    let indices = vec![0, 1];
    (vertices, indices)
}

/// Builds the translate cone tip: a 12-sided solid cone whose base
/// sits at the shaft tip and whose apex points along +X.
pub fn build_axis_cone_tip_geometry() -> (Vec<GizmoVertex>, Vec<u32>) {
    let mut vertices = Vec::with_capacity((CONE_SIDES + 1) as usize);
    let mut indices = Vec::with_capacity((CONE_SIDES * 3) as usize);

    for i in 0..CONE_SIDES {
        let theta = (i as f32 / CONE_SIDES as f32) * std::f32::consts::TAU;
        let (s, c) = theta.sin_cos();
        vertices.push(GizmoVertex {
            position: [CONE_BASE_X, CONE_BASE_RADIUS * c, CONE_BASE_RADIUS * s],
        });
    }
    let tip = vertices.len() as u32;
    vertices.push(GizmoVertex {
        position: [CONE_TIP_X, 0.0, 0.0],
    });

    // CCW-from-outside for each cone side: (base[i], base[i+1], tip).
    // The base ring-cap is omitted — the cone's bottom is hidden by
    // the shaft line ending at the cone base.
    for i in 0..CONE_SIDES {
        let next = (i + 1) % CONE_SIDES;
        indices.extend_from_slice(&[i, next, tip]);
    }

    (vertices, indices)
}

/// Builds the per-axis scale cube: an axis-aligned cube centred at
/// the shaft midpoint along +X, side length `MIDPOINT_CUBE_SIDE_LOCAL`.
pub fn build_axis_scale_cube_geometry() -> (Vec<GizmoVertex>, Vec<u32>) {
    let mut vertices = Vec::with_capacity(8);
    let mut indices = Vec::with_capacity(36);
    let centre = Vec3::new(SHAFT_LENGTH * MIDPOINT_FRACTION, 0.0, 0.0);
    append_axis_aligned_cube(
        &mut vertices,
        &mut indices,
        centre,
        MIDPOINT_CUBE_SIDE_LOCAL,
    );
    (vertices, indices)
}

/// Builds the rotate arc as a `LineList` polyline approximating a
/// 90° sweep around +X, centred on +Y in the YZ plane. The renderer
/// composes a per-frame X-roll to swing the arc onto the camera-
/// facing quadrant before the per-axis world rotation maps it onto
/// X / Y / Z.
pub fn build_rotate_arc_line_geometry() -> (Vec<GizmoVertex>, Vec<u32>) {
    let arc_span = std::f32::consts::FRAC_PI_2;
    let step = arc_span / ROTATE_ARC_SEGMENTS as f32;
    let r = ROTATE_ARC_RADIUS_LOCAL;

    let mut vertices = Vec::with_capacity((ROTATE_ARC_SEGMENTS + 1) as usize);
    let mut indices = Vec::with_capacity((ROTATE_ARC_SEGMENTS * 2) as usize);

    for i in 0..=ROTATE_ARC_SEGMENTS {
        let theta = -arc_span * 0.5 + i as f32 * step;
        let (s, c) = theta.sin_cos();
        vertices.push(GizmoVertex {
            position: [0.0, r * c, r * s],
        });
    }
    for i in 0..ROTATE_ARC_SEGMENTS {
        indices.push(i);
        indices.push(i + 1);
    }

    (vertices, indices)
}

/// Builds the rotate-handle two-cone spindle in handle-local space.
/// Two cones share their bases at the arc's camera-facing midpoint
/// `(0, R, 0)` (where `R = ROTATE_ARC_RADIUS_LOCAL`), tips at
/// `(0, R, ±ROTATE_CONE_LENGTH)` — i.e. along ±Z, the arc tangent at
/// the midpoint. The base disc therefore lies in the handle-local XY
/// plane, perpendicular to the arc curve at that point.
///
/// The renderer composes the same per-frame X-roll used for the arc
/// polyline to swing the spindle onto the camera-facing direction
/// before the world-axis rotation lifts it onto +X / +Y / +Z. This
/// is the *active* rotate handle (the click target for rotate drag).
pub fn build_rotate_cones_geometry() -> (Vec<GizmoVertex>, Vec<u32>) {
    let mut vertices = Vec::with_capacity((CONE_SIDES * 2 + 2) as usize);
    let mut indices = Vec::with_capacity((CONE_SIDES * 6) as usize);
    let midpoint = Vec3::new(0.0, ROTATE_ARC_RADIUS_LOCAL, 0.0);
    let axis = Vec3::Z;
    let basis_a = Vec3::X;
    let basis_b = Vec3::Y;
    append_cone(
        &mut vertices,
        &mut indices,
        midpoint,
        axis,
        basis_a,
        basis_b,
        ROTATE_CONE_LENGTH,
        ROTATE_CONE_RADIUS,
    );
    append_cone(
        &mut vertices,
        &mut indices,
        midpoint,
        -axis,
        basis_a,
        basis_b,
        ROTATE_CONE_LENGTH,
        ROTATE_CONE_RADIUS,
    );
    (vertices, indices)
}

/// Appends one solid cone to `vertices` / `indices`. The cone has
/// its base centred at `base_centre`, base ring in the plane spanned
/// by `basis_a` / `basis_b` (both unit, both perpendicular to
/// `axis_dir`), and tip at `base_centre + axis_dir * length`.
/// CCW-from-outside winding for `axis × basis_a = basis_b`.
#[allow(clippy::too_many_arguments)]
fn append_cone(
    vertices: &mut Vec<GizmoVertex>,
    indices: &mut Vec<u32>,
    base_centre: Vec3,
    axis_dir: Vec3,
    basis_a: Vec3,
    basis_b: Vec3,
    length: f32,
    radius: f32,
) {
    let base_start = vertices.len() as u32;
    for i in 0..CONE_SIDES {
        let theta = (i as f32 / CONE_SIDES as f32) * std::f32::consts::TAU;
        let (s, c) = theta.sin_cos();
        let p = base_centre + basis_a * (radius * c) + basis_b * (radius * s);
        vertices.push(GizmoVertex {
            position: [p.x, p.y, p.z],
        });
    }
    let tip_index = vertices.len() as u32;
    let tip = base_centre + axis_dir * length;
    vertices.push(GizmoVertex {
        position: [tip.x, tip.y, tip.z],
    });
    let positive = axis_dir.dot(basis_a.cross(basis_b)) > 0.0;
    for i in 0..CONE_SIDES {
        let next = (i + 1) % CONE_SIDES;
        if positive {
            indices.extend_from_slice(&[base_start + i, base_start + next, tip_index]);
        } else {
            indices.extend_from_slice(&[base_start + next, base_start + i, tip_index]);
        }
    }
}

/// Appends a 6-face axis-aligned cube to `vertices` / `indices`,
/// centred at `centre`, with side length `side`. Eight shared corner
/// vertices, twelve triangles. CCW-from-outside winding so the cube
/// renders correctly under the same `cull_mode: Back` /
/// `front_face: Ccw` pipeline state used elsewhere.
fn append_axis_aligned_cube(
    vertices: &mut Vec<GizmoVertex>,
    indices: &mut Vec<u32>,
    centre: Vec3,
    side: f32,
) {
    let h = side * 0.5;
    let base = vertices.len() as u32;
    let positions = [
        [centre.x - h, centre.y - h, centre.z - h],
        [centre.x + h, centre.y - h, centre.z - h],
        [centre.x + h, centre.y + h, centre.z - h],
        [centre.x - h, centre.y + h, centre.z - h],
        [centre.x - h, centre.y - h, centre.z + h],
        [centre.x + h, centre.y - h, centre.z + h],
        [centre.x + h, centre.y + h, centre.z + h],
        [centre.x - h, centre.y + h, centre.z + h],
    ];
    for p in positions {
        vertices.push(GizmoVertex { position: p });
    }
    let face_indices: [u32; 36] = [
        4, 5, 6, 4, 6, 7, // +Z
        0, 3, 2, 0, 2, 1, // -Z
        3, 7, 6, 3, 6, 2, // +Y
        0, 1, 5, 0, 5, 4, // -Y
        1, 2, 6, 1, 6, 5, // +X
        0, 4, 7, 0, 7, 3, // -X
    ];
    for &i in &face_indices {
        indices.push(base + i);
    }
}

#[cfg(test)]
mod axis_shaft_line_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    #[test]
    fn shaft_line_runs_from_origin_to_cone_base() {
        let (v, i) = build_axis_shaft_line_geometry();
        assert_eq!(v.len(), 2);
        assert_eq!(i, vec![0, 1]);
        assert_approx_eq!(f32, v[0].position[0], 0.0);
        assert_approx_eq!(f32, v[0].position[1], 0.0);
        assert_approx_eq!(f32, v[0].position[2], 0.0);
        assert_approx_eq!(f32, v[1].position[0], CONE_BASE_X);
        assert_approx_eq!(f32, v[1].position[1], 0.0);
        assert_approx_eq!(f32, v[1].position[2], 0.0);
    }
}

#[cfg(test)]
mod axis_cone_tip_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    #[test]
    fn vertex_and_index_counts_are_pinned() {
        let (v, i) = build_axis_cone_tip_geometry();
        assert_eq!(v.len(), CONE_SIDES as usize + 1);
        assert_eq!(i.len(), CONE_SIDES as usize * 3);
    }

    #[test]
    fn tip_sits_on_x_axis_at_cone_tip_x() {
        let (v, _) = build_axis_cone_tip_geometry();
        let tip = v.last().unwrap();
        assert_approx_eq!(f32, tip.position[0], CONE_TIP_X);
        assert_approx_eq!(f32, tip.position[1], 0.0);
        assert_approx_eq!(f32, tip.position[2], 0.0);
    }

    #[test]
    fn every_index_is_in_bounds() {
        let (v, indices) = build_axis_cone_tip_geometry();
        let max = v.len() as u32;
        for &idx in &indices {
            assert!(idx < max);
        }
    }

    #[test]
    fn every_triangle_winds_ccw_outward() {
        let (v, indices) = build_axis_cone_tip_geometry();
        for tri in indices.chunks_exact(3) {
            let p0 = Vec3::from(v[tri[0] as usize].position);
            let p1 = Vec3::from(v[tri[1] as usize].position);
            let p2 = Vec3::from(v[tri[2] as usize].position);
            let centroid = (p0 + p1 + p2) / 3.0;
            let n = (p1 - p0).cross(p2 - p0).normalize();
            // Outward at the cone side is the radial-plus-axial blend;
            // at any centroid it has positive dot product with both
            // +X (axial) and the YZ-plane radial.
            let radial = Vec3::new(0.0, centroid.y, centroid.z);
            assert!(n.x > 0.0 || n.dot(radial) > 0.0);
        }
    }
}

#[cfg(test)]
mod axis_scale_cube_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    #[test]
    fn cube_is_centred_on_shaft_midpoint() {
        let (v, _) = build_axis_scale_cube_geometry();
        assert_eq!(v.len(), 8);
        let mut centre = Vec3::ZERO;
        for vert in &v {
            centre += Vec3::from(vert.position);
        }
        centre /= v.len() as f32;
        assert_approx_eq!(
            f32,
            centre.x,
            SHAFT_LENGTH * MIDPOINT_FRACTION,
            epsilon = 1e-5
        );
        assert_approx_eq!(f32, centre.y, 0.0, epsilon = 1e-5);
        assert_approx_eq!(f32, centre.z, 0.0, epsilon = 1e-5);
    }

    #[test]
    fn every_triangle_winds_ccw_outward() {
        let (v, indices) = build_axis_scale_cube_geometry();
        let cube_centre = Vec3::new(SHAFT_LENGTH * MIDPOINT_FRACTION, 0.0, 0.0);
        for tri in indices.chunks_exact(3) {
            let p0 = Vec3::from(v[tri[0] as usize].position);
            let p1 = Vec3::from(v[tri[1] as usize].position);
            let p2 = Vec3::from(v[tri[2] as usize].position);
            let centroid = (p0 + p1 + p2) / 3.0;
            let n = (p1 - p0).cross(p2 - p0).normalize();
            let outward = (centroid - cube_centre).normalize();
            assert!(n.dot(outward) > 0.0);
        }
    }
}

#[cfg(test)]
mod rotate_arc_line_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    #[test]
    fn vertex_and_index_counts_match_segment_count() {
        let (v, i) = build_rotate_arc_line_geometry();
        assert_eq!(v.len() as u32, ROTATE_ARC_SEGMENTS + 1);
        assert_eq!(i.len() as u32, ROTATE_ARC_SEGMENTS * 2);
    }

    #[test]
    fn every_index_is_in_bounds() {
        let (v, indices) = build_rotate_arc_line_geometry();
        let max = v.len() as u32;
        for &idx in &indices {
            assert!(idx < max);
        }
    }

    #[test]
    fn every_vertex_lies_on_the_major_circle() {
        let (v, _) = build_rotate_arc_line_geometry();
        let r = ROTATE_ARC_RADIUS_LOCAL;
        for vert in &v {
            let p = Vec3::from(vert.position);
            // Polyline lies in the YZ plane (handle-local +X = 0)
            // and traces a circle of radius r.
            assert_approx_eq!(f32, p.x, 0.0, epsilon = 1e-5);
            assert_approx_eq!(f32, p.y.hypot(p.z), r, epsilon = 1e-5);
        }
    }

    #[test]
    fn middle_vertex_sits_on_plus_y() {
        let (v, _) = build_rotate_arc_line_geometry();
        let mid = (ROTATE_ARC_SEGMENTS / 2) as usize;
        let p = Vec3::from(v[mid].position);
        assert_approx_eq!(f32, p.x, 0.0, epsilon = 1e-5);
        assert_approx_eq!(f32, p.y, ROTATE_ARC_RADIUS_LOCAL, epsilon = 1e-5);
        assert_approx_eq!(f32, p.z, 0.0, epsilon = 1e-5);
    }

    #[test]
    fn endpoints_sit_at_plus_minus_45_degrees() {
        let (v, _) = build_rotate_arc_line_geometry();
        let r = ROTATE_ARC_RADIUS_LOCAL;
        let expected = r * std::f32::consts::FRAC_1_SQRT_2;
        let first = Vec3::from(v[0].position);
        let last = Vec3::from(v[v.len() - 1].position);
        // First vertex at θ = -π/4: (0, r cos(-π/4), r sin(-π/4)).
        assert_approx_eq!(f32, first.y, expected, epsilon = 1e-5);
        assert_approx_eq!(f32, first.z, -expected, epsilon = 1e-5);
        // Last vertex at θ = +π/4.
        assert_approx_eq!(f32, last.y, expected, epsilon = 1e-5);
        assert_approx_eq!(f32, last.z, expected, epsilon = 1e-5);
    }
}

#[cfg(test)]
mod rotate_cones_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    #[test]
    fn vertex_and_index_counts_are_pinned() {
        let (v, i) = build_rotate_cones_geometry();
        // Two cones, each with `CONE_SIDES` base ring vertices + 1 tip.
        assert_eq!(v.len() as u32, CONE_SIDES * 2 + 2);
        assert_eq!(i.len() as u32, CONE_SIDES * 6);
    }

    #[test]
    fn every_index_is_in_bounds() {
        let (v, indices) = build_rotate_cones_geometry();
        let max = v.len() as u32;
        for &idx in &indices {
            assert!(idx < max);
        }
    }

    #[test]
    fn tips_extend_along_arc_tangent_at_midpoint() {
        // Bases share the arc midpoint (0, R, 0); tips run along ±Z
        // (the arc tangent at that point), not along ±X (the rotation
        // axis). The two tips are the last vertex of each cone block.
        let (v, _) = build_rotate_cones_geometry();
        let r = ROTATE_ARC_RADIUS_LOCAL;
        let l = ROTATE_CONE_LENGTH;

        let tip_a = Vec3::from(v[CONE_SIDES as usize].position);
        let tip_b = Vec3::from(v[(CONE_SIDES * 2 + 1) as usize].position);

        assert_approx_eq!(f32, tip_a.x, 0.0, epsilon = 1e-5);
        assert_approx_eq!(f32, tip_a.y, r, epsilon = 1e-5);
        assert_approx_eq!(f32, tip_a.z.abs(), l, epsilon = 1e-5);

        assert_approx_eq!(f32, tip_b.x, 0.0, epsilon = 1e-5);
        assert_approx_eq!(f32, tip_b.y, r, epsilon = 1e-5);
        assert_approx_eq!(f32, tip_b.z.abs(), l, epsilon = 1e-5);

        // The two tips are on opposite sides of the midpoint along Z.
        assert!(tip_a.z * tip_b.z < 0.0);
    }

    #[test]
    fn base_ring_sits_in_xy_plane_through_midpoint() {
        // Each cone block is `CONE_SIDES` base ring vertices followed
        // by one tip vertex, so the second cone's ring starts at
        // `CONE_SIDES + 1`. With axis = ±Z the ring lies in the plane
        // z = 0, centred at (0, R, 0) with radius ROTATE_CONE_RADIUS.
        let (v, _) = build_rotate_cones_geometry();
        let r = ROTATE_ARC_RADIUS_LOCAL;
        let cone_r = ROTATE_CONE_RADIUS;
        for &start in &[0_usize, CONE_SIDES as usize + 1] {
            for i in 0..CONE_SIDES as usize {
                let p = Vec3::from(v[start + i].position);
                assert_approx_eq!(f32, p.z, 0.0, epsilon = 1e-5);
                let radial = (p - Vec3::new(0.0, r, 0.0)).length();
                assert_approx_eq!(f32, radial, cone_r, epsilon = 1e-5);
            }
        }
    }
}
