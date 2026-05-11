/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

struct Uniforms {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,
    // World-space mm envelope corners. .xyz hold the values, .w is
    // padding for std140-style 16-byte alignment.
    env_min: vec4<f32>,
    env_max: vec4<f32>,
    // face_breach_mask.x is a 5-bit field consumed by the plate
    // shader's warning-wall path; the mesh shader does not read it.
    // Declared here so this WGSL struct matches the host Rust
    // `Uniforms` layout (the buffer is shared with the plate pipeline).
    face_breach_mask: vec4<u32>,
};

struct ModelEntry {
    model: mat4x4<f32>,
    // Per-object flag word, written every frame.
    //   flags.x = selection bit (0 = not selected, 1 = selected)
    //   flags.y = envelope-breach bit (1 if any vertex of this object
    //             falls outside the build envelope; drives the mild
    //             interior wash so the object reads as "offending"
    //             even on fragments that are still inside)
    // Remaining lanes are free for later phases (hover, gizmo focus, ...).
    flags: vec4<u32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
// One uniform window per draw call. The CPU side binds the matching object's
// slot via a dynamic offset on this binding before each draw, so we read a
// plain ModelEntry here (not an array, no instance_index lookup); that keeps
// the shader within the default-limits envelope of every backend Slint may
// pick (storage buffers in the vertex stage are not always available).
@group(1) @binding(0) var<uniform> model_entry: ModelEntry;

struct VsIn {
    @location(0) position: vec3<f32>,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    // `flat` interpolation: every fragment of every triangle of this
    // object reads the same value, so the mix factor is a clean 0/1
    // switch instead of crossing a smooth gradient between objects.
    @location(1) @interpolate(flat) selected: u32,
    @location(2) @interpolate(flat) breaches: u32,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let world = model_entry.model * vec4<f32>(in.position, 1.0);
    out.clip_pos = u.view_proj * world;
    out.world_pos = world.xyz;
    out.selected = model_entry.flags.x;
    out.breaches = model_entry.flags.y;
    return out;
}

// Build-envelope warning palette. The "wash" is the mild pink applied
// across every fragment of an offending object; the "outside" red is
// the saturated colour painted where a fragment's world position
// actually lies beyond the envelope corners.
const WARNING_WASH: vec3<f32> = vec3<f32>(0.95, 0.55, 0.55);
const WARNING_RED: vec3<f32> = vec3<f32>(1.00, 0.15, 0.15);
const WARNING_WASH_STRENGTH: f32 = 0.55;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Face normal from screen-space derivatives: no per-vertex normals needed.
    let n = normalize(cross(dpdx(in.world_pos), dpdy(in.world_pos)));
    let l = normalize(u.light_dir.xyz);
    // abs() makes lighting orientation-agnostic, safe for any winding.
    let lambert = abs(dot(n, l));
    let ambient = 0.25;
    let intensity = ambient + (1.0 - ambient) * lambert;
    let unselected = vec3<f32>(0.75, 0.78, 0.85);
    let highlighted = vec3<f32>(1.00, 0.78, 0.42);
    let base_color = mix(unselected, highlighted, f32(in.selected));
    let washed = mix(base_color, WARNING_WASH, f32(in.breaches) * WARNING_WASH_STRENGTH);
    // Componentwise envelope test: a fragment outside on any axis
    // is painted in the saturated warning red instead of the wash.
    let p = in.world_pos;
    let lo = u.env_min.xyz;
    let hi = u.env_max.xyz;
    let outside = any(p < lo) || any(p > hi);
    let final_color = select(washed, WARNING_RED, outside);
    return vec4<f32>(final_color * intensity, 1.0);
}
