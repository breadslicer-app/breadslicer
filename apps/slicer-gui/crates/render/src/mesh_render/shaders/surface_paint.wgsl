/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

// Paint on a model's faces (#183) and the brush's ring on its surface. Both
// draw the model's own vertices again over the lit model, at the depth it
// left, without writing depth: `fs_tint` the painted faces alone, `fs_ring`
// the whole model, so the ring lies on the surface and bends with it.

struct Uniforms {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,
    env_min: vec4<f32>,
    env_max: vec4<f32>,
    face_breach_mask: vec4<u32>,
};

struct ModelEntry {
    model: mat4x4<f32>,
    flags: vec4<u32>,
};

struct PaintParams {
    // The paint's colour; .a is how much it covers the model.
    tint: vec4<f32>,
    // The ring: its centre in world mm, and its radius.
    ring: vec4<f32>,
    ring_color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(1) @binding(0) var<uniform> model_entry: ModelEntry;
@group(2) @binding(0) var<uniform> paint: PaintParams;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let world = model_entry.model * vec4<f32>(in.position, 1.0);
    out.clip_pos = u.view_proj * world;
    out.world_pos = world.xyz;
    out.normal = (model_entry.model * vec4<f32>(in.normal, 0.0)).xyz;
    return out;
}

// The light the model shader gives this fragment, so the paint keeps the
// shape of what lies under it.
fn lit(in: VsOut) -> f32 {
    let flat_n = normalize(cross(dpdx(in.world_pos), dpdy(in.world_pos)));
    let has_normal = dot(in.normal, in.normal) > 1e-12;
    let n = select(flat_n, normalize(in.normal), has_normal);
    let lambert = abs(dot(n, normalize(u.light_dir.xyz)));
    return 0.25 + 0.75 * lambert;
}

@fragment
fn fs_tint(in: VsOut) -> @location(0) vec4<f32> {
    let intensity = lit(in);
    if (in.world_pos.z > u.light_dir.w) {
        discard;
    }
    return vec4<f32>(paint.tint.rgb * intensity, paint.tint.a);
}

// How wide the ring's line is, in pixels.
const RING_WIDTH_PX: f32 = 1.6;
// How much the area inside the ring shows.
const RING_FILL: f32 = 0.10;

@fragment
fn fs_ring(in: VsOut) -> @location(0) vec4<f32> {
    let d = distance(in.world_pos, paint.ring.xyz);
    let per_pixel = max(fwidth(d), 1e-6);
    if (in.world_pos.z > u.light_dir.w) {
        discard;
    }
    let from_line = abs(d - paint.ring.w) / per_pixel;
    let line = 1.0 - smoothstep(RING_WIDTH_PX * 0.5, RING_WIDTH_PX * 0.5 + 1.0, from_line);
    let inside = RING_FILL * (1.0 - smoothstep(-1.0, 0.0, (d - paint.ring.w) / per_pixel));
    let alpha = max(line, inside) * paint.ring_color.a;
    if (alpha <= 0.002) {
        discard;
    }
    return vec4<f32>(paint.ring_color.rgb, alpha);
}
