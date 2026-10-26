/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Preview overlays: meshes drawn over the scene that are not part of it, such
//! as a support before it is placed (green, or red where it cannot go), a
//! hollow's cavity before it is applied, or a drain before it is drilled.
//! Pieces are in world coordinates and drawn translucent without depth, so
//! what lies inside or behind the model shows.

use std::num::NonZeroU64;
use std::sync::Arc;

use slint::wgpu_30::wgpu;

use super::Renderer;
use super::device_ops::upload_static_buffer;
use super::pipelines::mesh::MeshBindGroup;
use super::scene_upload::{GpuVertex, MODEL_ENTRY_SIZE, ModelEntry};

/// How an overlay piece is tinted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OverlayTint {
    /// A placement that will work.
    Valid = 1,
    /// A placement that will not.
    Invalid = 2,
    /// A hollow's cavity.
    Cavity = 3,
    /// A drain.
    Drain = 4,
}

/// The overlay as a whole: its pieces, each shared so a piece that did not
/// change (a cavity while the cursor moves) is not uploaded again.
pub(crate) type Overlay = Arc<[Arc<OverlayMesh>]>;

/// One overlay piece: a triangle list in world millimetres.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OverlayMesh {
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub tint: OverlayTint,
}

struct UploadedPiece {
    source: Arc<OverlayMesh>,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    entry_bind_group: wgpu::BindGroup,
    _entry: wgpu::Buffer,
}

/// The installed overlay and the pieces it was built from.
#[derive(Default)]
pub(super) struct OverlaySlot {
    source: Option<Overlay>,
    pieces: Vec<UploadedPiece>,
}

impl OverlaySlot {
    pub(super) fn record(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        pipeline: &wgpu::RenderPipeline,
        uniforms: &wgpu::BindGroup,
    ) {
        if self.pieces.is_empty() {
            return;
        }
        pass.set_pipeline(pipeline);
        pass.set_bind_group(MeshBindGroup::Uniforms.idx(), uniforms, &[]);
        for piece in &self.pieces {
            pass.set_bind_group(MeshBindGroup::Models.idx(), &piece.entry_bind_group, &[0]);
            pass.set_vertex_buffer(0, piece.vertices.slice(..));
            pass.set_index_buffer(piece.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..piece.index_count, 0, 0..1);
        }
    }
}

impl Renderer {
    /// Installs the overlay pieces (`None` clears them). Nothing happens when
    /// `pieces` is the `Arc` installed; otherwise a piece already uploaded
    /// (the same `Arc`) is kept and only new ones are uploaded. A piece the
    /// device refuses is left out.
    pub(crate) fn set_overlay(&mut self, pieces: Option<&Overlay>) {
        let same = match (&self.overlay.source, pieces) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        self.overlay.source = pieces.cloned();
        crate::breadcrumbs::note(&format!(
            "gpu: overlay {} piece(s)",
            pieces.map_or(0, |p| p.len())
        ));
        let mut kept = std::mem::take(&mut self.overlay.pieces);
        let Some(pieces) = pieces else {
            return;
        };
        let device = &self.core.device;
        let queue = &self.core.queue;
        for piece in pieces.iter() {
            if let Some(i) = kept.iter().position(|u| Arc::ptr_eq(&u.source, piece)) {
                self.overlay.pieces.push(kept.swap_remove(i));
                continue;
            }
            if piece.indices.is_empty() || piece.positions.is_empty() {
                continue;
            }
            let vertices: Vec<GpuVertex> =
                piece.positions.iter().map(|&p| GpuVertex::at(p)).collect();
            let entry = ModelEntry::overlay(piece.tint as u32);
            let uploaded = (|| {
                let vertices = upload_static_buffer(
                    device,
                    queue,
                    "overlay vertices",
                    bytemuck::cast_slice(&vertices),
                    wgpu::BufferUsages::VERTEX,
                )?;
                let indices = upload_static_buffer(
                    device,
                    queue,
                    "overlay indices",
                    bytemuck::cast_slice(&piece.indices),
                    wgpu::BufferUsages::INDEX,
                )?;
                let entry_buffer = upload_static_buffer(
                    device,
                    queue,
                    "overlay model entry",
                    bytemuck::bytes_of(&entry),
                    wgpu::BufferUsages::UNIFORM,
                )?;
                Ok::<_, wgpu::Error>((vertices, indices, entry_buffer))
            })();
            let Ok((vertices, indices, entry_buffer)) = uploaded else {
                continue;
            };
            let entry_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("overlay model entry bg"),
                layout: &self.pipelines.mesh.models_bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &entry_buffer,
                        offset: 0,
                        size: NonZeroU64::new(MODEL_ENTRY_SIZE),
                    }),
                }],
            });
            self.overlay.pieces.push(UploadedPiece {
                source: piece.clone(),
                vertices,
                indices,
                index_count: piece.indices.len() as u32,
                entry_bind_group,
                _entry: entry_buffer,
            });
        }
    }
}
