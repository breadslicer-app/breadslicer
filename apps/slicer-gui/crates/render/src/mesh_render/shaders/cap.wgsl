/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

// Slice-view cap: a flat quad spanning the build area at the clip
// height, textured with the selected layer's coverage raster. It fills
// the opening left by the mesh clip so the cut reads solid. Shares the
// mesh group-0 uniform buffer; the clip height rides in light_dir.w and
// the build-area corners in env_min / env_max.

struct Uniforms {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,
    env_min: vec4<f32>,
    env_max: vec4<f32>,
    face_breach_mask: vec4<u32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(1) @binding(0) var cap_tex: texture_2d<f32>;
@group(1) @binding(1) var cap_samp: sampler;

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    // Two triangles over the unit square; uv = fraction across the
    // build area, with (0, 0) at the minimum-X, minimum-Y corner to
    // match the coverage raster's bottom-to-top row order.
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    let c = corners[vid];
    let world_xy = mix(u.env_min.xy, u.env_max.xy, c);
    let clip_z = u.light_dir.w;

    var out: VsOut;
    out.clip_pos = u.view_proj * vec4<f32>(world_xy, clip_z, 1.0);
    out.uv = c;
    return out;
}

// Flat cap colour: the mesh's unselected base tint, so the cap reads as
// the same material as the shell it closes.
const CAP_COLOR: vec3<f32> = vec3<f32>(0.75, 0.78, 0.85);

// The highlight of a review finding's pixels (#409), the panel's
// HIGHLIGHT_RGB (230, 60, 170).
const HIGHLIGHT_COLOR: vec3<f32> = vec3<f32>(0.902, 0.235, 0.667);

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Red: coverage. Green: the highlight.
    let texel = textureSample(cap_tex, cap_samp, in.uv);
    if (texel.r < 0.5) {
        discard;
    }
    if (texel.g >= 0.5) {
        return vec4<f32>(HIGHLIGHT_COLOR, 1.0);
    }
    return vec4<f32>(CAP_COLOR, 1.0);
}
