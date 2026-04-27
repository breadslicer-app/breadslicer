/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Procedural mesh for the gizmo handles, all in **handle-local
//! space** — pivot at origin, primary axis along +X. The renderer
//! draws each buffer three times (or once, for the centre cube),
//! rotating onto +X, +Y, +Z; per-axis tint uniforms colour each draw.
//!
//! - **Axis geometry** ([`build_axis_geometry`]): the 8-sided shaft
//!   prism + 12-sided cone tip *plus* a small cube at the shaft
//!   midpoint that doubles as the per-axis scale handle. The two
//!   sub-ranges are exposed via the returned `shaft_tip_index_count`
//!   so the renderer can issue separate draws (different tints) for
//!   the translate part vs. the scale midpoint cube.
//! - **Rotate arc** ([`build_rotate_arc_geometry`]): a 90° torus
//!   segment around +X, swept through the camera-facing quadrant by
//!   a per-frame transform.
//! - **Centre cube** ([`build_centre_cube_geometry`]): a single cube
//!   at the gizmo pivot, the uniform-scale handle.
//!
//! Vertex attribute is position only — every vertex would be white
//! under the per-axis tint approach, so a colour attribute would be
//! pure bandwidth. Easy to add back if a future polish item wants
//! per-vertex shading (e.g. a brighter tip).

use bytemuck::{Pod, Zeroable};
use glam::Vec3;

