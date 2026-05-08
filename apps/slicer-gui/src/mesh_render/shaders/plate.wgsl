/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

// Plate pipeline: renders the 3D plate slab (top face inside the
// build envelope is the grid floor, the rounded outer ring is plain
// border colour, sides + bottom are a darker shade so the slab
// reads as 3D) plus the build-volume cage edges. Vertex positions
// are world-space mm; `kind` selects which colour path the fragment
// shader takes:
//   0 = floor (inner top, grid)
//   1 = cage edge
//   2 = plate top border (outer rounded ring at Z = 0)
//   3 = plate side (vertical panels and bottom face)

struct Uniforms {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,
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

const FLOOR_BASE: vec3<f32> = vec3<f32>(0.16, 0.20, 0.22);
const FLOOR_GRID: vec3<f32> = vec3<f32>(0.22, 0.28, 0.30);
const CAGE_COLOUR: vec3<f32> = vec3<f32>(0.55, 0.40, 0.25);
// Plate top border (outer rounded ring): slightly darker than the
// grid floor base so the build-area edge reads visually without a
// hard separator.
const PLATE_TOP_BORDER_COLOUR: vec3<f32> = vec3<f32>(0.13, 0.16, 0.18);
// Plate sides and bottom: darker still so the slab fakes 3D depth
// without a lighting model.
const PLATE_SIDE_COLOUR: vec3<f32> = vec3<f32>(0.09, 0.11, 0.13);

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

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    if in.kind == 0u {
        return vec4<f32>(floor_colour(in.world_pos.xy), 1.0);
    }
    if in.kind == 2u {
        return vec4<f32>(PLATE_TOP_BORDER_COLOUR, 1.0);
    }
    if in.kind == 3u {
        return vec4<f32>(PLATE_SIDE_COLOUR, 1.0);
    }
    return vec4<f32>(CAGE_COLOUR, 1.0);
}
