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
    // From under the plate, the supports drawn as lines (#493): x = the
    // height above which a support mesh fades out with the plate, mm (a
    // huge one keeps it), y = half the width of a dash, pixels, zw = the
    // target's size, pixels.
    under_plate: vec4<f32>,
    // The eye (#523): w = 1, its position; w = 0 (orthographic), the
    // direction toward it.
    eye: vec4<f32>,
    // The plates (#545): xy = how far apart they stand, z = how many,
    // w = the active one.
    plates: vec4<f32>,
};

struct ModelEntry {
    model: mat4x4<f32>,
    // Per-object flag word, written every frame.
    //   flags.x = bit 0 selected; bit 1 out of focus: something else is
    //             selected and this is not it nor its supports or model,
    //             so it is drawn dimmed (#523)
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

// A model under the overhang map whose mesh is fine enough to carry the
// surface around each vertex (#498): xyz that surface's normal in the leaf
// frame, w 1 where the map reads it and 0 where it keeps the facet's.
struct VsMappedIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) surface: vec4<f32>,
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
    // The world height the cuts read (the raft's, #481, and the slice
    // view's layer), at the centroid of the samples the triangle covers. A
    // wall seen near edge on covers a few samples of a pixel and not its
    // centre, and read there its height runs on far past the wall: a cut
    // kept such a pixel, a speck on the raft's rim or of the part above the
    // layer. `world_pos` stays at the centre, where its screen derivatives
    // are taken.
    @location(6) @interpolate(perspective, centroid) cut_z: f32,
    // The surface the overhang map reads (#498), in the world: xyz its unit
    // normal and w 1 where it is read, all 0 to read the facet.
    @location(7) surface: vec4<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    return placed(in.position, in.normal);
}

@vertex
fn vs_mapped(in: VsMappedIn) -> VsOut {
    var out = placed(in.position, in.normal);
    let n = normal_to_world(in.surface.xyz);
    let len = length(n);
    out.surface = select(
        vec4<f32>(0.0),
        vec4<f32>(n / len, in.surface.w),
        len > 0.0 && in.surface.w > 0.0,
    );
    return out;
}

// A normal of the leaf frame in the world, by the cofactors of the model
// matrix: they carry the normal of an area through any linear map, a scale
// that is not the same on every axis and a mirror included.
fn normal_to_world(n: vec3<f32>) -> vec3<f32> {
    let a = model_entry.model[0].xyz;
    let b = model_entry.model[1].xyz;
    let c = model_entry.model[2].xyz;
    let cofactors = mat3x3<f32>(cross(b, c), cross(c, a), cross(a, b));
    return cofactors * n * sign(dot(a, cross(b, c)));
}

