// Translate gizmo: three axis handles drawn after the main scene
// from a single axis-along-+X geometry, rotated and tinted per axis.
// No lighting — handles read as flat-shaded UI, not as 3D objects.

struct GizmoUniforms {
    view_proj: mat4x4<f32>,
    // xyz = world-space pivot (selected object's local-AABB centre,
    // transformed by its current Transform3D); w = world-unit
    // length of the axis shaft, derived from camera distance and
    // viewport height so the gizmo subtends a constant pixel size.
    pivot_and_scale: vec4<f32>,
};

struct DrawEntry {
    // Rotates handle-local +X onto the active world axis (identity
    // for X, +90° around Z for Y, −90° around Y for Z).
    axis_rotation: mat4x4<f32>,
    // Solid colour for this axis's draw call. Drives both visual
    // colour and the active/inactive/locked-out distinction —
    // brightness is modulated CPU-side.
    tint: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: GizmoUniforms;
// One entry-sized window; the CPU side rebinds with a dynamic
// offset for each of the three axis draw calls.
@group(1) @binding(0) var<uniform> entry: DrawEntry;

struct VsIn {
    @location(0) position: vec3<f32>,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) @interpolate(flat) tint: vec4<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let scaled = in.position * u.pivot_and_scale.w;
    let rotated = (entry.axis_rotation * vec4<f32>(scaled, 1.0)).xyz;
    let world = rotated + u.pivot_and_scale.xyz;
    out.clip_pos = u.view_proj * vec4<f32>(world, 1.0);
    out.tint = entry.tint;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.tint;
}
