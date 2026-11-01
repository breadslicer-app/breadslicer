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
    //   flags.z = overlay tint (0 = a scene object; 1 valid preview,
    //             2 invalid preview, 3 hollow cavity, 4 drain, 5 preview
    //             not known yet), read by `fs_overlay` only
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
    // Smooth shading normal in the leaf frame; zero means shade flat.
    @location(1) normal: vec3<f32>,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    // `flat` interpolation: every fragment of every triangle of this
    // object reads the same value, so the mix factor is a clean 0/1
    // switch instead of crossing a smooth gradient between objects.
    @location(1) @interpolate(flat) selected: u32,
    @location(2) @interpolate(flat) breaches: u32,
    @location(3) @interpolate(flat) tint: u32,
    @location(4) normal: vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let world = model_entry.model * vec4<f32>(in.position, 1.0);
    out.clip_pos = u.view_proj * world;
    out.world_pos = world.xyz;
    out.selected = model_entry.flags.x;
    out.breaches = model_entry.flags.y;
    out.tint = model_entry.flags.z;
    // Direction only: the model matrix's linear part. Non-uniform scale
    // bends it slightly; the shading tolerates that.
    out.normal = (model_entry.model * vec4<f32>(in.normal, 0.0)).xyz;
    return out;
}

// Build-envelope warning palette, in the theme's highlight, the amber
// `accent` #d7a75a, as the plate's warning stripes. The "wash" is the
// mild amber applied across every fragment of an offending object; the
// "outside" amber is the full accent painted where a fragment's world
// position actually lies beyond the envelope corners.
const WARNING_WASH: vec3<f32> = vec3<f32>(0.92, 0.78, 0.55);
const WARNING_OUTSIDE: vec3<f32> = vec3<f32>(0.843, 0.655, 0.353);
const WARNING_WASH_STRENGTH: f32 = 0.55;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Slice clip: drop fragments above the selected layer's top Z. The
    // clip height rides in light_dir.w; f32::MAX (no clip) discards none.
    if (in.world_pos.z > u.light_dir.w) {
        discard;
    }
    // The interpolated smooth normal; a vertex without one shades flat from
    // screen-space derivatives.
    let flat_n = normalize(cross(dpdx(in.world_pos), dpdy(in.world_pos)));
    let has_normal = dot(in.normal, in.normal) > 1e-12;
    let n = select(flat_n, normalize(in.normal), has_normal);
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
    // is painted in the full warning amber instead of the wash.
    let p = in.world_pos;
    let lo = u.env_min.xyz;
    let hi = u.env_max.xyz;
    let outside = any(p < lo) || any(p > hi);
    let final_color = select(washed, WARNING_OUTSIDE, outside);
    return vec4<f32>(final_color * intensity, 1.0);
}

// Preview overlays (a support before it is placed, a hollow cavity, a drain):
// drawn over the scene without depth, translucent, so what lies inside or
// behind the model still shows. Lit like the models so its shape reads.
@fragment
fn fs_overlay(in: VsOut) -> @location(0) vec4<f32> {
    let n = normalize(cross(dpdx(in.world_pos), dpdy(in.world_pos)));
    let lambert = abs(dot(n, normalize(u.light_dir.xyz)));
    let intensity = 0.35 + 0.65 * lambert;
    var color = vec4<f32>(0.35, 0.85, 0.55, 0.70);
    if (in.tint == 2u) {
        color = vec4<f32>(1.00, 0.25, 0.25, 0.70);
    } else if (in.tint == 3u) {
        color = vec4<f32>(0.35, 0.60, 1.00, 0.40);
    } else if (in.tint == 4u) {
        // The interface's amber: a drain reads apart from the blue cavity.
        color = vec4<f32>(1.00, 0.62, 0.18, 0.85);
    } else if (in.tint == 5u) {
        // Neither green nor red: not known yet whether it works.
        color = vec4<f32>(0.80, 0.80, 0.84, 0.60);
    }
    return vec4<f32>(color.rgb * intensity, color.a);
}
