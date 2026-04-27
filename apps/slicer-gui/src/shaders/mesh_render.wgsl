struct Uniforms {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,
};

struct ModelEntry {
    model: mat4x4<f32>,
    // Per-object flag word, written every frame. flags.x is reserved for the
    // selection bit (0 = not selected, 1 = selected); the remaining lanes are
    // free for later phases (hover, gizmo focus, …) and are currently zero.
    flags: vec4<u32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
// One uniform window per draw call. The CPU side binds the matching object's
// slot via a dynamic offset on this binding before each draw, so we read a
// plain ModelEntry here (not an array, no instance_index lookup) — that keeps
// the shader within the default-limits envelope of every backend Slint may
// pick (storage buffers in the vertex stage are not always available).
@group(1) @binding(0) var<uniform> model_entry: ModelEntry;

struct VsIn {
    @location(0) position: vec3<f32>,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let world = model_entry.model * vec4<f32>(in.position, 1.0);
    out.clip_pos = u.view_proj * world;
    out.world_pos = world.xyz;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Face normal from screen-space derivatives: no per-vertex normals needed.
    let n = normalize(cross(dpdx(in.world_pos), dpdy(in.world_pos)));
    let l = normalize(u.light_dir.xyz);
    // abs() makes lighting orientation-agnostic — safe for any winding.
    let lambert = abs(dot(n, l));
    let ambient = 0.25;
    let intensity = ambient + (1.0 - ambient) * lambert;
    let base_color = vec3<f32>(0.75, 0.78, 0.85);
    return vec4<f32>(base_color * intensity, 1.0);
}
