/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! What the camera sees, as depth (#3): every mesh of the scene, model and
//! supports, drawn into a depth buffer on the CPU, so Remove's box and brush
//! can ask of thousands of points whether something stands in front of
//! them. A ray each through the scene costs about a millisecond on a model
//! of two million triangles; drawing the whole scene once costs a few tens,
//! and every point after it a lookup.
//!
//! The buffer holds, per pixel, the depth along the camera's view of the
//! nearest surface there. A perspective camera interpolates the inverse of
//! the depth across a triangle on screen, which is exact; an orthographic
//! one the depth itself.

use std::sync::Arc;

use glam::Vec3;
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::Transform3D;
use sc_mesh_spatial::SceneSpatial;

use super::box_select::Projector;

/// The widest the buffer is drawn, in its own pixels: a wider viewport
/// is drawn at a fraction of its resolution.
const MAX_WIDTH: f32 = 960.0;

/// The scene's depth as the camera sees it.
pub(super) struct SightDepth {
    width: usize,
    height: usize,
    /// Logical pixels of the viewport per pixel of the buffer.
    scale: f32,
    /// Per pixel, the depth of the nearest surface (mm along the view);
    /// infinite where nothing is.
    depth: Vec<f32>,
}

/// A point of a mesh as the buffer draws it: its pixel and what is
/// interpolated across a triangle (the depth, or its inverse).
type Drawn = Option<([f32; 2], f32)>;

impl SightDepth {
    /// Draws every mesh of `spatial` as `projector` sees it, on a viewport
    /// `width` by `height` logical pixels.
    /// Every mesh of `spatial`, shared, with where it stands: what
    /// [`Self::draw`] draws, handed to another thread.
    pub(super) fn meshes_of(spatial: &SceneSpatial) -> Vec<(Arc<IndexedMesh>, Transform3D)> {
        let mut meshes = Vec::new();
        let mut index = 0;
        while let Some(id) = spatial.leaf_id_at_index(index) {
            index += 1;
            if let (Some(mesh), Some(query)) = (spatial.graph().leaf_mesh_arc(id), spatial.leaf(id))
            {
                meshes.push((mesh.clone(), query.world_transform()));
            }
        }
        meshes
    }

    /// Draws `meshes` as `projector` sees them, on a viewport `width` by
    /// `height` logical pixels.
    pub(super) fn draw(
        meshes: &[(Arc<IndexedMesh>, Transform3D)],
        projector: &Projector,
        orthographic: bool,
        width: f32,
        height: f32,
    ) -> Self {
        let scale = (width / MAX_WIDTH).max(1.0);
        let (w, h) = (
            (width / scale).ceil().max(1.0) as usize,
            (height / scale).ceil().max(1.0) as usize,
        );
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        let draw_vertex = |p: Vec3| -> Drawn {
            let at = projector.project(p)?;
            let d = projector.depth(p);
            let value = if orthographic { d } else { 1.0 / d.max(1e-6) };
            Some(([at[0] / scale, at[1] / scale], value))
        };
        // Each core draws its share of every mesh's triangles into a buffer
        // of its own; the nearest of them all is kept.
        let buffers: Vec<Vec<f32>> = std::thread::scope(|scope| {
            let parts: Vec<_> = (0..cores)
                .map(|part| {
                    let draw_vertex = &draw_vertex;
                    scope.spawn(move || {
                        let mut depth = vec![f32::INFINITY; w * h];
                        for (mesh, world) in meshes {
                            let faces = mesh.faces();
                            let share = faces.len().div_ceil(cores);
                            let lo = (part * share).min(faces.len());
                            let hi = ((part + 1) * share).min(faces.len());
                            let vertices = mesh.vertices();
                            for face in &faces[lo..hi] {
                                let [a, b, c] = face.vertices.map(|v| {
                                    let at = world.apply_to_vertex(vertices[usize::from(v)]);
                                    draw_vertex(Vec3::from(*at.as_array()))
                                });
                                if let (Some(a), Some(b), Some(c)) = (a, b, c) {
                                    fill(&mut depth, w, h, orthographic, a, b, c);
                                }
                            }
                        }
                        depth
                    })
                })
                .collect();
            parts
                .into_iter()
                .filter_map(|part| part.join().ok())
                .collect()
        });
        let mut depth = vec![f32::INFINITY; w * h];
        for buffer in buffers {
            for (d, b) in depth.iter_mut().zip(buffer) {
                *d = d.min(b);
            }
        }
        SightDepth {
            width: w,
            height: h,
            scale,
            depth,
        }
    }

    /// Whether `p` (world mm), landing on logical pixel `at`, is in sight:
    /// nothing nearer than `slack_mm` in front of it there.
    pub(super) fn in_sight(
        &self,
        projector: &Projector,
        p: Vec3,
        at: [f32; 2],
        slack_mm: f32,
    ) -> bool {
        let (x, y) = ((at[0] / self.scale) as isize, (at[1] / self.scale) as isize);
        if x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
            return true;
        }
        self.depth[y as usize * self.width + x as usize] >= projector.depth(p) - slack_mm
    }
}

/// Twice the signed area of the triangle `a`, `b`, `p` on screen.
fn edge(a: [f32; 2], b: [f32; 2], p: [f32; 2]) -> f32 {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}

/// Draws a triangle into `depth`: each pixel centre it covers keeps the
/// nearest depth.
fn fill(
    depth: &mut [f32],
    w: usize,
    h: usize,
    orthographic: bool,
    (a, va): ([f32; 2], f32),
    (b, vb): ([f32; 2], f32),
    (c, vc): ([f32; 2], f32),
) {
    let area = edge(a, b, c);
    if area.abs() < 1e-12 {
        return;
    }
    let x0 = a[0].min(b[0]).min(c[0]).floor().max(0.0) as usize;
    let y0 = a[1].min(b[1]).min(c[1]).floor().max(0.0) as usize;
    let x1 = (a[0].max(b[0]).max(c[0]).ceil() as isize).min(w as isize - 1);
    let y1 = (a[1].max(b[1]).max(c[1]).ceil() as isize).min(h as isize - 1);
    if x1 < 0 || y1 < 0 {
        return;
    }
    for y in y0..=y1 as usize {
        for x in x0..=x1 as usize {
            let p = [x as f32 + 0.5, y as f32 + 0.5];
            let (wa, wb) = (edge(b, c, p) / area, edge(c, a, p) / area);
            let wc = 1.0 - wa - wb;
            if wa < -1e-5 || wb < -1e-5 || wc < -1e-5 {
                continue;
            }
            let v = wa * va + wb * vb + wc * vc;
            let d = if orthographic { v } else { 1.0 / v.max(1e-12) };
            let cell = &mut depth[y * w + x];
            if d < *cell {
                *cell = d;
            }
        }
    }
}
