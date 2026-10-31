/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Procedural geometry for the gizmo handles, all in **handle-local space**:
//! pivot at origin, primary axis along +X, 1 unit = the shaft length. The
//! renderer draws each handle mesh once per axis, rotating onto +X / +Y / +Z,
//! with per-draw tint uniforms.
//!
//! Every handle is a closed triangle mesh with a real thickness in pixels, so
//! it reads as a 3 px stroke, not a 1 px line. Each vertex also carries an
//! `offset`: moving every vertex by `offset * d` grows the surface outward by
//! `d`, which is how the renderer draws the dark outline around a handle (the
//! same mesh, grown by the outline width, drawn first).
//!
//! - [`build_arrow_geometry`]: the move arrow, a shaft capped by a cone.
//! - [`build_ring_geometry`]: the full rotate ring around +X.
//! - [`build_scale_cube_geometry`]: the per-axis scale cube.
//! - [`build_centre_ball_geometry`]: the uniform-scale ball at the pivot.
//! - [`build_rotate_marks`]: while rotating, the swept slice, the ticks every
//!   15° and the start and end spokes, built per frame.

use bytemuck::{Pod, Zeroable};
use glam::Vec3;

use crate::gizmo::axis::{Axis, axis_basis};
use crate::gizmo::dimensions::{
    AXIS_SCALE_CUBE_PIXELS, CENTRE_BALL_PIXELS, CONE_BASE_X, CONE_RADIUS_PIXELS, CONE_TIP_X,
    MIDPOINT_FRACTION, RING_TUBE_PIXELS, ROTATE_ARC_RADIUS_FRACTION, SHAFT_RADIUS_PIXELS,
    SHAFT_START_PIXELS, px,
};

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
pub(crate) struct GizmoVertex {
    pub position: [f32; 3],
    /// Direction the vertex moves to grow the surface by one unit (see the
    /// module docs). Zero for the flat marks, which have no outline.
    pub offset: [f32; 3],
}

impl GizmoVertex {
    fn new(position: Vec3, offset: Vec3) -> Self {
        Self {
            position: position.to_array(),
            offset: offset.to_array(),
        }
    }

    fn flat(position: Vec3) -> Self {
        Self::new(position, Vec3::ZERO)
    }
}

/// Sides of the shaft cylinder and of the cone.
const SHAFT_SIDES: u32 = 12;
const CONE_SIDES: u32 = 24;

/// Segments around a rotate ring and around its tube.
pub(crate) const RING_SEGMENTS: u32 = 128;
const RING_TUBE_SIDES: u32 = 10;

/// Rings and segments of the centre ball.
const BALL_RINGS: u32 = 10;
const BALL_SEGMENTS: u32 = 16;

/// Major (circle) radius of the rotate ring in handle-local space.
pub(crate) const RING_RADIUS_LOCAL: f32 = ROTATE_ARC_RADIUS_FRACTION;

/// The swept slice reaches this far toward the ring.
const WEDGE_RADIUS_FRACTION: f32 = 0.97;
/// Segments of the swept slice per radian.
const WEDGE_SEGMENTS_PER_RADIAN: f32 = 20.0;