fn placed(position: vec3<f32>, normal: vec3<f32>) -> VsOut {
    var out: VsOut;
    let world = model_entry.model * vec4<f32>(position, 1.0);
    out.clip_pos = u.view_proj * world;
    out.world_pos = world.xyz;
    out.cut_z = world.z;
    out.selected = model_entry.flags.x;
    out.breaches = model_entry.flags.y;
    out.tint = model_entry.flags.z;
    out.supports = model_entry.flags.w;
    // Direction only: the model matrix's linear part. Non-uniform scale
    // bends it slightly; the shading tolerates that.
    out.normal = (model_entry.model * vec4<f32>(normal, 0.0)).xyz;
    out.surface = vec4<f32>(0.0);
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
// one the support engine classifies, or on a fine mesh the surface around the
// fragment (#498): the smooth shading normal rounds a facet off and would
// disagree with both, and the screen derivatives the face normal comes from
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

// Whether a support mesh's fragment fades out from under the plate: its
// raft, below env_max.w, so the tips above it show (#481), and the rest
// above under_plate.x, where the supports are drawn as lines (#493). While
// the plate fades (env_min.w between 0 and 1) such a fragment is drawn
// translucent by `fs_support_fade`, not by the opaque passes: a pattern of
// dropped pixels flickered, and a selected set's outline ran round every
// one of them.
fn fades_under_plate(supports: u32, cut_z: f32) -> bool {
    return u.env_min.w > 0.0 && supports == 1u
        && (cut_z < u.env_max.w || cut_z > u.under_plate.x);
}

// The colours of a model and of a support mesh (#523): a warm light grey,
// and a darker slate, so supports stand apart from what they hold.
const MODEL_COLOR: vec3<f32> = vec3<f32>(0.82, 0.82, 0.80);
const SUPPORT_COLOR: vec3<f32> = vec3<f32>(0.38, 0.44, 0.53);
// What is out of focus while something else is selected is drawn at this
// share of its light (#523).
const OUT_OF_FOCUS: f32 = 0.5;
// A second, cooler light from the other side, how much of it, and the soft
// highlight and rim that make folds read (#523).
const FILL_DIR: vec3<f32> = vec3<f32>(-0.62, -0.47, 0.36);
const FILL: f32 = 0.35;

// The direction from `world_pos` toward the eye.
fn toward_eye(world_pos: vec3<f32>) -> vec3<f32> {
    return select(normalize(u.eye.xyz), normalize(u.eye.xyz - world_pos), u.eye.w > 0.5);
}

// `color` lit as the scene is (#523): a sky and ground ambient, the key
// light on either side of a face, a cooler fill, and for `shiny` a soft
// highlight and a rim along the silhouette.
fn studio(color: vec3<f32>, normal: vec3<f32>, world_pos: vec3<f32>, shiny: f32) -> vec3<f32> {
    let v = toward_eye(world_pos);
    // Faced to the eye: lighting is the same whichever way a face winds.
    let n = select(normal, -normal, dot(normal, v) < 0.0);
    let l = normalize(u.light_dir.xyz);
    let hemi = mix(0.20, 0.34, n.z * 0.5 + 0.5);
    let key = abs(dot(n, l));
    let fill = max(dot(n, normalize(FILL_DIR)), 0.0) * FILL;
    let h = normalize(l + v);
    let spec = pow(max(dot(n, h), 0.0), 40.0) * 0.20 * shiny;
    let rim = pow(1.0 - max(dot(n, v), 0.0), 3.0) * 0.10 * shiny;
    // Never past full light, so no colour burns out (the overhang map's red
    // least of all).
    return color * min(hemi + 0.72 * key + fill, 1.0) + vec3<f32>(spec + rim);
}

// A fragment of a model or a support as the main pass paints it: lit, with
// the overhang map and the envelope's warnings, dimmed when out of focus.
// What is selected keeps its own colours: the outline marks it (#523).
fn surface_color(
    world_pos: vec3<f32>,
    normal: vec3<f32>,
    selected: u32,
    breaches: u32,
    tint: u32,
    surface: vec4<f32>,
    supports: u32,
) -> vec3<f32> {
    // The interpolated smooth normal; a vertex without one shades flat from
    // screen-space derivatives.
    let flat_n = normalize(cross(dpdx(world_pos), dpdy(world_pos)));
    let has_normal = dot(normal, normal) > 1e-12;
    let n = select(flat_n, normalize(normal), has_normal);
    let mapped = tint == MODEL_OVERHANG_MAP;
    let base_color = select(MODEL_COLOR, SUPPORT_COLOR, supports == 1u);
    var washed = mix(base_color, WARNING_WASH, f32(breaches) * WARNING_WASH_STRENGTH);
    if (mapped) {
        // `flat_n` runs right-to-down across the screen, into it: the face
        // normal, seen from its front, is the opposite way. Where the three
        // corners of the facet read the surface around them, the map reads
        // that instead (#498): a fine facet tilting down on a surface that
        // faces up is no overhang, nor one tilting up inside an overhang
        // anything else.
        let reads_surface = surface.w > 0.999 && dot(surface.xyz, surface.xyz) > 1e-6;
        let map_n = select(-flat_n, normalize(surface.xyz), reads_surface);
        washed = overhang_paint(washed, map_n, world_pos);
    }
    // Componentwise envelope test: a fragment outside on any axis of the
    // plate under it (#545) is painted in the full warning amber instead of
    // the wash.
    let outside = outside_plates(world_pos);
    let final_color = select(washed, WARNING_OUTSIDE, outside);
    // The overhang map is an analysis view, tuned under the single light
    // it was made with: its colours are read as they were (#91, #523).
    let flat_light = final_color * (0.25 + 0.75 * abs(dot(n, normalize(u.light_dir.xyz))));
    let shiny = select(1.0, 0.4, supports == 1u);
    // Past the build volume the warning's amber is lit alike, map or not.
    let lit = select(studio(final_color, n, world_pos, shiny), flat_light, mapped && !outside);
    return select(lit, lit * OUT_OF_FOCUS, (selected & 2u) != 0u);
}

// Whether `p` is outside the build volume of the plate whose cell holds it
// (#545): the nearest plate's, or outside every plate past the last.
fn outside_plates(p: vec3<f32>) -> bool {
    let col = clamp(round(p.x / u.plates.x), 0.0, 3.0);
    let row = max(round(p.y / u.plates.y), 0.0);
    let index = row * 4.0 + col;
    if index >= u.plates.z {
        return true;
    }
    let local = p - vec3<f32>(col * u.plates.x, row * u.plates.y, 0.0);
    return any(local < u.env_min.xyz) || any(local > u.env_max.xyz);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Slice clip: drop fragments above the selected layer's top Z. The
    // clip height rides in light_dir.w; f32::MAX (no clip) discards none.
    if (in.cut_z > u.light_dir.w) {
        discard;
    }
    if (fades_under_plate(in.supports, in.cut_z)) {
        discard;
    }
    let color = surface_color(
        in.world_pos,
        in.normal,
        in.selected,
        in.breaches,
        in.tint,
        in.surface,
        in.supports,
    );
    return vec4<f32>(color, 1.0);
}

// What of a support mesh fades from under the plate, while the plate fades
// (#481, #493), first only into the depth (and, for a selected set, the
// stencil, which its fading outline reads): so the colour pass after it
// paints the nearest surface alone, as the opaque mesh shows, instead of
// every surface in whatever order the triangles come.
@fragment
fn fs_support_fade_depth(in: VsOut) -> @location(0) vec4<f32> {
    if (in.cut_z > u.light_dir.w || !fades_under_plate(in.supports, in.cut_z)) {
        discard;
    }
    return vec4<f32>(0.0);
}

// What is selected, marked into the stencil through whatever stands in
// front (#523): the outline then runs round its silhouette alone. Only what
// is drawn: not above the slice clip, nor what fades from under the plate.
@fragment
fn fs_mark(in: VsOut) -> @location(0) vec4<f32> {
    if (in.cut_z > u.light_dir.w || fades_under_plate(in.supports, in.cut_z)) {
        discard;
    }
    return vec4<f32>(0.0);
}

// Then its colour: as the main pass would paint it, as opaque as the plate
// still is.
@fragment
fn fs_support_fade(in: VsOut) -> @location(0) vec4<f32> {
    if (in.cut_z > u.light_dir.w || !fades_under_plate(in.supports, in.cut_z)) {
        discard;
    }
    let color = surface_color(
        in.world_pos,
        in.normal,
        in.selected,
        in.breaches,
        in.tint,
        in.surface,
        in.supports,
    );
    return vec4<f32>(color, 1.0 - u.env_min.w);
}

// The cut a hidden raft leaves in a support mesh (#481). From under the
// plate the raft fades out and the shafts open where it stood; their
// insides, the back faces this pass draws, take the look of the flat face
// the cut would leave, so a shaft reads solid. Only for a support mesh, and
// only above what fades: the rest is left to the main pass.
@fragment
fn fs_cap(in: VsOut) -> @location(0) vec4<f32> {
    if (in.supports != 1u || in.cut_z > u.light_dir.w) {
        discard;
    }
    if (fades_under_plate(in.supports, in.cut_z)) {
        discard;
    }
    // The cut faces down, toward the camera under the plate.
    let n = vec3<f32>(0.0, 0.0, -1.0);
    let lit = studio(SUPPORT_COLOR, n, in.world_pos, 0.4);
    return vec4<f32>(select(lit, lit * OUT_OF_FOCUS, (in.selected & 2u) != 0u), 1.0);
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
    } else if (in.tint == 8u) {
        // Where a support is recommended (#524): a clear green, unlit, so it
        // reads the same on any face.
        return vec4<f32>(0.42, 0.92, 0.62, 0.95);
    } else if (in.tint == 9u) {
        // An island an analysis found (#525, #542), seen through everything:
        // a faint magenta, unlit, apart from the overhang map's red.
        return vec4<f32>(0.92, 0.26, 0.86, 0.35);
    } else if (in.tint == 10u) {
        // An island painted on the model's faces where they show (#542).
        color = vec4<f32>(0.92, 0.26, 0.86, 0.92);
    } else if (in.tint == 11u) {
        // The dot of an island too small for its paint to read (#542).
        return vec4<f32>(0.96, 0.36, 0.92, 0.95);
    } else if (in.tint == 12u) {
        // The island the camera went to (#538, #542): a lighter magenta.
        color = vec4<f32>(1.00, 0.62, 0.98, 0.95);
    }
    return vec4<f32>(color.rgb * intensity, color.a);
}

