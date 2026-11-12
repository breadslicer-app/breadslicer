/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

// Plate pipeline: renders the plate (top face inside the build envelope
// is the grid floor, the rounded outer ring carries the rulers and is the
// background's shade; the sides and the bottom are not drawn, the plate
// being a drawing on the ground) plus the build-volume cage edges, plus 5 invisible
// envelope-wall quads that turn into diagonal red stripes when the
// matching face-breach bit is set. Vertex positions are world-space
// mm; `kind` selects which colour path the fragment shader takes:
//   0 = floor (inner top, grid)
//   1 = cage edge
//   2 = plate top border (outer rounded ring at Z = 0)
//   3 = plate side (vertical panels)
//   4 = plate bottom (alpha-blended translucent face)
//   5 = envelope wall, -X face
//   6 = envelope wall, +X face
//   7 = envelope wall, -Y face
//   8 = envelope wall, +Y face
//   9 = envelope wall, +Z (top) face

struct Uniforms {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,
    // env_min / env_max are shared with the mesh pipeline; the plate
    // shader reads only env_min.w, how far the plate has faded from
    // under it (#481). face_breach_mask.x is a 6-bit
    // bitfield with layout
    //   0 = -X wall, 1 = +X wall, 2 = -Y wall, 3 = +Y wall,
    //   4 = +Z top, 5 = -Z floor (Z = 0)
    // Bits 0..=4 gate the corresponding wall quad (wall fragments
    // discard when their bit is clear). Bit 5 is read by the floor
    // path and overlays the same diagonal stripe pattern on top of
    // the build-plate grid wherever an object has any vertex below
    // Z = 0.
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

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) kind: u32,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    // On the plate drawn, as the first stands: the grid, the rulers and the
    // cage are measured from its corners whichever plate it is.
    @location(0) world_pos: vec3<f32>,
    @location(1) @interpolate(flat) kind: u32,
    @location(2) @interpolate(flat) plate: u32,
};

// One instance per plate (#545), moved to where it stands.
@vertex
fn vs_main(in: VsIn, @builtin(instance_index) plate: u32) -> VsOut {
    var out: VsOut;
    out.clip_pos = u.view_proj * vec4<f32>(in.position + plate_offset(plate), 1.0);
    out.world_pos = in.position;
    out.kind = in.kind;
    out.plate = plate;
    return out;
}

// Whether `plate` is one of several and not the active one (#545): it shows
// its floor alone, fainter, as an object out of focus does.
fn inactive_plate(plate: u32) -> bool {
    return u.plates.z > 1.5 && f32(plate) != u.plates.w;
}

// How opaque the floor of an inactive plate is.
const INACTIVE_PLATE_ALPHA: f32 = 0.45;

// Whether a model crosses plate `plate`'s walls: bit `plate` of
// face_breach_mask.y (#545).
fn breached_plate(plate: u32) -> bool {
    return ((u.face_breach_mask.y >> plate) & 1u) != 0u;
}

// Colours are Breadslicer's theme tokens (`ui/kit/theme.slint`, dark),
// written as-is: the viewport target is not sRGB-encoded. The plate is
// drawn as a technical plan (#507): a floor a shade off the background, a
// fine grid, millimetre rulers along the front and left edges, and the
// build volume as dashed uprights with brackets at its top corners.
// Floor: a shade over `ground` #111315.
const FLOOR_BASE: vec3<f32> = vec3<f32>(0.078, 0.090, 0.102);
// The 10 mm grid, #1f2428, and every 50 mm, #31393f.
const FLOOR_GRID: vec3<f32> = vec3<f32>(0.122, 0.141, 0.157);
const FLOOR_GRID_MAJOR: vec3<f32> = vec3<f32>(0.192, 0.224, 0.247);
// The floor's outline, `border-strong` #3c454d.
const FLOOR_EDGE: vec3<f32> = vec3<f32>(0.235, 0.271, 0.302);
// The edge of the active plate among several (#545): the selection's amber.
const ACTIVE_EDGE: vec3<f32> = vec3<f32>(1.0, 0.68, 0.28);
// Cage lines and brackets: `text-muted` #8b959d.
const CAGE_COLOUR: vec3<f32> = vec3<f32>(0.545, 0.584, 0.616);
// The rim around the floor: the background, so the plate reads as a
// drawing on the ground rather than a slab. One shade off, so what stands
// behind it still tells the two apart.
const PLATE_TOP_BORDER_COLOUR: vec3<f32> = vec3<f32>(0.071, 0.079, 0.086);
// The ruler's ticks, `text-soft` #c2c9cf dimmed.
const RULER_TICK: vec3<f32> = vec3<f32>(0.545, 0.584, 0.616);

// Grid spacing: 10 mm, with every fifth line stronger. Measured from the
// plate's front-left corner, as the rulers are.
const GRID_SPACING_MM: f32 = 10.0;
const GRID_MAJOR_EVERY: f32 = 5.0;

