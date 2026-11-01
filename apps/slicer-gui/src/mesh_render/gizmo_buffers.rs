/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Static gizmo VBO/IBO geometry plus the per-frame buffers. The handle
//! meshes of each gizmo style (#274) are uploaded once at renderer
//! construction and reused every frame:
//!
//! - the move arrow (shaft and cone)
//! - the full rotate ring
//! - the per-axis scale cube
//! - the uniform-scale ball
//! - the plane square (#207)
//! - the thin shaft under a lone scale cube
//!
//! Per frame, the draw entries are written in one go (`write_entries`) and,
//! while rotating, the marks of the turn (`write_marks`).

use std::num::NonZeroU64;
use std::ops::Range;

use slint::wgpu_30::wgpu;

use crate::gizmo::look::{GizmoStyle, Metrics};
use crate::gizmo_geometry::{self, GizmoVertex, RotateMarks};

use super::device_ops::{create_buffer_reporting, upload_static_buffer};
use super::gizmo_entries::{
    GIZMO_DRAW_ENTRY_SIZE, GIZMO_ENTRY_SLOTS, GizmoDrawEntry, GizmoMesh, GizmoUniformsGpu,
};
use super::scene_upload::align_up;

/// Vertices the per-frame marks buffer holds: a full-turn slice (126
/// triangles), 24 ticks and 2 spokes, with room to spare.
const MARKS_CAPACITY: u64 = 1024;

/// One static indexed mesh.
pub(super) struct MeshBuffers {
    pub(super) vertices: wgpu::Buffer,
    pub(super) indices: wgpu::Buffer,
    pub(super) index_count: u32,
}

impl MeshBuffers {
    fn upload(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        label: &'static str,
        (vertices, indices): (Vec<GizmoVertex>, Vec<u32>),
    ) -> Result<Self, wgpu::Error> {
        let vertex_buffer = upload_static_buffer(
            device,
            queue,
            label,
            bytemuck::cast_slice(&vertices),
            wgpu::BufferUsages::VERTEX,
        )?;
        let index_buffer = upload_static_buffer(
            device,
            queue,
            label,
            bytemuck::cast_slice(&indices),
            wgpu::BufferUsages::INDEX,
        )?;
        Ok(Self {
            vertices: vertex_buffer,
            indices: index_buffer,
            index_count: indices.len() as u32,
        })
    }
}

/// The static handle meshes of one gizmo style.
struct StyleMeshes {
    arrow: MeshBuffers,
    ring: MeshBuffers,
    cube: MeshBuffers,
    ball: MeshBuffers,
    plane: MeshBuffers,
    stalk: MeshBuffers,
}

impl StyleMeshes {
    fn upload(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        style: GizmoStyle,
    ) -> Result<Self, wgpu::Error> {
        let m = Metrics::of(style);
        let up = |label, geometry| MeshBuffers::upload(device, queue, label, geometry);
        Ok(Self {
            arrow: up(
                "mesh-viewer gizmo arrow",
                gizmo_geometry::build_arrow_geometry(m),
            )?,
            ring: up(
                "mesh-viewer gizmo ring",
                gizmo_geometry::build_ring_geometry(m),
            )?,
            cube: up(
                "mesh-viewer gizmo scale cube",
                gizmo_geometry::build_scale_cube_geometry(m),
            )?,
            ball: up(
                "mesh-viewer gizmo centre ball",
                gizmo_geometry::build_centre_ball_geometry(m),
            )?,
            plane: up(
                "mesh-viewer gizmo plane square",
                gizmo_geometry::build_plane_geometry(m),
            )?,
            stalk: up(
                "mesh-viewer gizmo scale stalk",
                gizmo_geometry::build_stalk_geometry(m),
            )?,
        })
    }
}

pub(super) struct GizmoBuffers {
    outline: StyleMeshes,
    focused: StyleMeshes,
    /// The marks of a turn in progress, rewritten each frame of a rotate
    /// drag: the slice first, then the ticks and spokes.
    pub(super) marks_buffer: wgpu::Buffer,
    pub(super) uniforms_buffer: wgpu::Buffer,
    /// Per-frame draw-entry buffer with `GIZMO_ENTRY_SLOTS` slots; each
    /// draw binds a window of one entry via a dynamic offset.
    pub(super) entries_buffer: wgpu::Buffer,
    pub(super) entry_stride: u64,
    /// Reused staging bytes for `write_entries`.
    staging: std::cell::RefCell<Vec<u8>>,
}

