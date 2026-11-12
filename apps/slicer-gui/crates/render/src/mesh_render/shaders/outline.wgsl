/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

// Outline post-pass.
//
// Reads sample 0 of the multisampled stencil produced by the main pass
// (the selected object's fragments wrote `1`; everything else is `0`)
// and emits an outline colour wherever the 3×3 neighbourhood mixes
// `0` and `1`, that is, on the silhouette edge of the selected
// object. Non-edge fragments `discard`, so the resolved colour from
// the main pass shows through unchanged.
//
// Sample 0 only: MSAA edges may have mixed sample stencil values, but
// for our 0-vs-1 use case sample 0 is a faithful representative
// almost everywhere; the rare per-sample stencil disagreement amounts
// to one pixel of edge dither, which is invisible against a 2-pixel
// outline.

@group(0) @binding(0) var stencil: texture_multisampled_2d<u32>;

// A selected set fading from under the plate (#493) marks the stencil with
// 2 and up, by how opaque it still is (`mod.rs::fading_stencil`): the
// outline round it fades with it.
const FADING_STENCIL_BASE: u32 = 2u;
const FADING_STENCIL_LEVELS: f32 = 200.0;

// How opaque the outline is next to a pixel marked `mark`: 0 unselected.
fn mark_opacity(mark: u32) -> f32 {
    if (mark == 0u) {
        return 0.0;
    }
    if (mark < FADING_STENCIL_BASE) {
        return 1.0;
    }
    return min(f32(mark - FADING_STENCIL_BASE) / FADING_STENCIL_LEVELS, 1.0);
}

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VsOut {
    // Single oversized triangle covering the screen, saves a vertex
    // versus a quad, no vertex buffer needed.
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    var out: VsOut;
    out.clip_pos = vec4<f32>(positions[idx], 0.0, 1.0);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // FragCoord is in pixel space (origin top-left, y-down) for both
    // the colour attachment and the multisampled stencil because they
    // share dimensions.
    let dim = textureDimensions(stencil);
    let last_x = i32(dim.x) - 1;
    let last_y = i32(dim.y) - 1;
    let cx = i32(in.clip_pos.x);
    let cy = i32(in.clip_pos.y);

    let centre = textureLoad(stencil, vec2<i32>(cx, cy), 0).x;
    let centre_selected = centre != 0u;

    var differs: u32 = 0u;
    var opacity = mark_opacity(centre);

    // Clamped 3×3 sample. Out-of-range loads on `texture_2d` return
    // zero in wgpu, which would falsely paint outlines along the
    // screen edge whenever the selected object is `1` at the border;
    // clamping keeps the comparison against an in-bounds neighbour.
    for (var dy: i32 = -1; dy <= 1; dy = dy + 1) {
        for (var dx: i32 = -1; dx <= 1; dx = dx + 1) {
            if (dx == 0 && dy == 0) { continue; }
            let nx = clamp(cx + dx, 0, last_x);
            let ny = clamp(cy + dy, 0, last_y);
            let n = textureLoad(stencil, vec2<i32>(nx, ny), 0).x;
            // Selected or not: a set's fading part and its whole part are
            // one set, with no outline between them.
            if ((n != 0u) != centre_selected) {
                differs = differs + 1u;
            }
            opacity = max(opacity, mark_opacity(n));
        }
    }

    if (differs == 0u || opacity <= 0.0) {
        discard;
    }

    // Match the warm tint hue from `mesh_render.wgsl` so the highlight
    // and outline read as one stylistic choice.
    return vec4<f32>(1.0, 0.68, 0.28, opacity);
}