/// Builds the move arrow along +X: a shaft from just past the centre ball to
/// the cone base, then a cone to the tip. Both pieces are closed.
pub(crate) fn build_arrow_geometry() -> (Vec<GizmoVertex>, Vec<u32>) {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    // Shaft: a ring at the start (with its cap) and one at the cone base. The
    // start ring grows backward as well as outward, so the cap stays flat.
    let start = px(SHAFT_START_PIXELS);
    let radius = px(SHAFT_RADIUS_PIXELS);
    let ring_dir = |i: u32| {
        let theta = i as f32 / SHAFT_SIDES as f32 * std::f32::consts::TAU;
        Vec3::new(0.0, theta.cos(), theta.sin())
    };
    for i in 0..SHAFT_SIDES {
        let r = ring_dir(i);
        vertices.push(GizmoVertex::new(
            Vec3::new(start, 0.0, 0.0) + r * radius,
            r - Vec3::X,
        ));
    }
    for i in 0..SHAFT_SIDES {
        let r = ring_dir(i);
        vertices.push(GizmoVertex::new(
            Vec3::new(CONE_BASE_X, 0.0, 0.0) + r * radius,
            r,
        ));
    }
    let cap = vertices.len() as u32;
    vertices.push(GizmoVertex::new(Vec3::new(start, 0.0, 0.0), -Vec3::X));
    for i in 0..SHAFT_SIDES {
        let next = (i + 1) % SHAFT_SIDES;
        let (a0, a1, b0, b1) = (i, next, SHAFT_SIDES + i, SHAFT_SIDES + next);
        indices.extend_from_slice(&[a0, b1, b0, a0, a1, b1]);
        indices.extend_from_slice(&[cap, a1, a0]);
    }

    // Cone: base ring, base centre and apex. The offsets move the slanted
    // side and the base plane each by one unit (see `cone_offsets`).
    let cone_len = CONE_TIP_X - CONE_BASE_X;
    let cone_r = px(CONE_RADIUS_PIXELS);
    let (rim_axial, rim_radial, apex_axial) = cone_offsets(cone_r, cone_len);
    let base = vertices.len() as u32;
    for i in 0..CONE_SIDES {
        let theta = i as f32 / CONE_SIDES as f32 * std::f32::consts::TAU;
        let r = Vec3::new(0.0, theta.cos(), theta.sin());
        vertices.push(GizmoVertex::new(
            Vec3::new(CONE_BASE_X, 0.0, 0.0) + r * cone_r,
            Vec3::X * rim_axial + r * rim_radial,
        ));
    }
    let base_centre = vertices.len() as u32;
    vertices.push(GizmoVertex::new(Vec3::new(CONE_BASE_X, 0.0, 0.0), -Vec3::X));
    let apex = vertices.len() as u32;
    vertices.push(GizmoVertex::new(
        Vec3::new(CONE_TIP_X, 0.0, 0.0),
        Vec3::X * apex_axial,
    ));
    for i in 0..CONE_SIDES {
        let next = (i + 1) % CONE_SIDES;
        indices.extend_from_slice(&[base + i, base + next, apex]);
        indices.extend_from_slice(&[base_centre, base + next, base + i]);
    }
    (vertices, indices)
}

/// Offsets that grow a cone of base radius `r` and length `len` by one unit:
/// `(rim axial, rim radial, apex axial)`. The rim moves back off the base
/// plane and out off the slanted side; the apex moves along the axis.
fn cone_offsets(r: f32, len: f32) -> (f32, f32, f32) {
    let slant = r.hypot(len);
    (-1.0, (slant + r) / len, slant / r)
}

/// Builds the full rotate ring around +X: a torus in the YZ plane with the
/// ring radius and a tube of `RING_TUBE_PIXELS`.
pub(crate) fn build_ring_geometry() -> (Vec<GizmoVertex>, Vec<u32>) {
    let tube = px(RING_TUBE_PIXELS);
    let mut vertices = Vec::with_capacity((RING_SEGMENTS * RING_TUBE_SIDES) as usize);
    let mut indices = Vec::with_capacity((RING_SEGMENTS * RING_TUBE_SIDES * 6) as usize);
    for i in 0..RING_SEGMENTS {
        let phi = i as f32 / RING_SEGMENTS as f32 * std::f32::consts::TAU;
        let radial = Vec3::new(0.0, phi.cos(), phi.sin());
        let centre = radial * RING_RADIUS_LOCAL;
        for j in 0..RING_TUBE_SIDES {
            let theta = j as f32 / RING_TUBE_SIDES as f32 * std::f32::consts::TAU;
            let normal = radial * theta.cos() + Vec3::X * theta.sin();
            vertices.push(GizmoVertex::new(centre + normal * tube, normal));
        }
    }
    for i in 0..RING_SEGMENTS {
        let ni = (i + 1) % RING_SEGMENTS;
        for j in 0..RING_TUBE_SIDES {
            let nj = (j + 1) % RING_TUBE_SIDES;
            let a = i * RING_TUBE_SIDES + j;
            let b = ni * RING_TUBE_SIDES + j;
            let c = ni * RING_TUBE_SIDES + nj;
            let d = i * RING_TUBE_SIDES + nj;
            indices.extend_from_slice(&[a, c, d, a, b, c]);
        }
    }
    (vertices, indices)
}

