/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Procedural mesh for one gizmo axis: an 8-sided prism (the shaft)
//! capped with a 12-sided cone (the tip), in **handle-local space**
//! — pivot at origin, axis along +X. The renderer draws this buffer
//! three times, rotating it onto +X, +Y, +Z; the per-axis tint
//! uniform colours each draw.
//!
//! Vertex attribute is position only — every vertex would be white
//! under the per-axis tint approach, so a colour attribute would be
//! pure bandwidth. Easy to add back if a future polish item wants
//! per-vertex shading (e.g. a brighter tip).

use bytemuck::{Pod, Zeroable};

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

/// Builds the gizmo axis geometry once. Caller is expected to upload
/// the result to a static VBO/IBO at `Renderer::new` time and reuse
/// it for every frame, every axis, every selection.
pub fn build_axis_geometry() -> (Vec<GizmoVertex>, Vec<u32>) {
    let mut vertices: Vec<GizmoVertex> =
        Vec::with_capacity((SHAFT_SIDES * 2 + CONE_SIDES + 1) as usize);
    let mut indices: Vec<u32> = Vec::with_capacity((SHAFT_SIDES * 6 + CONE_SIDES * 3) as usize);

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

    (vertices, indices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use float_cmp::assert_approx_eq;
    use glam::Vec3;

    #[test]
    fn vertex_count_is_two_rings_plus_cone_base_plus_tip() {
        let (v, _) = build_axis_geometry();
        // 8 sides × 2 ring vertices + 12 cone base + 1 tip.
        assert_eq!(v.len(), 8 * 2 + 12 + 1);
    }

    #[test]
    fn index_count_is_two_tris_per_side_plus_one_per_cone_face() {
        let (_, i) = build_axis_geometry();
        // 8 sides × 2 tris × 3 indices + 12 cone tris × 3 indices.
        assert_eq!(i.len(), 8 * 6 + 12 * 3);
    }

    #[test]
    fn every_index_is_in_bounds() {
        let (v, indices) = build_axis_geometry();
        let max = v.len() as u32;
        for &idx in &indices {
            assert!(idx < max, "index {idx} out of range (vertices = {max})");
        }
    }

    #[test]
    fn bounding_box_matches_design() {
        let (v, _) = build_axis_geometry();
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for vert in &v {
            let p = Vec3::from(vert.position);
            min = min.min(p);
            max = max.max(p);
        }
        // X spans 0 → CONE_TIP_X, with a small "below 0" margin
        // possible if SHAFT_RADIUS clipping nudged things — it
        // doesn't, so min.x must be exactly 0.0.
        assert_approx_eq!(f32, min.x, 0.0);
        assert_approx_eq!(f32, max.x, CONE_TIP_X);
        // Y/Z span the cone base radius (the wider of the two).
        assert!(min.y >= -CONE_BASE_RADIUS - 1e-6);
        assert!(max.y <= CONE_BASE_RADIUS + 1e-6);
        assert!(min.z >= -CONE_BASE_RADIUS - 1e-6);
        assert!(max.z <= CONE_BASE_RADIUS + 1e-6);
    }

    #[test]
    fn every_triangle_winds_ccw_outward() {
        // For each triangle, compute the geometric normal and check
        // it points outward — for the prism, outward is the radial
        // direction at the triangle's centroid; for the cone, the
        // outward direction is the outward face-normal of a cone,
        // which has a component along +radial and a small +X
        // component (toward the tip).
        let (v, indices) = build_axis_geometry();
        for tri in indices.chunks_exact(3) {
            let (a, b, c) = (tri[0], tri[1], tri[2]);
            let p0 = Vec3::from(v[a as usize].position);
            let p1 = Vec3::from(v[b as usize].position);
            let p2 = Vec3::from(v[c as usize].position);
            let centroid = (p0 + p1 + p2) / 3.0;
            let n = (p1 - p0).cross(p2 - p0).normalize();

            // Outward radial at the centroid (project to YZ plane).
            let radial = Vec3::new(0.0, centroid.y, centroid.z);
            // For the cone tip apex centroid, radial is essentially
            // zero — fall back to checking +X dominates the normal,
            // which is the correct outward direction for a vertex
            // sitting on the X-axis itself.
            if radial.length() < 1e-6 {
                assert!(
                    n.x > 0.0,
                    "tip-adjacent triangle should face away from -X (n = {n:?})"
                );
                continue;
            }
            let radial = radial.normalize();
            assert!(
                n.dot(radial) > 0.0,
                "triangle ({a}, {b}, {c}) winds inward — normal {n:?} vs outward radial {radial:?}"
            );
        }
    }

    #[test]
    fn shaft_ring_x_coords_are_pinned() {
        // The first 16 vertices form the prism's two rings — vertex
        // 2i sits at x=0 and 2i+1 at x=SHAFT_LENGTH. The cone-side
        // triangulation depends on this layout.
        let (v, _) = build_axis_geometry();
        for i in 0..SHAFT_SIDES as usize {
            assert_approx_eq!(f32, v[2 * i].position[0], 0.0);
            assert_approx_eq!(f32, v[2 * i + 1].position[0], SHAFT_LENGTH);
        }
    }

    #[test]
    fn cone_tip_sits_on_x_axis() {
        let (v, _) = build_axis_geometry();
        let tip = &v[v.len() - 1];
        assert_approx_eq!(f32, tip.position[0], CONE_TIP_X);
        assert_approx_eq!(f32, tip.position[1], 0.0);
        assert_approx_eq!(f32, tip.position[2], 0.0);
    }
}
