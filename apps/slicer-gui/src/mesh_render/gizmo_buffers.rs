/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Static gizmo VBO/IBO geometry plus the per-frame entry-buffer
//! allocation. All four gizmo geometries (axis shaft+cone+midpoint,
//! rotate arc, rotate cones, centre cube) are uploaded once at
//! renderer construction and reused every frame; only the
//! per-axis/per-handle draw-entry slots are written per frame, via
//! `write_entries`.

use std::num::NonZeroU64;

use slint::wgpu_28::wgpu;
use wgpu::util::DeviceExt;

use crate::gizmo_geometry;

use super::gizmo_entries::{GIZMO_DRAW_ENTRY_SIZE, GIZMO_ENTRY_SLOTS, GizmoDrawEntry};
use super::scene_upload::align_up;

pub(super) struct GizmoBuffers {
    /// Axis geometry: shaft prism + cone tip + per-axis midpoint
    /// cube, all in handle-local space (axis = +X). The renderer
    /// draws the shaft+cone subrange as the translate handle and
    /// the midpoint-cube subrange as the scale-axis handle.
    pub(super) axis_vertex_buffer: wgpu::Buffer,
    pub(super) axis_index_buffer: wgpu::Buffer,
    /// Number of indices in the translate (shaft + cone) subrange,
    /// from `[0, axis_shaft_tip_index_count)`. The remaining
    /// indices `[axis_shaft_tip_index_count, end)` belong to the
    /// per-axis midpoint cube.
    pub(super) axis_shaft_tip_index_count: u32,
    pub(super) axis_total_index_count: u32,
    /// Rotate-arc geometry: a 90° torus segment around +X in
    /// handle-local space, centred on +Y. The per-frame X-roll
    /// (baked into `axis_rotation`) swings the arc to face the
    /// camera before the world-axis rotation maps it onto X/Y/Z.
    /// Decorative — the click target is the cone spindle below.
    pub(super) arc_vertex_buffer: wgpu::Buffer,
    pub(super) arc_index_buffer: wgpu::Buffer,
    pub(super) arc_index_count: u32,
    /// Rotate-handle two-cone spindle: bases touching at the arc's
    /// camera-facing midpoint, tips along ±axis. The active rotate
    /// handle (mirrors `gizmo::ray_hits_rotate_cones` on the picker
    /// side).
    pub(super) rotate_cones_vertex_buffer: wgpu::Buffer,
    pub(super) rotate_cones_index_buffer: wgpu::Buffer,
    pub(super) rotate_cones_index_count: u32,
    /// Centre cube geometry for the uniform-scale handle.
    pub(super) centre_vertex_buffer: wgpu::Buffer,
    pub(super) centre_index_buffer: wgpu::Buffer,
    pub(super) centre_index_count: u32,
    pub(super) uniforms_buffer: wgpu::Buffer,
    /// Per-frame draw-entry buffer with `GIZMO_ENTRY_SLOTS` slots.
    /// Each pass binds a window of one entry via a dynamic offset:
    /// translate occupies `[0, 3)`, rotate `[3, 6)`, scale-axis
    /// `[6, 9)`, scale-uniform / centre `[9, 10)`.
    pub(super) entries_buffer: wgpu::Buffer,
    pub(super) entry_stride: u64,
}

impl GizmoBuffers {
    pub(super) fn create(device: &wgpu::Device) -> Self {
        let (axis_vertices, axis_indices, axis_shaft_tip_index_count) =
            gizmo_geometry::build_axis_geometry();
        let axis_total_index_count = axis_indices.len() as u32;
        let axis_vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh-viewer gizmo axis vbo"),
            contents: bytemuck::cast_slice(&axis_vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let axis_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh-viewer gizmo axis ibo"),
            contents: bytemuck::cast_slice(&axis_indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let (arc_vertices, arc_indices) = gizmo_geometry::build_rotate_arc_geometry();
        let arc_index_count = arc_indices.len() as u32;
        let arc_vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh-viewer gizmo arc vbo"),
            contents: bytemuck::cast_slice(&arc_vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let arc_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh-viewer gizmo arc ibo"),
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

        let (centre_vertices, centre_indices) = gizmo_geometry::build_centre_cube_geometry();
        let centre_index_count = centre_indices.len() as u32;
        let centre_vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh-viewer gizmo centre vbo"),
            contents: bytemuck::cast_slice(&centre_vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let centre_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh-viewer gizmo centre ibo"),
            contents: bytemuck::cast_slice(&centre_indices),
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
            axis_vertex_buffer,
            axis_index_buffer,
            axis_shaft_tip_index_count,
            axis_total_index_count,
            arc_vertex_buffer,
            arc_index_buffer,
            arc_index_count,
            rotate_cones_vertex_buffer,
            rotate_cones_index_buffer,
            rotate_cones_index_count,
            centre_vertex_buffer,
            centre_index_buffer,
            centre_index_count,
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
