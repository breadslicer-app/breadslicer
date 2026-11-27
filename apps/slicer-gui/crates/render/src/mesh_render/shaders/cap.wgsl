/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

// Slice-view cap: the selected layer's polygons as triangles, flat at
// the clip height (#776). It fills the opening left by the mesh clip so
// the cut reads solid. Shares the mesh group-0 uniform buffer; the clip
// height rides in light_dir.w.

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
// The plates of the scene (#545): xy = how far apart they stand. The cut
// shows on the plate the slice was cut for, the instance drawn (#547).
fn plate_offset(i: u32) -> vec2<f32> {
    return vec2<f32>(f32(i % 4u) * u.plates.x, f32(i / 4u) * u.plates.y);
}
struct VsIn {
    // Plate-0 world millimetres, and 1 on a finding's pixels (#409).
    @location(0) pos: vec3<f32>,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) highlight: f32,
};

@vertex
fn vs_main(in: VsIn, @builtin(instance_index) plate: u32) -> VsOut {
    let world_xy = in.pos.xy + plate_offset(plate);
    var out: VsOut;
    out.clip_pos = u.view_proj * vec4<f32>(world_xy, u.light_dir.w, 1.0);
    out.highlight = in.pos.z;
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
    if (in.highlight >= 0.5) {
        return vec4<f32>(HIGHLIGHT_COLOR, 1.0);
    }
    return vec4<f32>(CAP_COLOR, 1.0);
}
