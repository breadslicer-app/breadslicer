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
pub enum OverlayTint {
    /// A placement that will work.
    Valid = 1,
    /// A placement that will not.
    Invalid = 2,
    /// A hollow's cavity.
    Cavity = 3,
    /// A drain.
    Drain = 4,
    /// A placement not known yet to work or not (a drain before the cavity
    /// it would open into is computed): neutral, neither green nor red.
    Pending = 5,
    /// The ball at a support's tip, drawn from under the plate in place of
    /// the supports (#493). Not an overlay of its own: the support lines
    /// draw these.
    SupportBall = 7,
    /// Where a support is recommended (#524): drawn on the model's surface,
    /// hidden behind it like the scene, not over it.
    Hint = 8,
    /// An island an analysis found (#525), painted on its model (#542): its
    /// paint seen through everything, faint.
    Island = 9,
    /// An island's paint on the model's faces, where they show (#542).
    IslandFace = 10,
    /// The dot of an island too small for its paint to read, over
    /// everything (#542).
    IslandDot = 11,
    /// The paint of the island the camera went to (#538, #542): brighter.
    IslandVisited = 12,
}

/// The overlay as a whole: its pieces, each shared so a piece that did not
/// change (a cavity while the cursor moves) is not uploaded again.
pub type Overlay = Arc<[Arc<OverlayMesh>]>;

/// One overlay piece: a triangle list in world millimetres.
#[derive(Debug, Clone, PartialEq)]
pub struct OverlayMesh {
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

impl OverlaySlot {
    /// Installs `pieces` (`None` clears them). Nothing happens when `pieces`
    /// is the `Arc` installed; otherwise a piece already uploaded (the same
    /// `Arc`) is kept and only new ones are uploaded. A piece the device
    /// refuses is left out. `what` names the slot in the breadcrumbs.
    pub(super) fn install(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        models_layout: &wgpu::BindGroupLayout,
        pieces: Option<&Overlay>,
        what: &str,
    ) {
        let same = match (&self.source, pieces) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        self.source = pieces.cloned();
        crate::breadcrumbs::note(&format!(
            "gpu: {what} {} piece(s)",
            pieces.map_or(0, |p| p.len())
        ));
        let mut kept = std::mem::take(&mut self.pieces);
        let Some(pieces) = pieces else {
            return;
        };
        for piece in pieces.iter() {
            if let Some(i) = kept.iter().position(|u| Arc::ptr_eq(&u.source, piece)) {
                self.pieces.push(kept.swap_remove(i));
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
                layout: models_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &entry_buffer,
                        offset: 0,
                        size: NonZeroU64::new(MODEL_ENTRY_SIZE),
                    }),
                }],
            });
            self.pieces.push(UploadedPiece {
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

impl Renderer {
    /// Installs the overlay pieces (`None` clears them): see
    /// [`OverlaySlot::install`].
    pub(crate) fn set_overlay(&mut self, pieces: Option<&Overlay>) {
        self.overlay.install(
            &self.core.device,
            &self.core.queue,
            &self.pipelines.mesh.models_bind_group_layout,
            pieces,
            "overlay",
        );
    }

    /// Shows `pieces` (`None` clears them): the marks of where supports are
    /// recommended (#524) on the models' surfaces, tested against the
    /// scene's depth, and the rings of the islands (#525) over everything
    /// (#538).
    pub(crate) fn set_hint_marks(&mut self, pieces: Option<&Overlay>) {
        let same = match (&self.hint_source, pieces) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        self.hint_source = pieces.cloned();
        let all: Vec<Arc<OverlayMesh>> =
            pieces.into_iter().flat_map(|p| p.iter().cloned()).collect();
        let of = |keep: &dyn Fn(OverlayTint) -> bool| -> Overlay {
            all.iter()
                .filter(|p| keep(p.tint))
                .cloned()
                .collect::<Vec<_>>()
                .into()
        };
        let islands = of(&|t| t == OverlayTint::IslandDot);
        let hidden = of(&|t| t == OverlayTint::Island);
        let hints = of(&|t| !matches!(t, OverlayTint::IslandDot | OverlayTint::Island));
        self.hints.install(
            &self.core.device,
            &self.core.queue,
            &self.pipelines.mesh.models_bind_group_layout,
            pieces.map(|_| &hints),
            "hint marks",
        );
        self.island_marks.install(
            &self.core.device,
            &self.core.queue,
            &self.pipelines.mesh.models_bind_group_layout,
            pieces.map(|_| &islands),
            "island marks",
        );
        self.island_hidden.install(
            &self.core.device,
            &self.core.queue,
            &self.pipelines.mesh.models_bind_group_layout,
            pieces.map(|_| &hidden),
            "island paint hidden",
        );
    }
}