use crate::gizmo::{
    AXIS_SCALE_CUBE_PIXELS, CENTRE_CUBE_PIXELS, HANDLE_PIXEL_LENGTH, MIDPOINT_FRACTION,
    ROTATE_ARC_RADIUS_FRACTION, ROTATE_ARC_TUBE_PIXELS,
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
const SHAFT_RADIUS: f32 = 0.025;
const CONE_BASE_RADIUS: f32 = 0.07;

const SHAFT_SIDES: u32 = 8;
const CONE_SIDES: u32 = 12;

/// Side length, in handle-local space, of the per-axis midpoint
/// scale cube. The shaft is 1.0 unit long in handle-local space and
/// `HANDLE_PIXEL_LENGTH` pixels long on screen, so the world-to-pixel
/// ratio is `1.0 / HANDLE_PIXEL_LENGTH`; multiplying by the cube's
/// screen-pixel size gives its handle-local size.
const MIDPOINT_CUBE_SIDE_LOCAL: f32 = AXIS_SCALE_CUBE_PIXELS / HANDLE_PIXEL_LENGTH;

/// Side length of the centre (uniform-scale) cube in handle-local
/// space. Same conversion as `MIDPOINT_CUBE_SIDE_LOCAL`.
const CENTRE_CUBE_SIDE_LOCAL: f32 = CENTRE_CUBE_PIXELS / HANDLE_PIXEL_LENGTH;

/// Major (circle) radius of the rotate arc in handle-local space.
/// Already a fraction of the shaft length, which is 1.0 in this
/// space, so the constant maps 1:1.
const ROTATE_ARC_RADIUS_LOCAL: f32 = ROTATE_ARC_RADIUS_FRACTION;

/// Minor (tube) radius of the rotate arc in handle-local space.
const ROTATE_ARC_TUBE_LOCAL: f32 = ROTATE_ARC_TUBE_PIXELS / HANDLE_PIXEL_LENGTH;

/// Major-angle segment count for the rotate arc. 8 segments across a
/// 90° span gives ~11.25° between rings — visually smooth for a
/// pixel-radius tube.
const ROTATE_ARC_MAJOR_SEGMENTS: u32 = 8;

/// Minor-angle segment count for the rotate arc tube cross-section.
const ROTATE_ARC_MINOR_SEGMENTS: u32 = 8;

/// Builds the gizmo axis geometry once: shaft prism + cone tip
/// (the translate handle) followed by a midpoint cube (the per-axis
/// scale handle), all in a single VBO/IBO.
///
/// The returned `shaft_tip_index_count` is the number of indices
/// belonging to the translate part (shaft + cone). Indices in
/// `[0, shaft_tip_index_count)` draw the translate handle; indices
/// in `[shaft_tip_index_count, indices.len())` draw the scale
/// midpoint cube. The renderer issues two `draw_indexed` calls per
/// axis with different tints so the user can tell which sub-handle
/// the cursor is over.
pub fn build_axis_geometry() -> (Vec<GizmoVertex>, Vec<u32>, u32) {
    let mut vertices: Vec<GizmoVertex> =
        Vec::with_capacity((SHAFT_SIDES * 2 + CONE_SIDES + 1 + 8) as usize);
    let mut indices: Vec<u32> =
        Vec::with_capacity((SHAFT_SIDES * 6 + CONE_SIDES * 3 + 36) as usize);

    // ---- shaft prism ----------------------------------------------------
    // Two rings of `SHAFT_SIDES` vertices: ring 0 at x=0, ring 1 at
    // x=SHAFT_LENGTH. Vertices interleaved so ring0[i] = 2*i and
    // ring1[i] = 2*i + 1; the side-face triangulation below relies on
    // that layout.
    for i in 0..SHAFT_SIDES {
        let theta = (i as f32 / SHAFT_SIDES as f32) * std::f32::consts::TAU;
        let (s, c) = theta.sin_cos();
        let y = SHAFT_RADIUS * c;
        let z = SHAFT_RADIUS * s;
        vertices.push(GizmoVertex {
            position: [0.0, y, z],
        });
        vertices.push(GizmoVertex {
            position: [SHAFT_LENGTH, y, z],
        });
    }

    // CCW-from-outside winding for each side quad. For the face at
    // theta_i:
    //   a (ring0, i)   - lower-left
    //   c (ring0, i+1) - lower-right
    //   d (ring1, i+1) - upper-right
    //   b (ring1, i)   - upper-left
    // → triangles (a, c, d) and (a, d, b). The "outside" view is
    // along the radial direction at theta_i; checking the cross
    // product of any of these triangles confirms the outward normal
    // points along +radial.
    for i in 0..SHAFT_SIDES {
        let next = (i + 1) % SHAFT_SIDES;
        let a = i * 2;
        let b = i * 2 + 1;
        let c = next * 2;
        let d = next * 2 + 1;
        indices.extend_from_slice(&[a, c, d, a, d, b]);
    }

    // ---- cone -----------------------------------------------------------
    let cone_base_start = vertices.len() as u32;
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
    // The base ring-cap (closing the cone's bottom) is omitted — the
    // shaft prism's end ring sits inside the cone base ring radius
    // and visually plugs the gap; a dedicated cap fan would only be
    // visible if the user could see past the cone, which doesn't
    // happen in the gizmo's intended use.
    for i in 0..CONE_SIDES {
        let next = (i + 1) % CONE_SIDES;
        indices.extend_from_slice(&[cone_base_start + i, cone_base_start + next, tip]);
    }

    let shaft_tip_index_count = indices.len() as u32;

    // ---- midpoint scale cube --------------------------------------------
    // Centred at the shaft midpoint along +X, axis-aligned. Vertices
    // appended at the end of the buffer so the renderer can pick up
    // the cube as a separate index sub-range without reuploading.
    let midpoint = Vec3::new(SHAFT_LENGTH * MIDPOINT_FRACTION, 0.0, 0.0);
    append_axis_aligned_cube(
        &mut vertices,
        &mut indices,
        midpoint,
        MIDPOINT_CUBE_SIDE_LOCAL,
    );

    (vertices, indices, shaft_tip_index_count)
}

/// Builds the centre (uniform-scale) cube geometry: a single
/// axis-aligned cube centred on the gizmo pivot. The renderer
/// uploads this once at construction time and reuses it.
#[allow(dead_code)]
pub fn build_centre_cube_geometry() -> (Vec<GizmoVertex>, Vec<u32>) {
    let mut vertices = Vec::with_capacity(8);
    let mut indices = Vec::with_capacity(36);
    append_axis_aligned_cube(
        &mut vertices,
        &mut indices,
        Vec3::ZERO,
        CENTRE_CUBE_SIDE_LOCAL,
    );
    (vertices, indices)
}

/// Builds a 90° rotate-arc segment as a torus around +X, in
/// handle-local space. The arc is centred on +Y (azimuthal angle 0
/// in the YZ plane); the renderer composes a per-frame "roll" about
/// +X to swing the arc onto the camera-facing quadrant before drawing
/// it, then composes the per-axis world-rotation as for the translate
/// handle.
///
/// Vertices are laid out as a `(MAJOR_SEGMENTS + 1) × MINOR_SEGMENTS`
/// grid — major angle θ ∈ `[-π/4, +π/4]` centred on `+Y`, minor angle
/// φ ∈ `[0, 2π)` wrapping the tube. Triangle winding is CCW from the
/// outward (radial-plus-axial) direction.
#[allow(dead_code)]
pub fn build_rotate_arc_geometry() -> (Vec<GizmoVertex>, Vec<u32>) {
    let arc_span = std::f32::consts::FRAC_PI_2;
    let major_step = arc_span / ROTATE_ARC_MAJOR_SEGMENTS as f32;
    let minor_step = std::f32::consts::TAU / ROTATE_ARC_MINOR_SEGMENTS as f32;
    let r = ROTATE_ARC_RADIUS_LOCAL;
    let t = ROTATE_ARC_TUBE_LOCAL;

    let major = ROTATE_ARC_MAJOR_SEGMENTS;
    let minor = ROTATE_ARC_MINOR_SEGMENTS;

    let mut vertices = Vec::with_capacity(((major + 1) * minor) as usize);
    let mut indices = Vec::with_capacity((major * minor * 6) as usize);

    for i in 0..=major {
        let theta = -arc_span * 0.5 + i as f32 * major_step;
        let (st, ct) = theta.sin_cos();
        // Major-circle centre at (0, R cos θ, R sin θ). Outward
        // radial direction in the YZ plane is the unit form of that
        // centre; +X is the tangent along the major circle's plane.
        let centre = Vec3::new(0.0, r * ct, r * st);
        let radial = Vec3::new(0.0, ct, st);
        let axial = Vec3::X;
        for j in 0..minor {
            let phi = j as f32 * minor_step;
            let (sp, cp) = phi.sin_cos();
            let p = centre + radial * (t * cp) + axial * (t * sp);
            vertices.push(GizmoVertex {
                position: [p.x, p.y, p.z],
            });
        }
    }

    for i in 0..major {
        for j in 0..minor {
            let next_j = (j + 1) % minor;
            let a = i * minor + j;
            let b = i * minor + next_j;
            let c = (i + 1) * minor + j;
            let d = (i + 1) * minor + next_j;
            // Quad order (a, c, d, b): same convention as the shaft
            // prism. (∂p/∂θ × ∂p/∂φ) points outward at every (θ, φ),
            // and the corresponding triangle normal stays parallel to
            // it across the small Δθ × Δφ patch.
            indices.extend_from_slice(&[a, c, d, a, d, b]);
        }
    }

    (vertices, indices)
}

/// Appends a 6-face axis-aligned cube to `vertices` / `indices`,
/// centred at `centre`, with side length `side`. Eight shared
/// corner vertices, twelve triangles. CCW-from-outside winding so
/// the cube renders correctly under the same `cull_mode: Back` /
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
        [centre.x - h, centre.y - h, centre.z - h], // 0: -x -y -z
        [centre.x + h, centre.y - h, centre.z - h], // 1: +x -y -z
        [centre.x + h, centre.y + h, centre.z - h], // 2: +x +y -z
        [centre.x - h, centre.y + h, centre.z - h], // 3: -x +y -z
        [centre.x - h, centre.y - h, centre.z + h], // 4: -x -y +z
        [centre.x + h, centre.y - h, centre.z + h], // 5: +x -y +z
        [centre.x + h, centre.y + h, centre.z + h], // 6: +x +y +z
        [centre.x - h, centre.y + h, centre.z + h], // 7: -x +y +z
    ];
    for p in positions {
        vertices.push(GizmoVertex { position: p });
    }
    let face_indices: [u32; 36] = [
        4, 5, 6, 4, 6, 7, // +Z (top)
        0, 3, 2, 0, 2, 1, // -Z (bottom)
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
mod tests {
    use super::*;
    use float_cmp::assert_approx_eq;
    use glam::Vec3;

    /// Helper: classify a triangle's centroid into the kind of
    /// surface it belongs to (prism / cone / cube), so the
    /// outward-direction check can use the right rule for each.
    enum SurfaceKind {
        Prism,
        ConeSide,
        ConeTipApex,
        Cube { centre_x: f32 },
    }

    fn classify(centroid: Vec3) -> SurfaceKind {
        // Cube centroid lies inside `[midpoint − h, midpoint + h]³`
        // around the shaft midpoint at x = 0.5, with `side ≈ 0.083`.
        let cube_centre = SHAFT_LENGTH * MIDPOINT_FRACTION;
        let cube_half = MIDPOINT_CUBE_SIDE_LOCAL * 0.5 + 1e-3;
        if (centroid.x - cube_centre).abs() <= cube_half
            && centroid.y.abs() <= cube_half
            && centroid.z.abs() <= cube_half
        {
            return SurfaceKind::Cube {
                centre_x: cube_centre,
            };
        }
        if centroid.x >= CONE_BASE_X - 1e-3 {
            // Cone-tip apex triangles have centroids near +X with
            // small Y/Z; cone-side faces have a wider radial spread.
            if centroid.y.hypot(centroid.z) < SHAFT_RADIUS {
                return SurfaceKind::ConeTipApex;
            }
            return SurfaceKind::ConeSide;
        }
        SurfaceKind::Prism
    }

    #[test]
    fn vertex_count_is_shaft_plus_cone_plus_midpoint_cube() {
        let (v, _, _) = build_axis_geometry();
        // 8 sides × 2 ring vertices + 12 cone base + 1 tip + 8 cube corners.
        assert_eq!(v.len(), 8 * 2 + 12 + 1 + 8);
    }

    #[test]
    fn index_count_includes_midpoint_cube_triangles() {
        let (_, i, _) = build_axis_geometry();
        // 8 sides × 2 tris × 3 + 12 cone tris × 3 + 6 cube faces × 2 tris × 3.
        assert_eq!(i.len(), 8 * 6 + 12 * 3 + 6 * 2 * 3);
    }

    #[test]
    fn shaft_tip_index_count_marks_translate_subrange() {
        // The split between translate (shaft + cone) and scale
        // (midpoint cube) must match the legacy translate index
        // count so the renderer can issue two draw_indexed calls.
        let (_, _, shaft_tip) = build_axis_geometry();
        assert_eq!(shaft_tip, 8 * 6 + 12 * 3);
    }

    #[test]
    fn every_index_is_in_bounds() {
        let (v, indices, _) = build_axis_geometry();
        let max = v.len() as u32;
        for &idx in &indices {
            assert!(idx < max, "index {idx} out of range (vertices = {max})");
        }
    }

    #[test]
    fn bounding_box_matches_design() {
        let (v, _, _) = build_axis_geometry();
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for vert in &v {
            let p = Vec3::from(vert.position);
            min = min.min(p);
            max = max.max(p);
        }
        // X spans 0 → CONE_TIP_X. The cube extends ~0.04 on either
        // side of the midpoint at 0.5, well inside that range.
        assert_approx_eq!(f32, min.x, 0.0);
        assert_approx_eq!(f32, max.x, CONE_TIP_X);
        // Y/Z span the cone base radius (the wider of the two);
        // the cube's ±0.042 sits inside ±CONE_BASE_RADIUS = ±0.07.
        assert!(min.y >= -CONE_BASE_RADIUS - 1e-6);
        assert!(max.y <= CONE_BASE_RADIUS + 1e-6);
        assert!(min.z >= -CONE_BASE_RADIUS - 1e-6);
        assert!(max.z <= CONE_BASE_RADIUS + 1e-6);
    }

    #[test]
    fn every_triangle_winds_ccw_outward() {
        // For each triangle, compute the geometric normal and check
        // it points outward — outward direction depends on which
        // sub-surface the triangle belongs to (prism / cone /
        // midpoint cube).
        let (v, indices, _) = build_axis_geometry();
        for tri in indices.chunks_exact(3) {
            let (a, b, c) = (tri[0], tri[1], tri[2]);
            let p0 = Vec3::from(v[a as usize].position);
            let p1 = Vec3::from(v[b as usize].position);
            let p2 = Vec3::from(v[c as usize].position);
            let centroid = (p0 + p1 + p2) / 3.0;
            let n = (p1 - p0).cross(p2 - p0).normalize();

            match classify(centroid) {
                SurfaceKind::Prism | SurfaceKind::ConeSide => {
                    let radial = Vec3::new(0.0, centroid.y, centroid.z).normalize();
                    assert!(
                        n.dot(radial) > 0.0,
                        "triangle ({a}, {b}, {c}) winds inward — normal {n:?}"
                    );
                }
                SurfaceKind::ConeTipApex => {
                    assert!(
                        n.x > 0.0,
                        "tip-adjacent triangle should face +X (n = {n:?})"
                    );
                }
                SurfaceKind::Cube { centre_x } => {
                    // Outward direction = vector from cube centre to
                    // face centre; for an axis-aligned face the
                    // dominant axis component dictates the sign.
                    let outward =
                        Vec3::new(centroid.x - centre_x, centroid.y, centroid.z).normalize();
                    assert!(
                        n.dot(outward) > 0.0,
                        "cube triangle ({a}, {b}, {c}) winds inward — \
                         normal {n:?} vs outward {outward:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn shaft_ring_x_coords_are_pinned() {
        // The first 16 vertices form the prism's two rings — vertex
        // 2i sits at x=0 and 2i+1 at x=SHAFT_LENGTH. The cone-side
        // triangulation depends on this layout.
        let (v, _, _) = build_axis_geometry();
        for i in 0..SHAFT_SIDES as usize {
            assert_approx_eq!(f32, v[2 * i].position[0], 0.0);
            assert_approx_eq!(f32, v[2 * i + 1].position[0], SHAFT_LENGTH);
        }
    }

    #[test]
    fn cone_tip_sits_on_x_axis() {
        // The cone tip is the last vertex *before* the cube — the
        // cube appends 8 vertices after, so the apex is at len - 9.
        let (v, _, _) = build_axis_geometry();
        let tip = &v[v.len() - 1 - 8];
        assert_approx_eq!(f32, tip.position[0], CONE_TIP_X);
        assert_approx_eq!(f32, tip.position[1], 0.0);
        assert_approx_eq!(f32, tip.position[2], 0.0);
    }

    #[test]
    fn midpoint_cube_centre_sits_at_shaft_midpoint() {
        // Last 8 vertices are the cube corners. Their centroid is
        // the cube centre, which must equal `(SHAFT_LENGTH * 0.5, 0, 0)`.
        let (v, _, _) = build_axis_geometry();
        let cube = &v[v.len() - 8..];
        let mut centre = Vec3::ZERO;
        for vert in cube {
            centre += Vec3::from(vert.position);
        }
        centre /= cube.len() as f32;
        assert_approx_eq!(
            f32,
            centre.x,
            SHAFT_LENGTH * MIDPOINT_FRACTION,
            epsilon = 1e-5
        );
        assert_approx_eq!(f32, centre.y, 0.0, epsilon = 1e-5);
        assert_approx_eq!(f32, centre.z, 0.0, epsilon = 1e-5);
    }
}

#[cfg(test)]
mod centre_cube_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    #[test]
    fn vertex_and_index_counts_are_pinned() {
        let (v, i) = build_centre_cube_geometry();
        assert_eq!(v.len(), 8);
        assert_eq!(i.len(), 36);
    }

    #[test]
    fn cube_is_centred_on_origin() {
        let (v, _) = build_centre_cube_geometry();
        let mut centre = Vec3::ZERO;
        for vert in &v {
            centre += Vec3::from(vert.position);
        }
        centre /= v.len() as f32;
        assert_approx_eq!(f32, centre.x, 0.0, epsilon = 1e-6);
        assert_approx_eq!(f32, centre.y, 0.0, epsilon = 1e-6);
        assert_approx_eq!(f32, centre.z, 0.0, epsilon = 1e-6);
    }

    #[test]
    fn cube_extents_match_the_centre_cube_constant() {
        let (v, _) = build_centre_cube_geometry();
        let half = CENTRE_CUBE_SIDE_LOCAL * 0.5;
        for vert in &v {
            let p = Vec3::from(vert.position);
            assert_approx_eq!(f32, p.x.abs(), half, epsilon = 1e-6);
            assert_approx_eq!(f32, p.y.abs(), half, epsilon = 1e-6);
            assert_approx_eq!(f32, p.z.abs(), half, epsilon = 1e-6);
        }
    }

    #[test]
    fn every_cube_triangle_winds_ccw_outward() {
        let (v, indices) = build_centre_cube_geometry();
        for tri in indices.chunks_exact(3) {
            let p0 = Vec3::from(v[tri[0] as usize].position);
            let p1 = Vec3::from(v[tri[1] as usize].position);
            let p2 = Vec3::from(v[tri[2] as usize].position);
            let centroid = (p0 + p1 + p2) / 3.0;
            let n = (p1 - p0).cross(p2 - p0).normalize();
            // For a cube centred on origin, outward at any face's
            // centroid is the centroid direction itself.
            let outward = centroid.normalize();
            assert!(n.dot(outward) > 0.0, "cube triangle winds inward");
        }
    }
}

#[cfg(test)]
mod rotate_arc_tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    #[test]
    fn vertex_count_matches_grid_size() {
        let (v, _) = build_rotate_arc_geometry();
        let expected = (ROTATE_ARC_MAJOR_SEGMENTS + 1) * ROTATE_ARC_MINOR_SEGMENTS;
        assert_eq!(v.len() as u32, expected);
    }

    #[test]
    fn index_count_is_two_tris_per_quad() {
        let (_, i) = build_rotate_arc_geometry();
        let expected = ROTATE_ARC_MAJOR_SEGMENTS * ROTATE_ARC_MINOR_SEGMENTS * 6;
        assert_eq!(i.len() as u32, expected);
    }

    #[test]
    fn every_index_is_in_bounds() {
        let (v, indices) = build_rotate_arc_geometry();
        let max = v.len() as u32;
        for &idx in &indices {
            assert!(idx < max, "index {idx} out of range (vertices = {max})");
        }
    }

    #[test]
    fn arc_is_centred_on_plus_y_in_yz_plane() {
        // The 90° arc spans θ ∈ [-π/4, +π/4] around +X, centred on +Y.
        // The middle vertex ring (i = MAJOR/2) lies on the +Y axis.
        let (v, _) = build_rotate_arc_geometry();
        let mid = (ROTATE_ARC_MAJOR_SEGMENTS / 2) * ROTATE_ARC_MINOR_SEGMENTS;
        // The j = 0 vertex of that ring sits at the outer-radial
        // direction (φ = 0) → position centre + tube * radial.
        let p = Vec3::from(v[mid as usize].position);
        // Centre = (0, R, 0); radial = +Y; outer point = (0, R + t, 0).
        assert_approx_eq!(f32, p.x, 0.0, epsilon = 1e-5);
        assert_approx_eq!(
            f32,
            p.y,
            ROTATE_ARC_RADIUS_LOCAL + ROTATE_ARC_TUBE_LOCAL,
            epsilon = 1e-5
        );
        assert_approx_eq!(f32, p.z, 0.0, epsilon = 1e-5);
    }

    #[test]
    fn every_vertex_lies_on_the_torus_surface() {
        // For any vertex p, distance from the major circle (the
        // closest point on the YZ-plane circle of radius R) should
        // equal the tube radius.
        let (v, _) = build_rotate_arc_geometry();
        let r = ROTATE_ARC_RADIUS_LOCAL;
        let t = ROTATE_ARC_TUBE_LOCAL;
        for vert in &v {
            let p = Vec3::from(vert.position);
            // Project onto YZ plane to find the closest point on the
            // major circle.
            let yz_len = p.y.hypot(p.z);
            if yz_len < 1e-6 {
                // Vertex sits on the X axis — only possible if
                // |p.x| = t and the projection is at the major-
                // circle centre. Distance to the closest major-
                // circle point would be sqrt(R² + p.x²), not t.
                // The arc geometry never produces such vertices.
                panic!("unexpected vertex on the X axis: {p:?}");
            }
            let radial_diff = yz_len - r;
            let dist_to_circle = (radial_diff * radial_diff + p.x * p.x).sqrt();
            assert_approx_eq!(f32, dist_to_circle, t, epsilon = 1e-5);
        }
    }

    #[test]
    fn every_triangle_winds_ccw_outward() {
        // For each triangle, the geometric normal must point in the
        // same hemisphere as the outward direction at the centroid.
        // Outward = (centroid − closest_point_on_major_circle).
        let (v, indices) = build_rotate_arc_geometry();
        let r = ROTATE_ARC_RADIUS_LOCAL;
        for tri in indices.chunks_exact(3) {
            let p0 = Vec3::from(v[tri[0] as usize].position);
            let p1 = Vec3::from(v[tri[1] as usize].position);
            let p2 = Vec3::from(v[tri[2] as usize].position);
            let centroid = (p0 + p1 + p2) / 3.0;
            let n = (p1 - p0).cross(p2 - p0).normalize();

            // Closest point on the YZ-plane major circle to the
            // centroid: project to the YZ plane, scale to length R.
            let yz = Vec3::new(0.0, centroid.y, centroid.z);
            let closest = yz.normalize() * r;
            let outward = (centroid - closest).normalize();
            assert!(
                n.dot(outward) > 0.0,
                "torus triangle winds inward — normal {n:?} vs outward {outward:?}"
            );
        }
    }
}