// Ruler ticks, outward from the floor's edge: every 10 mm, longer every
// 50 mm.
const TICK_MM: f32 = 1.4;
const TICK_MAJOR_MM: f32 = 3.0;

// How far the brackets at the top of the volume reach along each edge, mm.
const BRACKET_MM: f32 = 14.0;
// The dashes of the volume's uprights: their period and how much of it is
// drawn, mm.
const DASH_PERIOD_MM: f32 = 6.0;
const DASH_ON_MM: f32 = 2.0;

// Coverage of a line `dist` mm from its middle, a pixel wide whatever the
// zoom: `aa` is the fragment's millimetres per pixel.
fn hairline(dist: f32, aa: f32, pixels: f32) -> f32 {
    return clamp(0.5 * pixels + 0.5 - dist / aa, 0.0, 1.0);
}

// Distance to the nearest multiple of `step` from `x`.
fn to_step(x: f32, step: f32) -> f32 {
    let f = fract(x / step);
    return min(f, 1.0 - f) * step;
}

fn floor_colour(world_xy: vec2<f32>, plate: u32) -> vec3<f32> {
    let lo = u.env_min.xy;
    let hi = u.env_max.xy;
    let local = world_xy - lo;
    let aa = max(fwidth(world_xy.x), fwidth(world_xy.y));
    let major_step = GRID_SPACING_MM * GRID_MAJOR_EVERY;
    let minor = max(
        hairline(to_step(local.x, GRID_SPACING_MM), aa, 1.0),
        hairline(to_step(local.y, GRID_SPACING_MM), aa, 1.0),
    );
    let major = max(
        hairline(to_step(local.x, major_step), aa, 1.2),
        hairline(to_step(local.y, major_step), aa, 1.2),
    );
    // Far from the camera the minor lines would turn the floor into a
    // haze: they fade out once they come closer than 4 px apart.
    let minor_shown = minor * (1.0 - smoothstep(GRID_SPACING_MM / 8.0, GRID_SPACING_MM / 4.0, aa));
    var colour = mix(FLOOR_BASE, FLOOR_GRID, minor_shown);
    colour = mix(colour, FLOOR_GRID_MAJOR, major);
    let edge = min(min(world_xy.x - lo.x, hi.x - world_xy.x), min(world_xy.y - lo.y, hi.y - world_xy.y));
    if u.plates.z > 1.5 && f32(plate) == u.plates.w {
        return mix(colour, ACTIVE_EDGE, hairline(edge, aa, 3.0));
    }
    return mix(colour, FLOOR_EDGE, hairline(edge, aa, 1.5));
}

// The rim around the floor, with the rulers' ticks on its front and left
// sides: counted from the front-left corner, every 10 mm.
fn rim_colour(world_xy: vec2<f32>) -> vec3<f32> {
    let lo = u.env_min.xy;
    let hi = u.env_max.xy;
    let aa = max(fwidth(world_xy.x), fwidth(world_xy.y));
    let local = world_xy - lo;
    var tick = 0.0;
    // Front: below the floor, along X.
    let out_front = lo.y - world_xy.y;
    if local.x >= -aa && world_xy.x <= hi.x + aa && out_front >= 0.0 {
        let major = to_step(local.x, GRID_SPACING_MM * GRID_MAJOR_EVERY) < GRID_SPACING_MM * 0.5;
        let reach = select(TICK_MM, TICK_MAJOR_MM, major);
        if out_front <= reach {
            tick = max(tick, hairline(to_step(local.x, GRID_SPACING_MM), aa, 1.0));
        }
    }
    // Left: beside the floor, along Y.
    let out_left = lo.x - world_xy.x;
    if local.y >= -aa && world_xy.y <= hi.y + aa && out_left >= 0.0 {
        let major = to_step(local.y, GRID_SPACING_MM * GRID_MAJOR_EVERY) < GRID_SPACING_MM * 0.5;
        let reach = select(TICK_MM, TICK_MAJOR_MM, major);
        if out_left <= reach {
            tick = max(tick, hairline(to_step(local.y, GRID_SPACING_MM), aa, 1.0));
        }
    }
    return mix(PLATE_TOP_BORDER_COLOUR, RULER_TICK, tick);
}

// How much of a cage fragment shows: the edges round the floor whole, the
// uprights dashed, and along the top only brackets at the corners and a
// faint line between them.
fn cage_alpha(p: vec3<f32>) -> f32 {
    let lo = u.env_min.xyz;
    let hi = u.env_max.xyz;
    let height = hi.z - lo.z;
    let along_x = min(abs(p.x - lo.x), abs(p.x - hi.x));
    let along_y = min(abs(p.y - lo.y), abs(p.y - hi.y));
    let up = p.z - lo.z;
    if up < 0.01 * height {
        return 1.0;
    }
    if up > 0.99 * height {
        // On a top edge one of the two distances is 0; the other is how
        // far along the edge from its nearest corner.
        if max(along_x, along_y) <= BRACKET_MM {
            return 1.0;
        }
        return 0.18;
    }
    if height - up <= BRACKET_MM {
        return 1.0;
    }
    if fract(up / DASH_PERIOD_MM) * DASH_PERIOD_MM < DASH_ON_MM {
        return 0.55;
    }
    return 0.0;
}

