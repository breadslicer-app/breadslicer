/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The supports as lines from under the plate (#493): with the supports
//! faded out, a dotted line along every support's path and the ball at the
//! end of every tip, where it touches the model. Only the picture changes.
//!
//! The dashes are cut once, in world millimetres, so they stay put while
//! the camera moves; the shader widens each to a few pixels on screen. The
//! balls are their true size. Both are tested against the depth of the
//! scene, so what lies behind the model does not show.

use std::sync::Arc;

use slint::wgpu_30::wgpu;

use super::device_ops::upload_static_buffer;
use super::overlay::{Overlay, OverlayMesh, OverlaySlot, OverlayTint};
use super::pipelines::mesh::MeshBindGroup;

/// The length of a dash and of the gap after it, world mm.
pub(super) const DASH_MM: f32 = 0.5;
pub(super) const GAP_MM: f32 = 0.35;

/// Half the width a dash is drawn at, in pixels of the target.
pub(super) const LINE_HALF_WIDTH_PX: f32 = 1.0;

/// How many sides and rings a ball is drawn with.
const BALL_SIDES: u32 = 10;
const BALL_RINGS: u32 = 6;

/// The supports of a scene as lines: the dashes along their paths and the
/// balls at their tips, world mm.
#[derive(Debug, Clone, PartialEq)]
pub struct SupportLines {
    /// Each dash, from one end to the other.
    pub dashes: Vec<[[f32; 3]; 2]>,
    /// The balls, as one triangle list.
    pub balls: Arc<OverlayMesh>,
}

impl SupportLines {
    /// The lines of supports along `paths` (each a polyline, world mm) with
    /// a ball at each of `balls` (centre and radius, mm).
    pub fn new(paths: &[Vec<[f32; 3]>], balls: &[([f32; 3], f32)]) -> Self {
        let mut dashes = Vec::new();
        for path in paths {
            dashes_along(path, &mut dashes);
        }
        let mut mesh = OverlayMesh {
            positions: Vec::new(),
            indices: Vec::new(),
            tint: OverlayTint::SupportBall,
        };
        for &(centre, radius) in balls {
            ball(centre, radius, &mut mesh);
        }
        SupportLines {
            dashes,
            balls: Arc::new(mesh),
        }
    }

    /// Whether there is nothing to draw.
    pub fn is_empty(&self) -> bool {
        self.dashes.is_empty() && self.balls.indices.is_empty()
    }
}

/// Cuts `path` into dashes of [`DASH_MM`] with gaps of [`GAP_MM`], the
/// pattern running on across its corners, and adds them to `out`. A path
/// starts with a dash, at its first point (a support's tip).
fn dashes_along(path: &[[f32; 3]], out: &mut Vec<[[f32; 3]; 2]>) {
    let period = DASH_MM + GAP_MM;
    // How far into the pattern the next point lies.
    let mut phase = 0.0f32;
    for pair in path.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        if len <= f32::EPSILON || !len.is_finite() {
            continue;
        }
        let at = |t: f32| {
            let k = t / len;
            [a[0] + d[0] * k, a[1] + d[1] * k, a[2] + d[2] * k]
        };
        // Along this segment, from its start: to the next end of a dash or
        // of a gap, or to the segment's end. A boundary is always reached,
        // even a step too small to move `t`, so the walk ends.
        let mut t = 0.0f32;
        while t < len {
            let in_dash = phase < DASH_MM;
            let boundary = if in_dash { DASH_MM } else { period };
            let step = (boundary - phase).max(0.0);
            let reached = t + step <= len;
            let end = if reached { t + step } else { len };
            if in_dash && end > t {
                out.push([at(t), at(end)]);
            }
            phase = match (reached, in_dash) {
                (true, true) => DASH_MM,
                (true, false) => 0.0,
                (false, _) => phase + (end - t),
            };
            t = end;
        }
    }
}

/// Adds a ball of `radius` at `centre` to `mesh`: rings of
/// [`BALL_SIDES`] around Z, closed by a vertex at each pole.
fn ball(centre: [f32; 3], radius: f32, mesh: &mut OverlayMesh) {
    if radius <= 0.0 || !radius.is_finite() {
        return;
    }
    let base = mesh.positions.len() as u32;
    let [cx, cy, cz] = centre;
    mesh.positions.push([cx, cy, cz + radius]);
    for ring in 1..BALL_RINGS {
        let polar = std::f32::consts::PI * ring as f32 / BALL_RINGS as f32;
        let (sin_p, cos_p) = polar.sin_cos();
        for side in 0..BALL_SIDES {
            let around = std::f32::consts::TAU * side as f32 / BALL_SIDES as f32;
            let (sin_a, cos_a) = around.sin_cos();
            mesh.positions.push([
                cx + radius * sin_p * cos_a,
                cy + radius * sin_p * sin_a,
                cz + radius * cos_p,
            ]);
        }
    }
    mesh.positions.push([cx, cy, cz - radius]);
    let ring_at = |ring: u32, side: u32| base + 1 + (ring - 1) * BALL_SIDES + side % BALL_SIDES;
    let bottom = base + 1 + (BALL_RINGS - 1) * BALL_SIDES;
    for side in 0..BALL_SIDES {
        mesh.indices
            .extend([base, ring_at(1, side), ring_at(1, side + 1)]);
        for ring in 1..BALL_RINGS - 1 {
            let (a, b) = (ring_at(ring, side), ring_at(ring, side + 1));
            let (c, d) = (ring_at(ring + 1, side), ring_at(ring + 1, side + 1));
            mesh.indices.extend([a, c, b, b, c, d]);
        }
        mesh.indices.extend([
            ring_at(BALL_RINGS - 1, side),
            bottom,
            ring_at(BALL_RINGS - 1, side + 1),
        ]);
    }
}

