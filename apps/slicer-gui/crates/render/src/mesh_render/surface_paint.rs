/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Paint on the models' faces and the brush's ring, in the 3D view (#183).
//!
//! Each painted object gets a second index buffer holding only its painted
//! triangles, over the vertex buffer it already has: the mesh is not uploaded
//! again, and the vertex format stays as it is. During a stroke only that
//! buffer is written, once per frame at most, whatever the number of dabs.
//!
//! Paint shows in the workspace, never in a picture of the print.

use std::sync::Arc;

use bytemuck::cast_slice;
use sc_mesh_core::IndexedMesh;
use sc_scene::{NodeId, SurfaceTags, UNPAINTED};
use slint::wgpu_30::wgpu;

use super::device_ops::create_buffer_reporting;
use super::pipelines::mesh::MeshBindGroup;
use super::pipelines::surface_paint::{PaintParams, SurfacePaintPipeline};
use super::smooth_shading::{SmoothMesh, shades_smooth, smooth_mesh};
use super::{InstalledScene, Renderer};

/// Bind group of the paint parameters: the third, after the mesh pipeline's.
const PARAMS_GROUP: u32 = 2;

/// One object's paint to show.
#[derive(Clone, Debug, PartialEq)]
pub struct PaintLayer {
    pub leaf: NodeId,
    pub tags: SurfaceTags,
}

/// The ring of a brush over a model, in world millimetres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrushRing {
    pub leaf: NodeId,
    pub center: [f32; 3],
    pub radius_mm: f32,
    /// The colour of its line, straight RGBA.
    pub color: [f32; 4],
}

/// What a layer's buffer was built from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LayerKey {
    revision: u64,
    mesh: usize,
    smooth: bool,
}

struct UploadedLayer {
    leaf: NodeId,
    key: LayerKey,
    indices: wgpu::Buffer,
    index_count: u32,
    /// What a smooth-shaded object draws from, kept so a stroke does not
    /// depend on the smooth cache still holding it.
    smooth: Option<Arc<SmoothMesh>>,
}

/// The paint and the ring installed for the next frames.
#[derive(Default)]
pub(super) struct PaintSlot {
    layers: Vec<UploadedLayer>,
    ring: Option<BrushRing>,
    tint: [f32; 4],
}

/// The vertex indices of the painted faces of `tags`, in the order the
/// object's own index buffer has them: `smooth`'s for a smooth-shaded object.
/// `None` when it draws from indices that are not one triple per face.
fn painted_indices(
    tags: &SurfaceTags,
    mesh: &IndexedMesh,
    smooth: Option<&SmoothMesh>,
) -> Option<Vec<u32>> {
    let mut out = Vec::with_capacity(tags.painted_faces() * 3);
    let painted = tags
        .values()
        .iter()
        .enumerate()
        .filter(|(_, v)| **v != UNPAINTED)
        .map(|(face, _)| face);
    if let Some(drawn) = smooth {
        if drawn.indices.len() != mesh.faces().len() * 3 {
            return None;
        }
        for face in painted {
            out.extend_from_slice(&drawn.indices[face * 3..face * 3 + 3]);
        }
    } else {
        for face in painted {
            let f = mesh.faces().get(face)?;
            out.extend(f.vertices.map(u32::from));
        }
    }
    Some(out)
}

fn key_of(tags: &SurfaceTags, mesh: &Arc<IndexedMesh>) -> LayerKey {
    LayerKey {
        revision: tags.revision(),
        mesh: Arc::as_ptr(mesh) as usize,
        smooth: shades_smooth(mesh),
    }
}

