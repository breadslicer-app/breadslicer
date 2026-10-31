/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

// Transform gizmo: handle meshes drawn after the main scene, each from one
// axis-along-+X geometry, rotated and tinted per draw. No lighting: handles
// read as flat UI with a dark outline, not as 3D objects. Alpha-blended into
// the multisampled colour target, so edges are antialiased.

struct GizmoUniforms {
    view_proj: mat4x4<f32>,
    // xyz = world-space pivot (selected object's local-AABB centre,
    // transformed by its current Transform3D); w = world-unit
    // length of the axis shaft, derived from camera distance and
    // viewport height so the gizmo subtends a constant pixel size.
    pivot_and_scale: vec4<f32>,
    // w = 1: xyz is the eye position. w = 0 (orthographic): xyz is the
    // direction toward the eye.
    eye: vec4<f32>,
};

struct DrawEntry {
    // Rotates handle-local +X onto the active world axis (identity
    // for X, +90° around Z for Y, -90° around Y for Z), plus any
    // per-frame slide of the handle.
    axis_rotation: mat4x4<f32>,
    // Colour and opacity of this draw (straight alpha).
    tint: vec4<f32>,
    // x = how far each vertex moves along its offset, in handle-local
    // units (the outline is the same mesh grown by its width).
    // y = which half to keep, seen from the eye: 0 all, 1 the front
    // half, 2 the back half (the faint back of a rotate ring).
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: GizmoUniforms;
// One entry-sized window; the CPU side rebinds with a dynamic
// offset for each draw call.
@group(1) @binding(0) var<uniform> entry: DrawEntry;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) offset: vec3<f32>,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world: vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let local = in.position + in.offset * entry.params.x;
    let scaled = local * u.pivot_and_scale.w;
    let rotated = (entry.axis_rotation * vec4<f32>(scaled, 1.0)).xyz;
    let world = rotated + u.pivot_and_scale.xyz;
    out.clip_pos = u.view_proj * vec4<f32>(world, 1.0);
    out.world = world;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let half = entry.params.y;
    if (half > 0.5) {
        let pivot = u.pivot_and_scale.xyz;
        let to_eye = select(u.eye.xyz, u.eye.xyz - pivot, u.eye.w > 0.5);
        let side = dot(normalize(in.world - pivot), normalize(to_eye));
        let back = side < -0.03;
        if (half < 1.5 && back) {
            discard;
        }
        if (half > 1.5 && !back) {
            discard;
        }
    }
    return entry.tint;
}