/// The installed lines on the device.
#[derive(Default)]
pub(super) struct LinesSlot {
    source: Option<Arc<SupportLines>>,
    dashes: Option<(wgpu::Buffer, u32)>,
    balls: OverlaySlot,
}

impl LinesSlot {
    /// Installs `lines` (`None` clears them); the same `Arc` is not uploaded
    /// again.
    pub(super) fn install(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        models_layout: &wgpu::BindGroupLayout,
        lines: Option<&Arc<SupportLines>>,
    ) {
        let same = match (&self.source, lines) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        self.source = lines.cloned();
        self.dashes = lines.filter(|l| !l.dashes.is_empty()).and_then(|l| {
            upload_static_buffer(
                device,
                queue,
                "support line dashes",
                bytemuck::cast_slice(&l.dashes),
                wgpu::BufferUsages::VERTEX,
            )
            .ok()
            .map(|buffer| (buffer, l.dashes.len() as u32))
        });
        let balls: Option<Overlay> = lines.map(|l| Arc::from(vec![l.balls.clone()]));
        self.balls.install(
            device,
            queue,
            models_layout,
            balls.as_ref(),
            "support balls",
        );
    }

    /// Whether anything is installed.
    pub(super) fn is_empty(&self) -> bool {
        self.source.as_ref().is_none_or(|l| l.is_empty())
    }

    /// Draws the dashes and the balls.
    pub(super) fn record(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        line_pipeline: &wgpu::RenderPipeline,
        ball_pipeline: &wgpu::RenderPipeline,
        uniforms: &wgpu::BindGroup,
    ) {
        if let Some((buffer, count)) = &self.dashes {
            pass.set_pipeline(line_pipeline);
            pass.set_bind_group(MeshBindGroup::Uniforms.idx(), uniforms, &[]);
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.draw(0..6, 0..*count);
        }
        self.balls.record(pass, ball_pipeline, uniforms);
    }
}

/// The byte size of one dash in the vertex buffer: two points.
pub(super) const DASH_STRIDE: u64 = 6 * 4;

const _: () = assert!(size_of::<[[f32; 3]; 2]>() as u64 == DASH_STRIDE);

#[cfg(test)]
mod tests {
    use super::*;

    fn length(d: &[[f32; 3]; 2]) -> f32 {
        let [a, b] = d;
        ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2) + (b[2] - a[2]).powi(2)).sqrt()
    }

    #[test]
    fn a_straight_path_is_cut_into_dashes_and_gaps() {
        let mut out = Vec::new();
        dashes_along(&[[0.0, 0.0, 10.0], [0.0, 0.0, 0.0]], &mut out);
        // 10 mm of 0.85 mm periods: 12 dashes, the last one whole.
        assert_eq!(out.len(), 12);
        assert_eq!(out[0][0], [0.0, 0.0, 10.0], "a dash starts at the tip");
        for (i, dash) in out.iter().enumerate() {
            let len = length(dash);
            assert!(len <= DASH_MM + 1e-4, "{i}: {len}");
            let start = 10.0 - i as f32 * (DASH_MM + GAP_MM);
            assert!((dash[0][2] - start).abs() < 1e-3, "{i}: {dash:?}");
        }
        let drawn: f32 = out.iter().map(length).sum();
        assert!((drawn - 12.0 * DASH_MM).abs() < 1e-3, "{drawn}");
    }

    #[test]
    fn the_pattern_runs_on_across_a_corner() {
        let mut out = Vec::new();
        // A corner 0.2 mm into the first dash: the dash goes on past it.
        dashes_along(
            &[[0.0, 0.0, 1.0], [0.0, 0.0, 0.8], [0.0, 1.0, 0.8]],
            &mut out,
        );
        assert_eq!(out[0], [[0.0, 0.0, 1.0], [0.0, 0.0, 0.8]]);
        assert!((length(&out[1]) - 0.3).abs() < 1e-5, "{:?}", out[1]);
        assert!((out[2][0][1] - 0.65).abs() < 1e-5, "{:?}", out[2]);
        // Degenerate segments add nothing.
        let mut none = Vec::new();
        dashes_along(&[[1.0, 1.0, 1.0], [1.0, 1.0, 1.0]], &mut none);
        dashes_along(&[[1.0, 1.0, 1.0]], &mut none);
        assert!(none.is_empty());
    }

    #[test]
    fn a_ball_is_closed_and_as_wide_as_asked() {
        let lines = SupportLines::new(&[], &[([1.0, 2.0, 3.0], 0.12), ([5.0, 5.0, 5.0], 0.0)]);
        let mesh = &lines.balls;
        assert_eq!(
            mesh.positions.len() as u32,
            2 + (BALL_RINGS - 1) * BALL_SIDES,
            "one ball: a zero radius draws none"
        );
        assert_eq!(mesh.indices.len() % 3, 0);
        assert!(
            mesh.indices
                .iter()
                .all(|&i| (i as usize) < mesh.positions.len())
        );
        // Every edge is shared by two triangles: the ball is closed.
        let mut edges = std::collections::HashMap::<(u32, u32), i32>::new();
        for t in mesh.indices.chunks(3) {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                *edges.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        assert!(edges.values().all(|&n| n == 2), "{edges:?}");
        for p in &mesh.positions {
            let r = ((p[0] - 1.0).powi(2) + (p[1] - 2.0).powi(2) + (p[2] - 3.0).powi(2)).sqrt();
            assert!((r - 0.12).abs() < 1e-5, "{p:?}");
        }
        assert!(!lines.is_empty());
        assert!(SupportLines::new(&[], &[]).is_empty());
    }
}