// The supports as lines from under the plate (#493), white (#523), as
// opaque as the supports have faded.
const SUPPORT_LINE_COLOR: vec3<f32> = vec3<f32>(0.92, 0.94, 0.96);
// The balls at their tips stay in the amber (#523): the lines go white.
const SUPPORT_BALL_COLOR: vec3<f32> = vec3<f32>(1.00, 0.66, 0.24);

struct LineIn {
    @builtin(vertex_index) corner: u32,
    // A dash, once per instance: its two ends, world mm.
    @location(0) a: vec3<f32>,
    @location(1) b: vec3<f32>,
};

struct LineOut {
    @builtin(position) clip_pos: vec4<f32>,
};

// A dash as a quad: two triangles over its ends, each corner pushed off the
// dash on screen by half its width, so it reads the same at any zoom. A dash
// with an end behind the eye is left out.
@vertex
fn vs_support_line(in: LineIn) -> LineOut {
    var out: LineOut;
    var ends = array<u32, 6>(0u, 0u, 1u, 0u, 1u, 1u);
    var sides = array<f32, 6>(-1.0, 1.0, -1.0, 1.0, 1.0, -1.0);
    let a = u.view_proj * vec4<f32>(in.a, 1.0);
    let b = u.view_proj * vec4<f32>(in.b, 1.0);
    if (a.w <= 1e-5 || b.w <= 1e-5) {
        out.clip_pos = vec4<f32>(0.0, 0.0, 2.0, 1.0);
        return out;
    }
    let size = max(u.under_plate.zw, vec2<f32>(1.0, 1.0));
    var along = (b.xy / b.w - a.xy / a.w) * size;
    if (dot(along, along) < 1e-8) {
        along = vec2<f32>(1.0, 0.0);
    }
    let across = normalize(vec2<f32>(-along.y, along.x));
    let end = select(a, b, ends[in.corner] == 1u);
    // From pixels to clip space: a target is 2 wide in NDC.
    let push = across * sides[in.corner] * u.under_plate.y * 2.0 / size;
    out.clip_pos = vec4<f32>(end.xy + push * end.w, end.z, end.w);
    return out;
}

@fragment
fn fs_support_line(in: LineOut) -> @location(0) vec4<f32> {
    return vec4<f32>(SUPPORT_LINE_COLOR, u.env_min.w);
}

// The ball at a support's tip (#493), lit like the models so it reads round.
@fragment
fn fs_support_ball(in: VsOut) -> @location(0) vec4<f32> {
    let n = normalize(cross(dpdx(in.world_pos), dpdy(in.world_pos)));
    let lambert = abs(dot(n, normalize(u.light_dir.xyz)));
    let intensity = 0.45 + 0.55 * lambert;
    return vec4<f32>(SUPPORT_BALL_COLOR * intensity, u.env_min.w);
}
