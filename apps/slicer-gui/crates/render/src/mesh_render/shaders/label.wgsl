/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

// The writing on the plate: the word its front tab carries, the rulers'
// numbers and the plates' names (#545), each drawn as quads at Z = 0 and
// textured with distance fields: each texel says how far its centre is from
// the ink's edge, so the edge stays sharp however close the camera comes.
// Shares the mesh group-0 uniform buffer for `view_proj`; the quads' corners
// arrive as world-space millimetres on their own vertex buffer.

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
    // How opaque the writing is: fainter by an inactive plate (#545).
    @location(1) @interpolate(flat) shown: f32,
};

// One instance per plate (#545): each plate's label and rulers by it.
@vertex
fn vs_main(in: VsIn, @builtin(instance_index) plate: u32) -> VsOut {
    var out: VsOut;
    out.clip_pos = u.view_proj * vec4<f32>(in.position + plate_offset(plate), 1.0);
    out.shown = select(1.0, 0.45, u.plates.z > 1.5 && f32(plate) != u.plates.w);
    out.uv = in.uv;
    return out;
}

// Ink colours, from Breadslicer's theme tokens: the word, and the
// rulers' numbers, in `text-muted` #8b959d, so they read as markings on
// a drawing, and the printer's name below the word in `text-faint`
// #69737b. Text selected in a plate's name being edited lies on the
// selection's amber.
const WORD_COLOUR: vec3<f32> = vec3<f32>(0.545, 0.584, 0.616);
const NAME_COLOUR: vec3<f32> = vec3<f32>(0.412, 0.451, 0.482);
const SELECTION_COLOUR: vec3<f32> = vec3<f32>(1.0, 0.68, 0.28);
const SELECTION_ALPHA: f32 = 0.38;

// How far, in texels, a field reaches either side of an edge, as the
// rasterisers write it (`text_sdf::SPREAD`): a byte of 0.5 is on the edge.
const SPREAD: f32 = 6.0;

// How much of this pixel each field's ink covers: its distance from the
// edge over how far one pixel of the screen spans in it, along the way the
// distance changes, so the edge is one pixel soft from any angle.
fn covered(field: vec3<f32>) -> vec3<f32> {
    let d = (field - vec3<f32>(0.5)) * (2.0 * SPREAD);
    let dx = dpdx(d);
    let dy = dpdy(d);
    let per_pixel = max(sqrt(dx * dx + dy * dy), vec3<f32>(1e-4));
    return clamp(d / per_pixel + vec3<f32>(0.5), vec3<f32>(0.0), vec3<f32>(1.0));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // The word's field is in red, the name's in green, and the two lines
    // never share a texel; a plate's name has its caret in red, its field's
    // outline in green and its selection in blue.
    let a = covered(textureSample(label_tex, label_samp, in.uv).rgb);
    let ink = max(a.r, a.g);
    let colour = select(WORD_COLOUR, NAME_COLOUR, a.g > a.r);
    let under = a.b * SELECTION_ALPHA * (1.0 - ink);
    let alpha = ink + under;
    let rgb = (colour * ink + SELECTION_COLOUR * under) / max(alpha, 1e-4);
    // Unlike the plate's faces, the writing stays from under the plate: its
    // copy facing down reads the right way round from there (#507).
    return vec4<f32>(rgb, alpha * in.shown);
}
