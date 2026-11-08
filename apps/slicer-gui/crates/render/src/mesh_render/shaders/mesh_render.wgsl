/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

struct Uniforms {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,
    // World-space mm envelope corners in .xyz. From under the plate
    // (#481), env_min.w is how far the plate and the rafts have faded
    // (0 to 1) and env_max.w the height below which a support mesh hides
    // with them, mm: the rafts' top and a hair.
    env_min: vec4<f32>,
    env_max: vec4<f32>,
    // face_breach_mask.x is a 5-bit field consumed by the plate
    // shader's warning-wall path; the mesh shader does not read it.
    // Declared here so this WGSL struct matches the host Rust
    // `Uniforms` layout (the buffer is shared with the plate pipeline).
    face_breach_mask: vec4<u32>,
    // The overhang map's terms, read only for an object whose flags.z asks
    // for it: x = the world normal Z under which the support engine calls a
    // face an overhang, y = its self-support angle in radians from horizontal,
    // z = the world height up to which a face rests on the plate, w = the
    // side of the cells the support tips are hashed in, mm (0 without
    // tips): what a tip holds, the map leaves plain (#484).
    overhang: vec4<f32>,
};

struct ModelEntry {
    model: mat4x4<f32>,
    // Per-object flag word, written every frame.
    //   flags.x = selection bit (0 = not selected, 1 = selected)
    //   flags.y = envelope-breach bit (1 if any vertex of this object
    //             falls outside the build envelope; drives the mild
    //             interior wash so the object reads as "offending"
    //             even on fragments that are still inside)
    //   flags.z = what the object is drawn as: 0 a scene object, 6 a scene
    //             object under the overhang map (read by `fs_main`); overlay
    //             tints, read by `fs_overlay`: 1 valid preview, 2 invalid
    //             preview, 3 hollow cavity, 4 drain, 5 preview not known yet
    //   flags.w = 1 for a support mesh, whose raft fades from under the
    //             plate (#481)
    flags: vec4<u32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
// The support tips the overhang map counts as holding a face (#484), as
// data textures, which every backend allows where storage buffers are not:
// per hash slot, where its tips start and how many there are; and each tip's
// centre with its reach squared, sorted by slot.
@group(0) @binding(1) var tip_slots: texture_2d<u32>;
@group(0) @binding(2) var tip_points: texture_2d<f32>;
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
    @location(5) @interpolate(flat) supports: u32,
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
    out.supports = model_entry.flags.w;
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

// The overhang map (#91). `MODEL_OVERHANG_MAP` is `flags.z` of an object it
// paints. A face past the engine's limit jumps straight to the coral, so the
// edge of the red is the limit itself, and sinks to the crimson as it nears a
// flat ceiling; the rose fades in over the last degrees on the safe side, to
// show what a little more tilt would tip over. Neither is the amber of the
// selection and the out-of-volume warning.
const MODEL_OVERHANG_MAP: u32 = 6u;
const OVERHANG_EDGE: vec3<f32> = vec3<f32>(0.93, 0.30, 0.25);
const OVERHANG_CEILING: vec3<f32> = vec3<f32>(0.60, 0.06, 0.13);
const OVERHANG_STRENGTH: f32 = 0.92;
const OVERHANG_NEAR: vec3<f32> = vec3<f32>(0.90, 0.48, 0.52);
const OVERHANG_NEAR_STRENGTH: f32 = 0.60;
const OVERHANG_NEAR_BAND_RAD: f32 = 0.17453292; // 10°
// How much of the selection's amber stays on a mapped object: its outline
// still marks it, and a full amber fill would swallow the map's warm colours.
const OVERHANG_SELECTED_TINT: f32 = 0.30;

// `base` with the overhang map over it. `face_n` is the true face normal, the
// one the support engine classifies: the smooth shading normal rounds a facet
// off and would disagree with it, and the screen derivatives it comes from
// hold under any model matrix, a non-uniform scale included. A face resting on
// the plate is left alone: the plate holds it, and the engine's supports could
// not go under it anyway. A degenerate face has a NaN normal, and every
// comparison below is false for it.
fn overhang_paint(base: vec3<f32>, face_n: vec3<f32>, p: vec3<f32>) -> vec3<f32> {
    if (p.z <= u.overhang.z) {
        return base;
    }
    let limit = u.overhang.y;
    let from_horizontal = acos(clamp(-face_n.z, -1.0, 1.0));
    let near = 1.0 - (from_horizontal - limit) / OVERHANG_NEAR_BAND_RAD;
    if ((face_n.z < u.overhang.x || near > 0.0) && held_by_a_tip(p)) {
        return base;
    }
    if (face_n.z < u.overhang.x) {
        let ceiling = saturate(1.0 - from_horizontal / limit);
        return mix(base, mix(OVERHANG_EDGE, OVERHANG_CEILING, ceiling), OVERHANG_STRENGTH);
    }
    if (near > 0.0) {
        return mix(base, OVERHANG_NEAR, near * near * OVERHANG_NEAR_STRENGTH);
    }
    return base;
}

// The hash of a tip cell; `tip_coverage::cell_hash` on the CPU.
fn tip_cell_hash(cell: vec3<i32>) -> u32 {
    let c = bitcast<vec3<u32>>(cell);
    return (c.x * 73856093u) ^ (c.y * 19349663u) ^ (c.z * 83492791u);
}

// Whether a support tip holds the point `p` (#484): one of the tips hashed in
// the cells within the longest reach of it stands within its own reach. The
// cells are twice that reach wide, so at most two per axis.
// `CoverageGrid::holds` does the same on the CPU.
fn held_by_a_tip(p: vec3<f32>) -> bool {
    let cell = u.overhang.w;
    if (!(cell > 0.0)) {
        return false;
    }
    let slots_size = textureDimensions(tip_slots);
    let mask = slots_size.x * slots_size.y - 1u;
    let tips_width = textureDimensions(tip_points).x;
    let look = cell * 0.5 / (1.0 + TIP_CELL_MARGIN) * (1.0 + TIP_LOOK_MARGIN);
    let lo = vec3<i32>(floor((p - vec3<f32>(look)) / cell));
    let hi = min(vec3<i32>(floor((p + vec3<f32>(look)) / cell)), lo + vec3<i32>(1));
    for (var x = lo.x; x <= hi.x; x = x + 1) {
        for (var y = lo.y; y <= hi.y; y = y + 1) {
            for (var z = lo.z; z <= hi.z; z = z + 1) {
                let slot = tip_cell_hash(vec3<i32>(x, y, z)) & mask;
                let range = textureLoad(
                    tip_slots,
                    vec2<u32>(slot % slots_size.x, slot / slots_size.x),
                    0,
                ).xy;
                for (var k = 0u; k < range.y; k = k + 1u) {
                    let j = range.x + k;
                    let tip = textureLoad(tip_points, vec2<u32>(j % tips_width, j / tips_width), 0);
                    let d = p - tip.xyz;
                    if (dot(d, d) <= tip.w) {
                        return true;
                    }
                }
            }
        }
    }
    return false;
}

// `tip_coverage::CELL_MARGIN` and `LOOK_MARGIN`.
const TIP_CELL_MARGIN: f32 = 0.001;
const TIP_LOOK_MARGIN: f32 = 0.0005;

// A threshold in (0, 1) for each pixel of a 4 x 4 tile, every sixteenth
// once: a fade that has reached f drops about f of the pixels, spread
// evenly, so an opaque mesh fades without blending.
fn screen_door(pixel: vec2<f32>) -> f32 {
    var tile = array<u32, 16>(0u, 8u, 2u, 10u, 12u, 4u, 14u, 6u, 3u, 11u, 1u, 9u, 15u, 7u, 13u, 5u);
    let at = vec2<u32>(pixel) % 4u;
    return (f32(tile[at.y * 4u + at.x]) + 0.5) / 16.0;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Slice clip: drop fragments above the selected layer's top Z. The
    // clip height rides in light_dir.w; f32::MAX (no clip) discards none.
    if (in.world_pos.z > u.light_dir.w) {
        discard;
    }
    // From under the plate (#481), a support mesh's raft fades with the
    // plate, so the tips above it show.
    if (in.supports == 1u && in.world_pos.z < u.env_max.w
        && screen_door(in.clip_pos.xy) < u.env_min.w) {
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
    let mapped = in.tint == MODEL_OVERHANG_MAP;
    let selection_tint = select(1.0, OVERHANG_SELECTED_TINT, mapped);
    let base_color = mix(unselected, highlighted, f32(in.selected) * selection_tint);
    var washed = mix(base_color, WARNING_WASH, f32(in.breaches) * WARNING_WASH_STRENGTH);
    if (mapped) {
        // `flat_n` runs right-to-down across the screen, into it: the face
        // normal, seen from its front, is the opposite way.
        washed = overhang_paint(washed, -flat_n, in.world_pos);
    }
    // Componentwise envelope test: a fragment outside on any axis
    // is painted in the full warning amber instead of the wash.
    let p = in.world_pos;
    let lo = u.env_min.xyz;
    let hi = u.env_max.xyz;
    let outside = any(p < lo) || any(p > hi);
    let final_color = select(washed, WARNING_OUTSIDE, outside);
    return vec4<f32>(final_color * intensity, 1.0);
}

// The cut a hidden raft leaves in a support mesh (#481). From under the
// plate the raft fades out and the shafts open where it stood; their
// insides, the back faces this pass draws, take the look of the flat face
// the cut would leave, so a shaft reads solid. Only for a support mesh, and
// only above what fades: the rest is left to the main pass.
@fragment
fn fs_cap(in: VsOut) -> @location(0) vec4<f32> {
    if (in.supports != 1u || in.world_pos.z > u.light_dir.w) {
        discard;
    }
    if (in.world_pos.z < u.env_max.w && screen_door(in.clip_pos.xy) < u.env_min.w) {
        discard;
    }
    // The cut faces down, toward the camera under the plate.
    let n = vec3<f32>(0.0, 0.0, -1.0);
    let lambert = abs(dot(n, normalize(u.light_dir.xyz)));
    let intensity = 0.25 + 0.75 * lambert;
    let unselected = vec3<f32>(0.75, 0.78, 0.85);
    let highlighted = vec3<f32>(1.00, 0.78, 0.42);
    let base_color = mix(unselected, highlighted, f32(in.selected));
    return vec4<f32>(base_color * intensity, 1.0);
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
