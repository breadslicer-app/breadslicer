/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Per-frame data for the mesh + gizmo passes: the `Uniforms` struct
//! uploaded to the mesh pipeline, the view-projection matrix shared
//! between the mesh and gizmo passes, and the named constants the
//! renderer references each frame (clear colour, light direction).

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use slint::wgpu_30::wgpu;

use crate::camera::OrbitCamera;

/// Mesh-pipeline per-frame uniforms. Layout matches
/// `shaders/mesh_render.wgsl`'s `Uniforms` struct (16-byte-aligned
/// fields; `light_dir`, `env_min`, `env_max` are padded to `vec4` so
/// the shader can read `.xyz` and ignore the trailing component).
/// `env_min` / `env_max` carry the printer's build envelope corners
/// in world-space mm so the fragment shader can mark fragments that
/// fall outside the envelope. `face_breach_mask.x` is a 5-bit field,
/// one bit per envelope side wall + top, telling the plate shader
/// which walls' warning stripes to draw; the other lanes are
/// vec4-alignment padding. `overhang` carries the overhang map's terms
/// ([`super::overhang_map::OverhangMap::uniform`]), read only for the
/// objects the map paints. The view from under the plate (#481) rides in
/// the `.w` lanes of the envelope corners: `env_min[3]` is how far the
/// plate and the rafts have faded ([`crate::under_plate::under_plate_fade`]),
/// `env_max[3]` the height below which a support mesh fades with them
/// ([`crate::under_plate::hidden_below_mm`]).
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub(super) struct Uniforms {
    pub(super) view_proj: [[f32; 4]; 4],
    pub(super) light_dir: [f32; 4],
    pub(super) env_min: [f32; 4],
    pub(super) env_max: [f32; 4],
    pub(super) face_breach_mask: [u32; 4],
    pub(super) overhang: [f32; 4],
    /// From under the plate, the supports drawn as lines (#493): the height
    /// above which a support mesh fades out, half a dash's width in pixels
    /// and the target's size.
    pub(super) under_plate: [f32; 4],
    /// The eye (#523): `w = 1` its position, `w = 0` (orthographic) the
    /// direction toward it, for the highlight and the rim.
    pub(super) eye: [f32; 4],
}

/// World-space directional light. The fourth component carries the
/// slice clip height (see [`pack_light_dir`]); the mesh shader reads
/// `.xyz` for lighting and `.w` for the clip test.
pub(super) const LIGHT_DIR: [f32; 4] = [0.5, 0.7, 1.0, 0.0];

/// Build the mesh uniform's `light_dir` vector, packing the optional
/// slice clip height into the free `.w` lane. `None` uses `f32::MAX`, a
/// height no world fragment exceeds, so the shader's `world_pos.z > .w`
/// test discards nothing.
pub(super) fn pack_light_dir(clip_z: Option<f32>) -> [f32; 4] {
    [
        LIGHT_DIR[0],
        LIGHT_DIR[1],
        LIGHT_DIR[2],
        clip_z.unwrap_or(f32::MAX),
    ]
}

/// Clear colour for the main mesh pass: the theme's `ground` #111315, so the
/// view and the panels around it are one surface (#507).
pub(super) const CLEAR_COLOR: wgpu::Color = wgpu::Color {
    r: 0.067,
    g: 0.075,
    b: 0.082,
    a: 1.0,
};

/// Near-plane multiplier as a fraction of the scene's bounding-box
/// diagonal. Keeps near far enough to avoid depth-precision crunch
/// even at extreme zoom-in.
const NEAR_MULT: f32 = 0.01;

/// Far-plane multiplier as a fraction of the scene's bounding-box
/// diagonal. Generous enough that orbiting out doesn't clip the
/// scene's far side.
const FAR_MULT: f32 = 100.0;

/// Compute the view-projection matrix for the current frame. The
/// near/far planes scale with the scene's bounding diagonal so depth
/// precision remains stable across scenes of wildly different
/// physical size.
pub(super) fn compute_view_proj(
    camera: &OrbitCamera,
    aabb_min: Vec3,
    aabb_max: Vec3,
    width: u32,
    height: u32,
    fov_y: f32,
) -> Mat4 {
    let diagonal = (aabb_max - aabb_min).length().max(1e-3);
    let aspect = width as f32 / height as f32;
    let view_mat = camera.view_matrix();
    let near = (NEAR_MULT * diagonal).min(camera.distance * 0.01).max(1e-5);
    let proj = camera.projection(fov_y, aspect, near, FAR_MULT * diagonal);
    proj * view_mat
}

#[cfg(test)]
mod pack_light_dir_tests {
    use super::*;

    #[test]
    fn none_uses_a_clip_height_no_fragment_exceeds() {
        let ld = pack_light_dir(None);
        assert_eq!(
            [ld[0], ld[1], ld[2]],
            [LIGHT_DIR[0], LIGHT_DIR[1], LIGHT_DIR[2]]
        );
        assert_eq!(ld[3], f32::MAX);
    }

    #[test]
    fn some_packs_the_clip_height_into_the_w_lane() {
        let ld = pack_light_dir(Some(3.5));
        assert_eq!(
            [ld[0], ld[1], ld[2]],
            [LIGHT_DIR[0], LIGHT_DIR[1], LIGHT_DIR[2]]
        );
        assert_eq!(ld[3], 3.5);
    }
}
