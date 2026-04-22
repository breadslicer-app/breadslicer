struct Uniforms {
    mvp: mat4x4<f32>,
    light_dir: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

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
    out.clip_pos = u.mvp * vec4<f32>(in.position, 1.0);
    // Model transform is identity for now; pass position through as world.
    out.world_pos = in.position;
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