impl GizmoBuffers {
    pub(super) fn create(device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Self, wgpu::Error> {
        let outline = StyleMeshes::upload(device, queue, GizmoStyle::Outline)?;
        let focused = StyleMeshes::upload(device, queue, GizmoStyle::Focused)?;

        let marks_buffer = create_buffer_reporting(
            device,
            &wgpu::BufferDescriptor {
                label: Some("mesh-viewer gizmo rotate marks"),
                size: MARKS_CAPACITY * size_of::<GizmoVertex>() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        )?;

        let uniforms_buffer = create_buffer_reporting(
            device,
            &wgpu::BufferDescriptor {
                label: Some("mesh-viewer gizmo uniforms"),
                size: size_of::<GizmoUniformsGpu>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        )?;

        let alignment = device.limits().min_uniform_buffer_offset_alignment as u64;
        let entry_stride = align_up(GIZMO_DRAW_ENTRY_SIZE, alignment);
        let entries_buffer = create_buffer_reporting(
            device,
            &wgpu::BufferDescriptor {
                label: Some("mesh-viewer gizmo entries"),
                size: entry_stride * GIZMO_ENTRY_SLOTS,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        )?;

        Ok(Self {
            outline,
            focused,
            marks_buffer,
            uniforms_buffer,
            entries_buffer,
            entry_stride,
            staging: std::cell::RefCell::new(Vec::new()),
        })
    }

    /// The static mesh behind `mesh` in `style`, `None` for the per-frame
    /// marks.
    pub(super) fn mesh(&self, mesh: GizmoMesh, style: GizmoStyle) -> Option<&MeshBuffers> {
        let set = match style {
            GizmoStyle::Outline => &self.outline,
            GizmoStyle::Focused => &self.focused,
        };
        match mesh {
            GizmoMesh::Arrow => Some(&set.arrow),
            GizmoMesh::Ring => Some(&set.ring),
            GizmoMesh::Cube => Some(&set.cube),
            GizmoMesh::Ball => Some(&set.ball),
            GizmoMesh::Plane => Some(&set.plane),
            GizmoMesh::Stalk => Some(&set.stalk),
            GizmoMesh::Wedge | GizmoMesh::Marks => None,
        }
    }

    /// Resource window matching the gizmo entries BGL's
    /// `min_binding_size`, suitable for the dynamic-offset bind
    /// group. Used when `GizmoPipeline` builds the entries bind
    /// group.
    pub(super) fn entries_binding_window(&self) -> wgpu::BindingResource<'_> {
        wgpu::BindingResource::Buffer(wgpu::BufferBinding {
            buffer: &self.entries_buffer,
            offset: 0,
            size: NonZeroU64::new(GIZMO_DRAW_ENTRY_SIZE),
        })
    }

    /// Writes `entries` into consecutive slots from slot 0, in one upload.
    /// Entries past `GIZMO_ENTRY_SLOTS` are dropped.
    pub(super) fn write_entries(&self, queue: &wgpu::Queue, entries: &[GizmoDrawEntry]) {
        let n = entries.len().min(GIZMO_ENTRY_SLOTS as usize);
        if n == 0 {
            return;
        }
        let stride = self.entry_stride as usize;
        let mut staging = self.staging.borrow_mut();
        staging.clear();
        staging.resize(stride * n, 0);
        for (i, entry) in entries[..n].iter().enumerate() {
            let bytes = bytemuck::bytes_of(entry);
            staging[i * stride..i * stride + bytes.len()].copy_from_slice(bytes);
        }
        queue.write_buffer(&self.entries_buffer, 0, &staging);
    }

    /// Uploads the marks of a turn and returns the vertex ranges of the slice
    /// and of the ticks and spokes. Whatever does not fit is dropped.
    pub(super) fn write_marks(
        &self,
        queue: &wgpu::Queue,
        marks: &RotateMarks,
    ) -> (Range<u32>, Range<u32>) {
        let cap = MARKS_CAPACITY as usize;
        let wedge = marks.wedge.len().min(cap);
        let rest = marks.marks.len().min(cap - wedge);
        let mut all = Vec::with_capacity(wedge + rest);
        all.extend_from_slice(&marks.wedge[..wedge]);
        all.extend_from_slice(&marks.marks[..rest]);
        if !all.is_empty() {
            queue.write_buffer(&self.marks_buffer, 0, bytemuck::cast_slice(&all));
        }
        let wedge = wedge as u32;
        (0..wedge, wedge..wedge + rest as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gizmo::axis::Axis;

    #[test]
    fn a_full_turn_of_marks_fits_the_buffer() {
        let marks = gizmo_geometry::build_rotate_marks(
            Axis::X,
            0.0,
            std::f32::consts::TAU * 3.0,
            crate::gizmo::look::FOCUSED_METRICS.ring_radius,
        );
        assert!((marks.wedge.len() + marks.marks.len()) as u64 <= MARKS_CAPACITY);
    }
}
