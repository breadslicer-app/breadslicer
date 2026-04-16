/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::borrow::Cow;
use std::num::NonZeroU64;

use anyhow::{Result, anyhow};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use sc_mesh_core::scene::{Scene, SceneObjectContent};
use slint::wgpu_28::wgpu;
use wgpu::util::DeviceExt;

const SHADER_SOURCE: &str = r#"
struct Uniforms {
    mvp: mat4x4<f32>,
    model: mat4x4<f32>,
    light_dir: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip_pos = u.mvp * vec4<f32>(in.position, 1.0);
    out.world_normal = (u.model * vec4<f32>(in.normal, 0.0)).xyz;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let n = normalize(in.world_normal);
    let l = normalize(u.light_dir.xyz);
    let lambert = max(dot(n, l), 0.0);
    let ambient = 0.25;
    let intensity = ambient + (1.0 - ambient) * lambert;
    let base_color = vec3<f32>(0.75, 0.78, 0.85);
    return vec4<f32>(base_color * intensity, 1.0);
}
"#;

const TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct GpuVertex {
    position: [f32; 3],
    normal: [f32; 3],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Uniforms {
    mvp: [[f32; 4]; 4],
    model: [[f32; 4]; 4],
    light_dir: [f32; 4],
}

struct MeshGpu {
    vertex_buffer: wgpu::Buffer,
    vertex_count: u32,
    aabb_min: Vec3,
    aabb_max: Vec3,
}

struct DepthTex {
    width: u32,
    height: u32,
    view: wgpu::TextureView,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    mesh: Option<MeshGpu>,
    depth: Option<DepthTex>,
}

impl Renderer {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mesh-viewer shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(SHADER_SOURCE)),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh-viewer bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(std::mem::size_of::<Uniforms>() as u64),
                },
                count: None,
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh-viewer pll"),
            bind_group_layouts: &[&bind_group_layout],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh-viewer pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GpuVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x3,
                        1 => Float32x3,
                    ],
                }],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: TEXTURE_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh-viewer uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mesh-viewer bg"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        Self {
            device,
            queue,
            pipeline,
            uniform_buffer,
            bind_group,
            mesh: None,
            depth: None,
        }
    }

    pub fn clear_scene(&mut self) {
        self.mesh = None;
    }

    pub fn upload_scene(&mut self, scene: &Scene) -> Result<()> {
        let mut vertices: Vec<GpuVertex> = Vec::new();
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);

        for object in &scene.objects {
            let mesh = match &object.content {
                SceneObjectContent::Mesh(m) => m,
                SceneObjectContent::Resource(idx) => scene
                    .resources
                    .get(*idx)
                    .ok_or_else(|| anyhow!("Resource index {idx} out of bounds"))?,
            };

            for face in &mesh.faces {
                let normal = face.normal.0;
                for &vidx in &face.vertices {
                    let pos = mesh.vertices[vidx as usize].0;
                    vertices.push(GpuVertex {
                        position: pos,
                        normal,
                    });
                    let p = Vec3::from(pos);
                    min = min.min(p);
                    max = max.max(p);
                }
            }
        }

        if vertices.is_empty() {
            return Err(anyhow!("Scene has no geometry to render"));
        }

        let vertex_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("mesh-viewer vertex buffer"),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });

        self.mesh = Some(MeshGpu {
            vertex_buffer,
            vertex_count: vertices.len() as u32,
            aabb_min: min,
            aabb_max: max,
        });

        Ok(())
    }

    fn ensure_depth(&mut self, width: u32, height: u32) {
        let same = self
            .depth
            .as_ref()
            .is_some_and(|d| d.width == width && d.height == height);
        if same {
            return;
        }

        let depth_texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mesh-viewer depth"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.depth = Some(DepthTex {
            width,
            height,
            view,
        });
    }

    pub fn render(&mut self, width: u32, height: u32) -> Option<wgpu::Texture> {
        if width == 0 || height == 0 || self.mesh.is_none() {
            return None;
        }

        self.ensure_depth(width, height);
        let depth_view = &self.depth.as_ref().unwrap().view;
        let mesh = self.mesh.as_ref().unwrap();

        let center = (mesh.aabb_min + mesh.aabb_max) * 0.5;
        let extent = (mesh.aabb_max - mesh.aabb_min).length().max(1e-3);
        let dir = Vec3::new(1.0, 0.8, 1.0).normalize();
        let eye = center + dir * extent * 1.5;
        let aspect = width as f32 / height as f32;
        let view_mat = Mat4::look_at_rh(eye, center, Vec3::Z);
        let proj = Mat4::perspective_rh(45f32.to_radians(), aspect, 0.01 * extent, 100.0 * extent);
        let mvp = proj * view_mat;

        let uniforms = Uniforms {
            mvp: mvp.to_cols_array_2d(),
            model: Mat4::IDENTITY.to_cols_array_2d(),
            light_dir: [0.5, 0.7, 1.0, 0.0],
        };
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        let target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mesh-viewer color"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("mesh-viewer encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mesh-viewer pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.12,
                            g: 0.12,
                            b: 0.13,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            pass.draw(0..mesh.vertex_count, 0..1);
        }
        self.queue.submit(std::iter::once(encoder.finish()));

        Some(target)
    }
}