impl PaintSlot {
    /// Draws the paint and the ring over the objects of `inst`.
    pub(super) fn record(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        pipeline: &SurfacePaintPipeline,
        inst: &InstalledScene,
    ) {
        if self.layers.is_empty() && self.ring.is_none() {
            return;
        }
        pass.set_bind_group(
            MeshBindGroup::Uniforms.idx(),
            &pipeline.uniforms_bind_group,
            &[],
        );
        pass.set_bind_group(PARAMS_GROUP, &pipeline.params_bind_group, &[]);
        let objects = inst.gpu.objects();
        let slot_of = |leaf: NodeId| objects.iter().position(|o| o.node_id() == leaf);
        let offset = |i: usize| (i as u64 * inst.gpu.model_stride()) as u32;
        if !self.layers.is_empty() {
            pass.set_pipeline(&pipeline.tint);
            for layer in &self.layers {
                let Some(i) = slot_of(layer.leaf) else {
                    continue;
                };
                pass.set_bind_group(
                    MeshBindGroup::Models.idx(),
                    &inst.models_bind_group,
                    &[offset(i)],
                );
                pass.set_vertex_buffer(0, objects[i].vertex_buffer().slice(..));
                pass.set_index_buffer(layer.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..layer.index_count, 0, 0..1);
            }
        }
        if let Some(ring) = &self.ring
            && let Some(i) = slot_of(ring.leaf)
        {
            pass.set_pipeline(&pipeline.ring);
            pass.set_bind_group(
                MeshBindGroup::Models.idx(),
                &inst.models_bind_group,
                &[offset(i)],
            );
            pass.set_vertex_buffer(0, objects[i].vertex_buffer().slice(..));
            pass.set_index_buffer(
                objects[i].index_buffer().slice(..),
                wgpu::IndexFormat::Uint32,
            );
            pass.draw_indexed(0..objects[i].index_count(), 0, 0..1);
        }
    }

    /// How many objects show paint.
    #[cfg(test)]
    pub(super) fn layer_count(&self) -> usize {
        self.layers.len()
    }

    /// The indices the object `leaf` draws its paint with.
    #[cfg(test)]
    pub(super) fn index_count(&self, leaf: NodeId) -> Option<u32> {
        self.layers
            .iter()
            .find(|l| l.leaf == leaf)
            .map(|l| l.index_count)
    }
}

impl Renderer {
    /// Shows `layers` in the workspace, tinted `tint`, in place of what was
    /// shown. A layer that did not change keeps its buffer; one the device
    /// refuses is left out, as an overlay piece is.
    pub fn set_surface_paint(&mut self, layers: &[PaintLayer], tint: [f32; 4]) {
        let mut old = std::mem::take(&mut self.surface_paint.layers);
        for layer in layers {
            let mesh = layer.tags.mesh();
            let key = key_of(&layer.tags, mesh);
            let kept = old.iter().position(|u| u.leaf == layer.leaf);
            let reused = kept.map(|i| old.swap_remove(i));
            if let Some(uploaded) = &reused
                && uploaded.key == key
            {
                self.surface_paint.layers.extend(reused);
                continue;
            }
            let smooth = key.smooth.then(|| {
                reused
                    .as_ref()
                    .filter(|uploaded| uploaded.key.mesh == key.mesh)
                    .and_then(|uploaded| uploaded.smooth.clone())
                    .unwrap_or_else(|| smooth_mesh(mesh))
            });
            let Some(indices) = painted_indices(&layer.tags, mesh, smooth.as_deref()) else {
                continue;
            };
            if indices.is_empty() {
                continue;
            }
            let bytes: &[u8] = cast_slice(&indices);
            // The buffer of the frame before, when the paint still fits.
            let buffer = match reused {
                Some(uploaded) if uploaded.indices.size() >= bytes.len() as u64 => uploaded.indices,
                _ => {
                    let size = (bytes.len() as u64 * 3 / 2).next_multiple_of(4);
                    let created = create_buffer_reporting(
                        &self.core.device,
                        &wgpu::BufferDescriptor {
                            label: Some("mesh-viewer paint indices"),
                            size,
                            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                            mapped_at_creation: false,
                        },
                    );
                    let Ok(buffer) = created else {
                        continue;
                    };
                    buffer
                }
            };
            self.core.queue.write_buffer(&buffer, 0, bytes);
            self.surface_paint.layers.push(UploadedLayer {
                leaf: layer.leaf,
                key,
                indices: buffer,
                index_count: indices.len() as u32,
                smooth,
            });
        }
        self.surface_paint.tint = tint;
        self.write_paint_params();
    }

    /// Shows `ring` on its model (`None` hides it).
    pub fn set_brush_ring(&mut self, ring: Option<BrushRing>) {
        if self.surface_paint.ring == ring {
            return;
        }
        self.surface_paint.ring = ring;
        self.write_paint_params();
    }

    fn write_paint_params(&self) {
        let ring = self.surface_paint.ring;
        let params = PaintParams {
            tint: self.surface_paint.tint,
            ring: ring.map_or([0.0; 4], |r| {
                [r.center[0], r.center[1], r.center[2], r.radius_mm]
            }),
            ring_color: ring.map_or([0.0; 4], |r| r.color),
        };
        self.core.queue.write_buffer(
            &self.pipelines.surface_paint.params,
            0,
            bytemuck::bytes_of(&params),
        );
    }
}
