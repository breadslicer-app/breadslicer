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
use slint::wgpu_28::wgpu;

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
/// vec4-alignment padding.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub(super) struct Uniforms {
    pub(super) view_proj: [[f32; 4]; 4],
    pub(super) light_dir: [f32; 4],
    pub(super) env_min: [f32; 4],
    pub(super) env_max: [f32; 4],
    pub(super) face_breach_mask: [u32; 4],
}

/// World-space directional light. The fourth component is layout
/// padding; the mesh shader reads only `.xyz`.
pub(super) const LIGHT_DIR: [f32; 4] = [0.5, 0.7, 1.0, 0.0];

/// Clear colour for the main mesh pass. A near-black neutral so the
/// rendered model and gizmo handles read clearly without competing
/// with the background.
pub(super) const CLEAR_COLOR: wgpu::Color = wgpu::Color {
    r: 0.12,
    g: 0.12,
    b: 0.13,
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
    let proj = Mat4::perspective_rh(fov_y, aspect, NEAR_MULT * diagonal, FAR_MULT * diagonal);
    proj * view_mat
}