/// Builds the per-axis scale cube: an axis-aligned cube centred on the shaft at
/// `MIDPOINT_FRACTION`, `AXIS_SCALE_CUBE_PIXELS` on a side.
pub(crate) fn build_scale_cube_geometry() -> (Vec<GizmoVertex>, Vec<u32>) {
    let centre = Vec3::new(MIDPOINT_FRACTION, 0.0, 0.0);
    let h = px(AXIS_SCALE_CUBE_PIXELS) * 0.5;
    let corners = [
        Vec3::new(-1.0, -1.0, -1.0),
        Vec3::new(1.0, -1.0, -1.0),
        Vec3::new(1.0, 1.0, -1.0),
        Vec3::new(-1.0, 1.0, -1.0),
        Vec3::new(-1.0, -1.0, 1.0),
        Vec3::new(1.0, -1.0, 1.0),
        Vec3::new(1.0, 1.0, 1.0),
        Vec3::new(-1.0, 1.0, 1.0),
    ];
    // A corner moves along (±1, ±1, ±1): each face plane moves by one unit.
    let vertices = corners
        .iter()
        .map(|&c| GizmoVertex::new(centre + c * h, c))
        .collect();
    let indices = vec![
        4, 5, 6, 4, 6, 7, // +Z
        0, 3, 2, 0, 2, 1, // -Z
        3, 7, 6, 3, 6, 2, // +Y
        0, 1, 5, 0, 5, 4, // -Y
        1, 2, 6, 1, 6, 5, // +X
        0, 4, 7, 0, 7, 3, // -X
    ];
    (vertices, indices)
}

/// Builds the uniform-scale ball at the pivot.
pub(crate) fn build_centre_ball_geometry() -> (Vec<GizmoVertex>, Vec<u32>) {
    let r = px(CENTRE_BALL_PIXELS);
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    vertices.push(GizmoVertex::new(Vec3::Z * r, Vec3::Z));
    for i in 1..BALL_RINGS {
        let theta = i as f32 / BALL_RINGS as f32 * std::f32::consts::PI;
        for j in 0..BALL_SEGMENTS {
            let phi = j as f32 / BALL_SEGMENTS as f32 * std::f32::consts::TAU;
            let n = Vec3::new(
                theta.sin() * phi.cos(),
                theta.sin() * phi.sin(),
                theta.cos(),
            );
            vertices.push(GizmoVertex::new(n * r, n));
        }
    }
    let bottom = vertices.len() as u32;
    vertices.push(GizmoVertex::new(-Vec3::Z * r, -Vec3::Z));
    let at = |ring: u32, seg: u32| 1 + (ring - 1) * BALL_SEGMENTS + seg % BALL_SEGMENTS;
    for j in 0..BALL_SEGMENTS {
        indices.extend_from_slice(&[0, at(1, j), at(1, j + 1)]);
        indices.extend_from_slice(&[bottom, at(BALL_RINGS - 1, j + 1), at(BALL_RINGS - 1, j)]);
    }
    for ring in 1..BALL_RINGS - 1 {
        for j in 0..BALL_SEGMENTS {
            let (a, b) = (at(ring, j), at(ring, j + 1));
            let (c, d) = (at(ring + 1, j), at(ring + 1, j + 1));
            indices.extend_from_slice(&[a, c, d, a, d, b]);
        }
    }
    (vertices, indices)
}

