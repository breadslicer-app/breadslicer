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
    under_plate: vec4<f32>,
    eye: vec4<f32>,
    plates: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
// The plates of the scene (#545): xy = how far apart they stand, centre to
// centre, z = how many, w = the active one. Plate `i` stands at column
// i % 4 and row i / 4 from the first, which is centred on the origin.
fn plate_offset(i: u32) -> vec3<f32> {
    return vec3<f32>(f32(i % 4u) * u.plates.x, f32(i / 4u) * u.plates.y, 0.0);
}

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

// One instance per plate (#545): each plate's label and rulers by it.
@vertex
fn vs_main(in: VsIn, @builtin(instance_index) plate: u32) -> VsOut {
    var out: VsOut;
    out.clip_pos = u.view_proj * vec4<f32>(in.position + plate_offset(plate), 1.0);
    out.uv = in.uv;
    return out;
}

// Ink colours, from Breadslicer's theme tokens: the word, and the
// rulers' numbers, in `text-muted` #8b959d, so they read as markings on
// a drawing, and the printer's name below the word in `text-faint`
// #69737b.
const WORD_COLOUR: vec3<f32> = vec3<f32>(0.545, 0.584, 0.616);
const NAME_COLOUR: vec3<f32> = vec3<f32>(0.412, 0.451, 0.482);

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // The word's coverage is in red, the name's in green; the two
    // lines never share a texel.
    let coverage = textureSample(label_tex, label_samp, in.uv).rg;
    let colour = select(WORD_COLOUR, NAME_COLOUR, coverage.g > coverage.r);
    // Unlike the plate's faces, the writing stays from under the plate: its
    // copy facing down reads the right way round from there (#507).
    return vec4<f32>(colour, max(coverage.r, coverage.g));
}
