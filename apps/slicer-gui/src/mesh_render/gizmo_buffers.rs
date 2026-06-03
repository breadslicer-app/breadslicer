/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Static gizmo VBO/IBO geometry plus the per-frame entry-buffer
//! allocation. Five geometries are uploaded once at renderer
//! construction and reused every frame:
//!
//! - shaft line (LineList)
//! - cone tip (TriangleList)
//! - per-axis scale cube (TriangleList)
//! - rotate arc polyline (LineList)
//! - rotate-drag two-cone spindle (TriangleList)
//!
//! Only the per-axis draw-entry slots are written per frame, via
//! `write_entries`.

use std::num::NonZeroU64;

use slint::wgpu_28::wgpu;
use wgpu::util::DeviceExt;

use crate::gizmo_geometry;

use super::gizmo_entries::{GIZMO_DRAW_ENTRY_SIZE, GIZMO_ENTRY_SLOTS, GizmoDrawEntry};
use super::scene_upload::align_up;

pub(super) struct GizmoBuffers {
    /// Axis shaft as a 1-pixel `LineList` segment from pivot to cone
    /// base, in handle-local space.
    pub(super) shaft_line_vertex_buffer: wgpu::Buffer,
    pub(super) shaft_line_index_buffer: wgpu::Buffer,
    pub(super) shaft_line_index_count: u32,
    /// Translate cone tip (12-side solid cone). The active translate
    /// click target.
    pub(super) cone_tip_vertex_buffer: wgpu::Buffer,
    pub(super) cone_tip_index_buffer: wgpu::Buffer,
    pub(super) cone_tip_index_count: u32,
    /// Per-axis scale midpoint cube.
    pub(super) scale_cube_vertex_buffer: wgpu::Buffer,
    pub(super) scale_cube_index_buffer: wgpu::Buffer,
    pub(super) scale_cube_index_count: u32,
    /// Rotate arc polyline (`LineList`) approximating a 90° sweep
    /// around handle-local +X, centred on +Y. The per-frame X-roll
    /// (baked into `axis_rotation`) swings the arc to face the camera
    /// before the world-axis rotation maps it onto X/Y/Z. Decorative;
    /// the click target is the cone spindle below.
    pub(super) arc_line_vertex_buffer: wgpu::Buffer,
    pub(super) arc_line_index_buffer: wgpu::Buffer,
    pub(super) arc_line_index_count: u32,
    /// Rotate-drag two-cone spindle.
    pub(super) rotate_cones_vertex_buffer: wgpu::Buffer,
    pub(super) rotate_cones_index_buffer: wgpu::Buffer,
    pub(super) rotate_cones_index_count: u32,
    pub(super) uniforms_buffer: wgpu::Buffer,
    /// Per-frame draw-entry buffer with `GIZMO_ENTRY_SLOTS` slots.
    /// Each pass binds a window of one entry via a dynamic offset:
    /// translate occupies `[0, 3)`, rotate `[3, 6)`, scale-axis
    /// `[6, 9)`.
    pub(super) entries_buffer: wgpu::Buffer,
    pub(super) entry_stride: u64,
}

impl GizmoBuffers {
    pub(super) fn create(device: &wgpu::Device) -> Self {
        let (shaft_vertices, shaft_indices) = gizmo_geometry::build_axis_shaft_line_geometry();
        let shaft_line_index_count = shaft_indices.len() as u32;
        let shaft_line_vertex_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("mesh-viewer gizmo shaft line vbo"),
                contents: bytemuck::cast_slice(&shaft_vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let shaft_line_index_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("mesh-viewer gizmo shaft line ibo"),
                contents: bytemuck::cast_slice(&shaft_indices),
                usage: wgpu::BufferUsages::INDEX,
            });

        let (cone_vertices, cone_indices) = gizmo_geometry::build_axis_cone_tip_geometry();
        let cone_tip_index_count = cone_indices.len() as u32;
        let cone_tip_vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh-viewer gizmo cone tip vbo"),
            contents: bytemuck::cast_slice(&cone_vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let cone_tip_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh-viewer gizmo cone tip ibo"),
            contents: bytemuck::cast_slice(&cone_indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let (cube_vertices, cube_indices) = gizmo_geometry::build_axis_scale_cube_geometry();
        let scale_cube_index_count = cube_indices.len() as u32;
        let scale_cube_vertex_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("mesh-viewer gizmo scale cube vbo"),
                contents: bytemuck::cast_slice(&cube_vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let scale_cube_index_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("mesh-viewer gizmo scale cube ibo"),
                contents: bytemuck::cast_slice(&cube_indices),
                usage: wgpu::BufferUsages::INDEX,
            });

        let (arc_vertices, arc_indices) = gizmo_geometry::build_rotate_arc_line_geometry();
        let arc_line_index_count = arc_indices.len() as u32;
        let arc_line_vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh-viewer gizmo arc line vbo"),
            contents: bytemuck::cast_slice(&arc_vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let arc_line_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh-viewer gizmo arc line ibo"),
            contents: bytemuck::cast_slice(&arc_indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let (rotate_cones_vertices, rotate_cones_indices) =
            gizmo_geometry::build_rotate_cones_geometry();
        let rotate_cones_index_count = rotate_cones_indices.len() as u32;
        let rotate_cones_vertex_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("mesh-viewer gizmo rotate cones vbo"),
                contents: bytemuck::cast_slice(&rotate_cones_vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let rotate_cones_index_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("mesh-viewer gizmo rotate cones ibo"),
                contents: bytemuck::cast_slice(&rotate_cones_indices),
                usage: wgpu::BufferUsages::INDEX,
            });

        let uniforms_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh-viewer gizmo uniforms"),
            size: std::mem::size_of::<super::gizmo_entries::GizmoUniformsGpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let alignment = device.limits().min_uniform_buffer_offset_alignment as u64;
        let entry_stride = align_up(GIZMO_DRAW_ENTRY_SIZE, alignment);
        let entries_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh-viewer gizmo entries"),
            size: entry_stride * GIZMO_ENTRY_SLOTS,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            shaft_line_vertex_buffer,
            shaft_line_index_buffer,
            shaft_line_index_count,
            cone_tip_vertex_buffer,
            cone_tip_index_buffer,
            cone_tip_index_count,
            scale_cube_vertex_buffer,
            scale_cube_index_buffer,
            scale_cube_index_count,
            arc_line_vertex_buffer,
            arc_line_index_buffer,
            arc_line_index_count,
            rotate_cones_vertex_buffer,
            rotate_cones_index_buffer,
            rotate_cones_index_count,
            uniforms_buffer,
            entries_buffer,
            entry_stride,
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

    pub(super) fn write_entries(
        &self,
        queue: &wgpu::Queue,
        slot_base: u64,
        entries: &[GizmoDrawEntry],
    ) {
        for (i, entry) in entries.iter().enumerate() {
            let offset = (slot_base + i as u64) * self.entry_stride;
            queue.write_buffer(&self.entries_buffer, offset, bytemuck::bytes_of(entry));
        }
    }
}
