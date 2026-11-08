/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

// Plate pipeline: renders the 3D plate slab (top face inside the
// build envelope is the grid floor, the rounded outer ring is plain
// border colour, sides are a darker shade, bottom is translucent
// so the cage and any meshes show through when the camera looks up
// from below) plus the build-volume cage edges, plus 5 invisible
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
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) kind: u32,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) @interpolate(flat) kind: u32,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip_pos = u.view_proj * vec4<f32>(in.position, 1.0);
    out.world_pos = in.position;
    out.kind = in.kind;
    return out;
}

// Colours are Breadslicer's theme tokens (`ui/kit/theme.slint`, dark),
// written as-is: the viewport target is not sRGB-encoded.
// Floor: `raised` #272b2e, grid lines: `border` #383e42.
const FLOOR_BASE: vec3<f32> = vec3<f32>(0.153, 0.169, 0.180);
const FLOOR_GRID: vec3<f32> = vec3<f32>(0.220, 0.243, 0.259);
// Cage edges: `border-strong` #545c62.
const CAGE_COLOUR: vec3<f32> = vec3<f32>(0.329, 0.361, 0.384);
// Plate top border (outer rounded ring): `border-soft` #2c3135, a
// shade lighter than the floor, so the build area reads as set into
// a rim without a hard separator.
const PLATE_TOP_BORDER_COLOUR: vec3<f32> = vec3<f32>(0.173, 0.192, 0.208);
// Plate sides: `well` #16181a, darker so the slab fakes 3D depth
// without a lighting model.
const PLATE_SIDE_COLOUR: vec3<f32> = vec3<f32>(0.086, 0.094, 0.102);
// Bottom face: same hue as the sides, but emitted with alpha < 1
// so the alpha-blending pipeline mixes it over whatever was in the
// framebuffer when the camera looks up from below.
const PLATE_BOTTOM_ALPHA: f32 = 0.45;

// Major grid spacing: 10 mm. Single set of lines, moderate intensity.
const GRID_SPACING_MM: f32 = 10.0;

// Half-width of a grid line in millimetres. Together with the
// fwidth-based anti-aliasing below, this gives a roughly 1 mm-wide
// line at world scale, which appears as a thin stripe at default
// zoom and stays readable as the camera dollies in or out.
const GRID_LINE_HALF_WIDTH_MM: f32 = 0.5;

fn floor_colour(world_xy: vec2<f32>) -> vec3<f32> {
    // Distance to the nearest grid line on each axis, in millimetres.
    // `fract(x / S) * S` is in `[0, S)`; `min(d, S - d)` folds it to
    // `[0, S/2]`, the distance to whichever neighbour line is closer.
    let coord = world_xy / GRID_SPACING_MM;
    let folded = abs(fract(coord) - 0.5) - 0.5;
    let dist_mm = min(abs(folded.x), abs(folded.y)) * GRID_SPACING_MM;
    // Screen-space derivative gives a per-fragment mm-per-pixel
    // estimate; widening the line by it keeps the apparent thickness
    // stable as the camera moves.
    let aa = max(fwidth(world_xy.x), fwidth(world_xy.y));
    let line_intensity = 1.0 - smoothstep(GRID_LINE_HALF_WIDTH_MM, GRID_LINE_HALF_WIDTH_MM + aa, dist_mm);
    return mix(FLOOR_BASE, FLOOR_GRID, line_intensity);
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
        let grid = floor_colour(in.world_pos.xy);
        // Bit 5 of face_breach_mask flags a breach below Z = 0. When
        // it is set, paint the same diagonal stripe pattern used by
        // the wall pass directly over the grid; the indicator is
        // visible across the entire build floor regardless of where
        // the offending vertex lies in XY.
        if (u.face_breach_mask.x & (1u << 5u)) != 0u {
            let s = in.world_pos.x + in.world_pos.y;
            let phase = fract(s / WALL_STRIPE_PERIOD_MM);
            if phase < WALL_STRIPE_DUTY {
                return plate_face(WALL_WARNING_COLOUR, 1.0);
            }
        }
        return plate_face(grid, 1.0);
    }
    if in.kind == 2u {
        return plate_face(PLATE_TOP_BORDER_COLOUR, 1.0);
    }
    if in.kind == 3u {
        return plate_face(PLATE_SIDE_COLOUR, 1.0);
    }
    if in.kind == 4u {
        return plate_face(PLATE_SIDE_COLOUR, PLATE_BOTTOM_ALPHA);
    }
    if in.kind >= 5u && in.kind <= 9u {
        // Walls 5..9 map to bits 0..4 of face_breach_mask.x. Discard
        // when the bit is clear so a non-breached wall costs zero
        // pixels regardless of the blend state.
        let face_idx = in.kind - 5u;
        let breached = (u.face_breach_mask.x >> face_idx) & 1u;
        if breached == 0u {
            discard;
        }
        let s = wall_stripe_coord(in.kind, in.world_pos);
        let phase = fract(s / WALL_STRIPE_PERIOD_MM);
        if phase >= WALL_STRIPE_DUTY {
            discard;
        }
        return vec4<f32>(WALL_WARNING_COLOUR, WALL_STRIPE_ALPHA);
    }
    return vec4<f32>(CAGE_COLOUR, 1.0);
}
