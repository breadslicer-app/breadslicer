/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

// Front label: the word the plate's front tab carries, drawn as one
// quad at Z = 0 and textured with the label's coverage raster. Shares
// the mesh group-0 uniform buffer for `view_proj`; the quad's corners
// arrive as world-space millimetres on its own vertex buffer.

struct Uniforms {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,
    env_min: vec4<f32>,
    env_max: vec4<f32>,
    face_breach_mask: vec4<u32>,
    overhang: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(1) @binding(0) var label_tex: texture_2d<f32>;
@group(1) @binding(1) var label_samp: sampler;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip_pos = u.view_proj * vec4<f32>(in.position, 1.0);
    out.uv = in.uv;
    return out;
}

// Ink colours, from Breadslicer's theme tokens: the word in
// `text-muted` #8d959b, so it reads as a marking on the machine, and
// the printer's name below it in the highlight, `accent` #d7a75a.
const WORD_COLOUR: vec3<f32> = vec3<f32>(0.553, 0.584, 0.608);
const NAME_COLOUR: vec3<f32> = vec3<f32>(0.843, 0.655, 0.353);

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // The word's coverage is in red, the name's in green; the two
    // lines never share a texel.
    let coverage = textureSample(label_tex, label_samp, in.uv).rg;
    let colour = select(WORD_COLOUR, NAME_COLOUR, coverage.g > coverage.r);
    // The tab is the plate's: it fades with it from under the plate (#481).
    return vec4<f32>(colour, max(coverage.r, coverage.g) * (1.0 - u.env_min.w));
}