/// The marks of a turn in progress, as plain triangle lists in world-aligned
/// handle-local space (no per-axis rotation).
#[derive(Debug, Default)]
pub(crate) struct RotateMarks {
    /// The swept slice, from the pivot.
    pub wedge: Vec<GizmoVertex>,
    /// The ticks every 15° (longer every 90°) from the start angle, and the
    /// spokes to the start and to the current angle.
    pub marks: Vec<GizmoVertex>,
}

/// Builds the marks of a turn around `axis` that started at `start` (the
/// cursor angle at press, in the basis of `axis_basis`) and has turned
/// `sweep` radians so far. The slice shows at most one full turn.
pub(crate) fn build_rotate_marks(axis: Axis, start: f32, sweep: f32) -> RotateMarks {
    let (u, v) = axis_basis(axis);
    let dir = |a: f32| u * a.cos() + v * a.sin();
    let tangent = |a: f32| -u * a.sin() + v * a.cos();
    let r = RING_RADIUS_LOCAL;
    let mut out = RotateMarks::default();

    let shown = sweep.clamp(-std::f32::consts::TAU, std::f32::consts::TAU);
    let steps = ((shown.abs() * WEDGE_SEGMENTS_PER_RADIAN).ceil() as u32).max(1);
    let wedge_r = r * WEDGE_RADIUS_FRACTION;
    if shown.abs() > 1e-4 {
        for s in 0..steps {
            let a0 = start + shown * s as f32 / steps as f32;
            let a1 = start + shown * (s + 1) as f32 / steps as f32;
            out.wedge.extend_from_slice(&[
                GizmoVertex::flat(Vec3::ZERO),
                GizmoVertex::flat(dir(a0) * wedge_r),
                GizmoVertex::flat(dir(a1) * wedge_r),
            ]);
        }
    }

    let mut quad = |a: f32, r0: f32, r1: f32, half_width: f32| {
        let (d, t) = (dir(a), tangent(a) * half_width);
        let p = |rr: f32, s: f32| GizmoVertex::flat(d * rr + t * s);
        out.marks.extend_from_slice(&[
            p(r0, -1.0),
            p(r1, -1.0),
            p(r1, 1.0),
            p(r0, -1.0),
            p(r1, 1.0),
            p(r0, 1.0),
        ]);
    };
    let tick_from = r + px(RING_TUBE_PIXELS + 4.0);
    for t in 0..24 {
        let major = t % 6 == 0;
        let a = start + t as f32 * std::f32::consts::PI / 12.0;
        let (len, half) = if major { (16.0, 1.3) } else { (10.0, 0.9) };
        quad(a, tick_from, r + px(len), px(half));
    }
    for a in [start, start + sweep] {
        quad(a, 0.0, r, px(1.0));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    fn pos(v: &[GizmoVertex], i: u32) -> Vec3 {
        Vec3::from(v[i as usize].position)
    }

    fn assert_indices_in_bounds(v: &[GizmoVertex], indices: &[u32]) {
        assert_eq!(indices.len() % 3, 0);
        for &i in indices {
            assert!((i as usize) < v.len());
        }
    }

    /// Every triangle faces away from `inside(centroid)`, a point inside the
    /// solid near the triangle: CCW seen from outside, as back-face culling
    /// expects.
    fn assert_faces_outward(v: &[GizmoVertex], indices: &[u32], inside: impl Fn(Vec3) -> Vec3) {
        for tri in indices.chunks(3) {
            let (p0, p1, p2) = (pos(v, tri[0]), pos(v, tri[1]), pos(v, tri[2]));
            let centroid = (p0 + p1 + p2) / 3.0;
            let n = (p1 - p0).cross(p2 - p0);
            assert!(
                n.dot(centroid - inside(centroid)) > 0.0,
                "triangle {tri:?} faces inward"
            );
        }
    }

    /// Grows every vertex by `d` along its offset.
    fn grown(v: &[GizmoVertex], d: f32) -> Vec<Vec3> {
        v.iter()
            .map(|x| Vec3::from(x.position) + Vec3::from(x.offset) * d)
            .collect()
    }

    #[test]
    fn arrow_runs_from_past_the_centre_to_the_tip_and_faces_out() {
        let (v, i) = build_arrow_geometry();
        assert_indices_in_bounds(&v, &i);
        let xs: Vec<f32> = v.iter().map(|x| x.position[0]).collect();
        let min = xs.iter().copied().fold(f32::MAX, f32::min);
        let max = xs.iter().copied().fold(f32::MIN, f32::max);
        assert_approx_eq!(f32, min, px(SHAFT_START_PIXELS));
        assert_approx_eq!(f32, max, CONE_TIP_X);
        // A point on the axis beside each triangle; for the cone base, just
        // inside the cone.
        assert_faces_outward(&v, &i, |c| {
            let x = if (c.x - CONE_BASE_X).abs() < 1e-6 {
                CONE_BASE_X + 0.05
            } else {
                c.x.clamp(0.1, 1.1)
            };
            Vec3::new(x, 0.0, 0.0)
        });
    }

    #[test]
    fn arrow_is_a_three_pixel_line_with_a_thirteen_pixel_cone() {
        let (v, _) = build_arrow_geometry();
        let radial = |p: Vec3| p.y.hypot(p.z);
        let shaft = v
            .iter()
            .map(|x| Vec3::from(x.position))
            .filter(|p| p.x < CONE_BASE_X - 1e-4)
            .map(radial)
            .fold(0.0, f32::max);
        assert_approx_eq!(f32, shaft * 2.0, px(3.0), epsilon = 1e-6);
        let cone = v
            .iter()
            .map(|x| Vec3::from(x.position))
            .map(radial)
            .fold(0.0, f32::max);
        assert_approx_eq!(f32, cone * 2.0, px(13.0), epsilon = 1e-6);
    }

    #[test]
    fn the_outline_grows_the_cone_side_and_base_by_the_same_width() {
        // The grown cone's slanted side and base each sit `d` from the drawn
        // ones, so the outline is even all round.
        let (v, _) = build_arrow_geometry();
        let d = 0.01;
        let r = px(CONE_RADIUS_PIXELS);
        let len = CONE_TIP_X - CONE_BASE_X;
        let n2 = glam::Vec2::new(r, len).normalize();
        let g = grown(&v, d);
        let apex = g[v.len() - 1];
        assert_approx_eq!(f32, (apex.x - CONE_TIP_X) * n2.x, d, epsilon = 1e-6);
        let rim = g[v.len() - 2 - CONE_SIDES as usize];
        assert_approx_eq!(f32, rim.x, CONE_BASE_X - d, epsilon = 1e-6);
        // Distance from the grown rim point to the drawn slant line is `d`.
        let rim2 = glam::Vec2::new(rim.x - CONE_BASE_X, rim.y.hypot(rim.z) - r);
        assert_approx_eq!(f32, rim2.dot(n2), d, epsilon = 1e-6);
    }

    #[test]
    fn ring_is_a_full_circle_with_a_three_pixel_tube() {
        let (v, i) = build_ring_geometry();
        assert_indices_in_bounds(&v, &i);
        for x in &v {
            let p = Vec3::from(x.position);
            let to_circle = p.y.hypot(p.z) - RING_RADIUS_LOCAL;
            assert_approx_eq!(
                f32,
                to_circle.hypot(p.x),
                px(RING_TUBE_PIXELS),
                epsilon = 1e-6
            );
        }
        // The ring spans every direction in its plane, not only a quadrant.
        let angles: Vec<f32> = v
            .iter()
            .map(|x| x.position[2].atan2(x.position[1]))
            .collect();
        assert!(angles.iter().any(|a| *a > 3.0));
        assert!(angles.iter().any(|a| *a < -3.0));
        assert!(angles.iter().any(|a| a.abs() < 0.05));
        assert_faces_outward(&v, &i, |c| {
            let r = Vec3::new(0.0, c.y, c.z).normalize_or_zero();
            r * RING_RADIUS_LOCAL
        });
    }

    #[test]
    fn cube_sits_on_the_shaft_and_faces_out() {
        let (v, i) = build_scale_cube_geometry();
        assert_indices_in_bounds(&v, &i);
        let centre = Vec3::new(MIDPOINT_FRACTION, 0.0, 0.0);
        let mean = v.iter().map(|x| Vec3::from(x.position)).sum::<Vec3>() / v.len() as f32;
        assert!((mean - centre).length() < 1e-6);
        assert_faces_outward(&v, &i, |_| centre);
        // Growing by `d` moves each face out by exactly `d`.
        let g = grown(&v, 0.01);
        let max_x = g.iter().map(|p| p.x).fold(f32::MIN, f32::max);
        assert_approx_eq!(
            f32,
            max_x,
            MIDPOINT_FRACTION + px(AXIS_SCALE_CUBE_PIXELS) * 0.5 + 0.01,
            epsilon = 1e-6
        );
    }

    #[test]
    fn centre_ball_is_round_and_faces_out() {
        let (v, i) = build_centre_ball_geometry();
        assert_indices_in_bounds(&v, &i);
        for x in &v {
            assert_approx_eq!(
                f32,
                Vec3::from(x.position).length(),
                px(CENTRE_BALL_PIXELS),
                epsilon = 1e-6
            );
        }
        assert_faces_outward(&v, &i, |_| Vec3::ZERO);
    }

    #[test]
    fn rotate_marks_sweep_from_the_start_angle() {
        let marks = build_rotate_marks(Axis::Z, 0.0, std::f32::consts::FRAC_PI_2);
        assert!(!marks.wedge.is_empty());
        assert_eq!(marks.wedge.len() % 3, 0);
        // The slice lies in the XY plane (Z ring), from +X to +Y.
        for x in &marks.wedge {
            assert_approx_eq!(f32, x.position[2], 0.0);
            assert!(x.position[0] >= -1e-6 && x.position[1] >= -1e-6);
        }
        let far = marks
            .wedge
            .iter()
            .map(|x| Vec3::from(x.position).length())
            .fold(0.0, f32::max);
        assert!(far <= RING_RADIUS_LOCAL);
        // 24 ticks and 2 spokes, 6 vertices each.
        assert_eq!(marks.marks.len(), 26 * 6);
    }

    #[test]
    fn rotate_marks_show_no_slice_before_turning_and_at_most_one_turn() {
        assert!(build_rotate_marks(Axis::X, 0.3, 0.0).wedge.is_empty());
        let many = build_rotate_marks(Axis::X, 0.0, 5.0 * std::f32::consts::TAU);
        let one = build_rotate_marks(Axis::X, 0.0, std::f32::consts::TAU);
        assert_eq!(many.wedge.len(), one.wedge.len());
    }

    #[test]
    fn rotate_marks_follow_the_drag_angle_basis_of_every_axis() {
        // The spoke to the current angle points where `rotate_drag_angle`
        // measures it: angle a in the (u, v) basis of the axis.
        for axis in [Axis::X, Axis::Y, Axis::Z] {
            let (u, v) = axis_basis(axis);
            let a = 1.0;
            let marks = build_rotate_marks(axis, 0.0, a);
            let spoke = &marks.marks[marks.marks.len() - 6..];
            let tip = (Vec3::from(spoke[1].position) + Vec3::from(spoke[2].position)) * 0.5;
            let expected = (u * a.cos() + v * a.sin()) * RING_RADIUS_LOCAL;
            assert!((tip - expected).length() < 1e-5, "{axis:?}");
        }
    }
}