// Warning-wall palette and pattern. Diagonal stripes in world-space
// millimetres: STRIPE_PERIOD_MM mm from one bright band's leading
// edge to the next; STRIPE_DUTY of the period is bright. The stripe
// alpha is the user-visible "intensity" of the warning. Picked low
// enough that the offending mesh's red-tinted pixels remain readable
// through the wall, high enough that the wall reads as an unambiguous
// warning at default viewing distance.
// The theme's highlight, `accent` #d7a75a.
const WALL_WARNING_COLOUR: vec3<f32> = vec3<f32>(0.843, 0.655, 0.353);
const WALL_STRIPE_PERIOD_MM: f32 = 12.0;
const WALL_STRIPE_DUTY: f32 = 0.36;
const WALL_STRIPE_ALPHA: f32 = 0.55;

// Returns the 2D parameter `u + v` (in mm) used by the diagonal stripe
// pattern for a given envelope-wall kind. Each wall has two in-plane
// axes; the third is fixed by the envelope corner the wall sits on.
fn wall_stripe_coord(kind: u32, world_pos: vec3<f32>) -> f32 {
    // -X and +X walls: in-plane axes are Y and Z.
    if kind == 5u || kind == 6u {
        return world_pos.y + world_pos.z;
    }
    // -Y and +Y walls: in-plane axes are X and Z.
    if kind == 7u || kind == 8u {
        return world_pos.x + world_pos.z;
    }
    // +Z top: in-plane axes are X and Y.
    return world_pos.x + world_pos.y;
}

// The plate's own faces (floor, border, sides, bottom) with `colour`, as
// far as the view from under the plate has faded them (#481). The cage and
// the warning walls stay: they keep the view's bearings.
fn plate_face(colour: vec3<f32>, alpha: f32) -> vec4<f32> {
    let shown = alpha * (1.0 - u.env_min.w);
    if shown <= 0.004 {
        discard;
    }
    return vec4<f32>(colour, shown);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    if in.kind == 0u {
        let grid = floor_colour(in.world_pos.xy, in.plate);
        // Bit 5 of face_breach_mask flags a breach below Z = 0. When
        // it is set, paint the same diagonal stripe pattern used by
        // the wall pass directly over the grid; the indicator is
        // visible across the entire build floor regardless of where
        // the offending vertex lies in XY.
        if (u.face_breach_mask.x & (1u << 5u)) != 0u && breached_plate(in.plate) {
            let s = in.world_pos.x + in.world_pos.y;
            let phase = fract(s / WALL_STRIPE_PERIOD_MM);
            if phase < WALL_STRIPE_DUTY {
                return plate_face(WALL_WARNING_COLOUR, 1.0);
            }
        }
        return plate_face(grid, select(1.0, INACTIVE_PLATE_ALPHA, inactive_plate(in.plate)));
    }
    if in.kind == 2u {
        return plate_face(
            rim_colour(in.world_pos.xy),
            select(1.0, INACTIVE_PLATE_ALPHA, inactive_plate(in.plate)),
        );
    }
    // An inactive plate has no cage nor warning walls (#545).
    if inactive_plate(in.plate) {
        discard;
    }
    if in.kind == 3u || in.kind == 4u {
        // The plate is a drawing on the ground, not a slab (#507): its
        // sides and underside are not drawn, so a camera going under the
        // plate sees no rim of a thickness the drawing does not have.
        discard;
    }
    if in.kind >= 5u && in.kind <= 9u {
        // Walls 5..9 map to bits 0..4 of face_breach_mask.x. Discard
        // when the bit is clear so a non-breached wall costs zero
        // pixels regardless of the blend state.
        let face_idx = in.kind - 5u;
        let breached = (u.face_breach_mask.x >> face_idx) & 1u;
        if breached == 0u || !breached_plate(in.plate) {
            discard;
        }
        let s = wall_stripe_coord(in.kind, in.world_pos);
        let phase = fract(s / WALL_STRIPE_PERIOD_MM);
        if phase >= WALL_STRIPE_DUTY {
            discard;
        }
        return vec4<f32>(WALL_WARNING_COLOUR, WALL_STRIPE_ALPHA);
    }
    let shown = cage_alpha(in.world_pos);
    if shown <= 0.0 {
        discard;
    }
    return vec4<f32>(CAGE_COLOUR, shown);
}
