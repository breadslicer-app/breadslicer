/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::fmt;

use anyhow::{Result, anyhow};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3, Vec4};
use sc_mesh_core::Aabb;
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::primitives::Vertex;
use sc_mesh_core::scene::Scene;
use sc_mesh_core::scene::node::{LeafPayload, NodeId, NodeKind};
use sc_mesh_core::transform::AffineTransform3D;
use sc_mesh_core::transform::Transform3D;
use slint::wgpu_29::wgpu;

use super::device_loss::DeviceLoss;
use super::device_ops::create_buffer_reporting;
use crate::build_volume::BuildVolume;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub(super) struct GpuVertex {
    position: [f32; 3],
}

/// Per-object entry in the model uniform buffer. Mirrors the WGSL
/// `ModelEntry` struct one-for-one. `flags.x` is the selection bit
/// (1 for the selected object, 0 otherwise); the remaining lanes are
/// reserved for future per-object hints (hover, gizmo focus, …).
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
pub(super) struct ModelEntry {
    model: [[f32; 4]; 4],
    flags: [u32; 4],
}

pub(super) const MODEL_ENTRY_SIZE: u64 = std::mem::size_of::<ModelEntry>() as u64;

/// Round `size` up to the next multiple of `alignment`. Used to space
/// successive `ModelEntry` slots in the per-scene uniform buffer at the
/// device's `min_uniform_buffer_offset_alignment`. Both inputs are u64
/// and non-zero in practice; the `alignment - 1` mask trick is the
/// standard `assume-power-of-two` form used everywhere wgpu samples
/// align uniform-buffer offsets.
pub(super) const fn align_up(size: u64, alignment: u64) -> u64 {
    (size + alignment - 1) & !(alignment - 1)
}

/// GPU-resident draw data for a single graph leaf: its local-space
/// vertex+index buffers (the leaf's transform is *not* baked in; it
/// arrives at draw time through the model storage buffer). The
/// `node_id` lets the draw loop key per-leaf state (selection
/// stencil) against the graph's stable id.
pub(super) struct ObjectDraw {
    node_id: NodeId,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
}

impl ObjectDraw {
    pub(super) fn node_id(&self) -> NodeId {
        self.node_id
    }

    pub(super) fn vertex_buffer(&self) -> &wgpu::Buffer {
        &self.vertex_buffer
    }

    pub(super) fn index_buffer(&self) -> &wgpu::Buffer {
        &self.index_buffer
    }

    pub(super) fn index_count(&self) -> u32 {
        self.index_count
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeafBufferKind {
    Vertex,
    Index,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeafBufferExceedsDeviceLimit {
    pub node_id: NodeId,
    pub leaf_name: Option<String>,
    pub buffer_kind: LeafBufferKind,
    pub num_elements: usize,
    pub num_bytes: u64,
    pub max_buffer_size: u64,
}

impl fmt::Display for LeafBufferExceedsDeviceLimit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let what = match self.buffer_kind {
            LeafBufferKind::Vertex => "vertices",
            LeafBufferKind::Index => "triangles",
        };
        match &self.leaf_name {
            Some(name) => write!(f, "Object '{name}' has more {what}")?,
            None => write!(f, "An object has more {what}")?,
        }
        write!(
            f,
            " ({}) than this graphics device can hold in one buffer",
            self.num_elements
        )
    }
}

impl std::error::Error for LeafBufferExceedsDeviceLimit {}

#[derive(Debug)]
pub struct DeviceRefusedBuffer {
    /// What the buffer holds, in the user's terms ("index buffer of
    /// object 'hull'", "per-object transform table").
    pub what: String,
    pub num_bytes: u64,
    pub source: wgpu::Error,
}

impl fmt::Display for DeviceRefusedBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "The graphics device refused the {} ({} bytes)",
            self.what, self.num_bytes
        )
    }
}

impl std::error::Error for DeviceRefusedBuffer {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

#[derive(Debug)]
pub struct DeviceLostDuringUpload;

impl fmt::Display for DeviceLostDuringUpload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("The graphics device was lost while uploading the scene")
    }
}

impl std::error::Error for DeviceLostDuringUpload {}

fn describe_leaf_buffer(buffer: &str, leaf_name: Option<&str>) -> String {
    match leaf_name {
        Some(name) => format!("{buffer} of object '{name}'"),
        None => format!("{buffer} of an object"),
    }
}

/// Creates a buffer holding `contents`, reporting a refusal instead of
/// panicking. Empty contents give an empty, unmapped buffer. If the
/// device is lost before or during the call, the error is
/// [`DeviceLostDuringUpload`].
fn upload_buffer(
    device: &wgpu::Device,
    device_loss: &DeviceLoss,
    label: &'static str,
    contents: &[u8],
    usage: wgpu::BufferUsages,
    what: impl FnOnce() -> String,
) -> Result<wgpu::Buffer> {
    let unpadded = contents.len() as u64;
    // wgpu requires mapped-at-creation sizes to be a non-zero multiple
    // of COPY_BUFFER_ALIGNMENT.
    let align_mask = wgpu::COPY_BUFFER_ALIGNMENT - 1;
    let size = if contents.is_empty() {
        0
    } else {
        ((unpadded + align_mask) & !align_mask).max(wgpu::COPY_BUFFER_ALIGNMENT)
    };
    let buffer = create_buffer_reporting(
        device,
        &wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage,
            mapped_at_creation: !contents.is_empty(),
        },
    )
    .map_err(|source| DeviceRefusedBuffer {
        what: what(),
        num_bytes: size,
        source,
    })?;
    if !contents.is_empty() {
        if device_loss.is_lost() {
            return Err(DeviceLostDuringUpload.into());
        }
        // wgpu panics, instead of reporting, when a buffer on a lost
        // device is mapped. The closure owns nothing that a panic could
        // leave half-written; wgpu-core has already returned its error
        // by the time the API layer panics.
        let written = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            buffer
                .get_mapped_range_mut(..)
                .slice(..contents.len())
                .copy_from_slice(contents);
            buffer.unmap();
        }));
        if written.is_err() {
            return Err(DeviceLostDuringUpload.into());
        }
    }
    Ok(buffer)
}

/// GPU mirror of a `Scene`. Holds one [`ObjectDraw`] per leaf in
/// pre-order traversal of the graph's roots, plus a uniform buffer of
/// [`ModelEntry`] slots, padded to the device's
/// `min_uniform_buffer_offset_alignment` so the render loop can address
/// each slot via a dynamic-offset bind. The world-space AABB is captured
/// at build time from `Scene::aabb`; it is used once for camera
/// framing and is not refreshed on per-frame transform updates (the
/// camera persists across edits, so a post-edit AABB is irrelevant for
/// framing).
///
/// Construction ([`GpuScene::build`]) touches a `wgpu::Device` but not
/// the UI loop, so it can run on the worker thread that parsed the
/// scene; the UI thread then attaches the matching bind group via
/// [`Renderer::install_scene`].
pub struct GpuScene {
    objects: Vec<ObjectDraw>,
    model_buffer: wgpu::Buffer,
    /// Distance in bytes between consecutive `ModelEntry` slots in
    /// `model_buffer`. Always ≥ `MODEL_ENTRY_SIZE` and a multiple of
    /// the device's `min_uniform_buffer_offset_alignment`.
    model_stride: u64,
    /// Identity token of the device this scene's GPU resources were
    /// built against. `Renderer::install_scene` checks this against
    /// the renderer's own token before attaching the scene.
    device_token: super::DeviceToken,
    aabb_min: Vec3,
    aabb_max: Vec3,
}

/// Triangle counts reported while a scene's GPU data is built. Each
/// object's triangles count twice, once when its vertex/index vectors
/// are flattened and once when its device buffers are created, so
/// `total_triangles` is twice the scene's reachable triangle count
/// and the final tick reaches it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UploadProgress {
    /// Triangles processed so far, summed across both passes.
    pub processed_triangles: u64,
    /// Twice the reachable triangle count.
    pub total_triangles: u64,
}

impl GpuScene {
    pub fn build(
        device: &wgpu::Device,
        device_token: super::DeviceToken,
        graph: &Scene,
        device_loss: &DeviceLoss,
    ) -> Result<Self> {
        Self::build_with_progress(device, device_token, graph, device_loss, |_| {})
    }

    /// [`GpuScene::build`], reporting per-object triangle progress;
    /// see [`UploadProgress`] for the tick contract.
    pub fn build_with_progress(
        device: &wgpu::Device,
        device_token: super::DeviceToken,
        graph: &Scene,
        device_loss: &DeviceLoss,
        mut progress: impl FnMut(UploadProgress),
    ) -> Result<Self> {
        let mut scene_triangles: u64 = 0;
        for_each_leaf(graph, |_node_id, _world_transform, mesh| {
            scene_triangles += mesh.faces().len() as u64;
            Ok(())
        })?;
        let total_triangles = scene_triangles.saturating_mul(2);
        let mut processed_triangles: u64 = 0;
        let mut tick = |processed: &mut u64, triangles: u64| {
            *processed += triangles;
            progress(UploadProgress {
                processed_triangles: *processed,
                total_triangles,
            });
        };

        let data = build_gpu_scene_data(graph, |triangles| {
            tick(&mut processed_triangles, triangles);
        })?;
        check_leaf_buffers_fit(&data.objects, device.limits().max_buffer_size)?;
        let mut objects = Vec::with_capacity(data.objects.len());
        for obj in &data.objects {
            let vertex_buffer = upload_buffer(
                device,
                device_loss,
                "mesh-viewer object vertex buffer",
                bytemuck::cast_slice(&obj.vertices),
                wgpu::BufferUsages::VERTEX,
                || describe_leaf_buffer("vertex buffer", obj.name.as_deref()),
            )?;
            let index_buffer = upload_buffer(
                device,
                device_loss,
                "mesh-viewer object index buffer",
                bytemuck::cast_slice(&obj.indices),
                wgpu::BufferUsages::INDEX,
                || describe_leaf_buffer("index buffer", obj.name.as_deref()),
            )?;
            objects.push(ObjectDraw {
                node_id: obj.node_id,
                vertex_buffer,
                index_buffer,
                index_count: obj.indices.len() as u32,
            });
            tick(&mut processed_triangles, (obj.indices.len() / 3) as u64);
        }

        let alignment = device.limits().min_uniform_buffer_offset_alignment as u64;
        let model_stride = align_up(MODEL_ENTRY_SIZE, alignment);

        // The bind group requires a non-zero binding size; we already
        // bail above if the scene has no objects.
        let model_size = model_stride * objects.len() as u64;
        let model_buffer = create_buffer_reporting(
            device,
            &wgpu::BufferDescriptor {
                label: Some("mesh-viewer model uniform"),
                size: model_size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        )
        .map_err(|source| DeviceRefusedBuffer {
            what: "per-object transform table".to_string(),
            num_bytes: model_size,
            source,
        })?;

        Ok(Self {
            objects,
            model_buffer,
            model_stride,
            device_token,
            aabb_min: data.aabb_min,
            aabb_max: data.aabb_max,
        })
    }

    /// Identity token of the device this scene was built against.
    /// `Renderer::install_scene` matches it against the renderer's
    /// own token before attaching.
    pub fn device_token(&self) -> super::DeviceToken {
        self.device_token
    }

    pub(super) fn object_count(&self) -> usize {
        self.objects.len()
    }

    pub(super) fn objects(&self) -> &[ObjectDraw] {
        &self.objects
    }

    pub(super) fn model_buffer(&self) -> &wgpu::Buffer {
        &self.model_buffer
    }

    pub(super) fn model_stride(&self) -> u64 {
        self.model_stride
    }

    pub(super) fn aabb(&self) -> (Vec3, Vec3) {
        (self.aabb_min, self.aabb_max)
    }
}

/// Translates `AffineTransform3D`'s row-major 3×3 + translation form
/// into glam's column-major 4×4. f64 internals are downcast to f32
/// here at the GPU boundary.
fn affine_to_mat4(a: &AffineTransform3D) -> Mat4 {
    let l = a.linear();
    let t = a.translation();
    Mat4::from_cols(
        Vec4::new(l[0][0] as f32, l[1][0] as f32, l[2][0] as f32, 0.0),
        Vec4::new(l[0][1] as f32, l[1][1] as f32, l[2][1] as f32, 0.0),
        Vec4::new(l[0][2] as f32, l[1][2] as f32, l[2][2] as f32, 0.0),
        Vec4::new(t[0] as f32, t[1] as f32, t[2] as f32, 1.0),
    )
}

fn transform_to_mat4(t: &Transform3D) -> Mat4 {
    match t {
        Transform3D::Identity => Mat4::IDENTITY,
        Transform3D::Affine(a) => affine_to_mat4(a),
    }
}

/// `selection` is the [`NodeId`] of the selected leaf, or `None` for
/// no selection. A `NodeId` that does not name a reachable leaf
/// leaves every entry's `flags.x` at 0. The shader reads `flags.x`
/// and mixes a warm tint when it is 1; `flags.y` is set when the
/// leaf has any vertex outside the build envelope, driving the mild
/// interior wash that marks the whole object as "offending" even
/// where individual fragments are still inside the envelope.
pub(super) fn build_model_entries(
    graph: &Scene,
    selection: Option<NodeId>,
    build_volume: BuildVolume,
) -> Result<Vec<ModelEntry>> {
    let mut entries: Vec<ModelEntry> = Vec::new();
    for_each_leaf(graph, |node_id, world_transform, mesh| {
        let selected = selection == Some(node_id);
        let breaches = leaf_breaches_envelope(mesh, &world_transform, &build_volume);
        entries.push(ModelEntry {
            model: transform_to_mat4(&world_transform).to_cols_array_2d(),
            flags: [u32::from(selected), u32::from(breaches), 0, 0],
        });
        Ok(())
    })?;
    Ok(entries)
}

/// Returns `true` if `mesh`, after `world_transform` is applied, has
/// any part outside `build_volume`'s envelope. Computes the
/// transformed AABB of the mesh vertices and asks
/// [`BuildVolume::contains_aabb`]: containment of the AABB is
/// equivalent to containment of every vertex (and therefore of every
/// triangle interior) when the envelope is convex.
fn leaf_breaches_envelope(
    mesh: &IndexedMesh,
    world_transform: &Transform3D,
    build_volume: &BuildVolume,
) -> bool {
    let transform_mat = transform_to_mat4(world_transform);
    let mut aabb = Aabb::empty();
    for v in mesh.vertices() {
        let p = transform_mat.transform_point3(Vec3::from(v.coords()));
        aabb.extend(Vertex::new([p.x, p.y, p.z]));
    }
    !build_volume.contains_aabb(aabb)
}

/// Bit positions for the per-face breach mask consumed by the plate
/// shader. Bits 0..=4 drive the side-wall + top warning quads; bit 5
/// (-Z, the build-plate floor) is read by the floor path of the same
/// shader and overlays a stripe pattern on top of the grid instead of
/// going through a dedicated wall quad.
pub(super) const FACE_BIT_X_NEG: u32 = 1 << 0;
pub(super) const FACE_BIT_X_POS: u32 = 1 << 1;
pub(super) const FACE_BIT_Y_NEG: u32 = 1 << 2;
pub(super) const FACE_BIT_Y_POS: u32 = 1 << 3;
pub(super) const FACE_BIT_Z_POS: u32 = 1 << 4;
pub(super) const FACE_BIT_Z_NEG: u32 = 1 << 5;

/// Walks every reachable leaf's world-transformed vertices and returns
/// a bitmask of envelope faces that some vertex falls outside. The mask
/// matches the shader's bit layout: `FACE_BIT_X_NEG` etc.
///
/// Per-vertex (not per-AABB) is mathematically exact for a convex
/// envelope: a vertex outside on axis `a` is the only way for the
/// mesh's `a`-face to be breached, since the convex hull of the
/// vertices contains the mesh and the envelope is convex.
pub(super) fn build_envelope_warning_mask(graph: &Scene, build_volume: BuildVolume) -> Result<u32> {
    let env_min = build_volume.envelope_min_mm();
    let env_max = build_volume.envelope_max_mm();
    let mut mask = 0u32;
    for_each_leaf(graph, |_node_id, world_transform, mesh| {
        let transform_mat = transform_to_mat4(&world_transform);
        for v in mesh.vertices() {
            let p = transform_mat.transform_point3(Vec3::from(v.coords()));
            if p.x < env_min.x {
                mask |= FACE_BIT_X_NEG;
            }
            if p.x > env_max.x {
                mask |= FACE_BIT_X_POS;
            }
            if p.y < env_min.y {
                mask |= FACE_BIT_Y_NEG;
            }
            if p.y > env_max.y {
                mask |= FACE_BIT_Y_POS;
            }
            if p.z < env_min.z {
                mask |= FACE_BIT_Z_NEG;
            }
            if p.z > env_max.z {
                mask |= FACE_BIT_Z_POS;
            }
        }
        Ok(())
    })?;
    Ok(mask)
}

/// How many leaves are reachable from `graph.roots()`. Counts the same
/// population [`build_gpu_scene_data`] would turn into draws, so a zero
/// answer means there is nothing for the GPU to mirror.
///
/// Walks through [`for_each_leaf`], so a dangling node id or dangling
/// subtree-resource id surfaces as an error instead of being flattened
/// into a count of zero.
pub(super) fn renderable_leaf_count(graph: &Scene) -> Result<usize> {
    let mut count = 0usize;
    for_each_leaf(graph, |_node_id, _world_transform, _mesh| {
        count += 1;
        Ok(())
    })?;
    Ok(count)
}

/// Pre-order traversal of every leaf reachable from `graph.roots()`,
/// resolving `MeshResource` payloads and composing each leaf's world
/// transform from the ancestor chain. The closure receives the leaf's
/// `NodeId`, its composed world transform, and a borrow of the leaf's
/// mesh. Propagates the closure's error and any dangling-id error from
/// the walk.
fn for_each_leaf<F>(graph: &Scene, mut f: F) -> Result<()>
where
    F: FnMut(NodeId, Transform3D, &IndexedMesh) -> Result<()>,
{
    for &root in graph.roots() {
        descend_leaves(graph, root, Transform3D::Identity, &mut f)?;
    }
    Ok(())
}

fn descend_leaves<F>(
    graph: &Scene,
    node_id: NodeId,
    parent_world: Transform3D,
    f: &mut F,
) -> Result<()>
where
    F: FnMut(NodeId, Transform3D, &IndexedMesh) -> Result<()>,
{
    let node = graph
        .node(node_id)
        .ok_or_else(|| anyhow!("dangling node id {}", node_id.get()))?;
    let world_from_self = parent_world.compose(&node.local_transform);
    match &node.kind {
        NodeKind::Group { children } | NodeKind::Csg { children, .. } => {
            for &c in children {
                descend_leaves(graph, c, world_from_self, f)?;
            }
        }
        NodeKind::Reference { resource } => {
            let root = graph
                .subtree_resource_root(*resource)
                .ok_or_else(|| anyhow!("dangling subtree resource id {}", resource.get()))?;
            descend_leaves(graph, root, world_from_self, f)?;
        }
        NodeKind::Leaf(payload) => {
            let mesh = match payload {
                LeafPayload::Mesh(m) => m.as_ref(),
                LeafPayload::MeshResource(rid) => graph
                    .mesh_resource(*rid)
                    .ok_or_else(|| anyhow!("dangling mesh resource id {}", rid.get()))?,
            };
            f(node_id, world_from_self, mesh)?;
        }
    }
    Ok(())
}

/// CPU-only build of the per-object draw data plus the world-space
/// scene AABB. Kept separate from buffer creation so the transformation
/// can be unit-tested without a `wgpu::Device`.
struct ObjectData {
    node_id: NodeId,
    name: Option<String>,
    vertices: Vec<GpuVertex>,
    indices: Vec<u32>,
}

/// Refuses the first leaf (pre-order, vertex buffer before index buffer)
/// whose buffer would exceed `max_buffer_size`. Both element sizes are
/// multiples of wgpu's copy alignment, so the byte counts here are the
/// sizes that will be requested.
fn check_leaf_buffers_fit(
    objects: &[ObjectData],
    max_buffer_size: u64,
) -> std::result::Result<(), LeafBufferExceedsDeviceLimit> {
    for obj in objects {
        let vertex_bytes = (obj.vertices.len() * std::mem::size_of::<GpuVertex>()) as u64;
        if vertex_bytes > max_buffer_size {
            return Err(LeafBufferExceedsDeviceLimit {
                node_id: obj.node_id,
                leaf_name: obj.name.clone(),
                buffer_kind: LeafBufferKind::Vertex,
                num_elements: obj.vertices.len(),
                num_bytes: vertex_bytes,
                max_buffer_size,
            });
        }
        let index_bytes = (obj.indices.len() * std::mem::size_of::<u32>()) as u64;
        if index_bytes > max_buffer_size {
            return Err(LeafBufferExceedsDeviceLimit {
                node_id: obj.node_id,
                leaf_name: obj.name.clone(),
                buffer_kind: LeafBufferKind::Index,
                num_elements: obj.indices.len() / 3,
                num_bytes: index_bytes,
                max_buffer_size,
            });
        }
    }
    Ok(())
}

struct SceneData {
    objects: Vec<ObjectData>,
    aabb_min: Vec3,
    aabb_max: Vec3,
}

/// `on_object_flattened` receives each flattened object's triangle
/// count (0 for geometry-less leaves), in traversal order; the
/// progress plumbing in [`GpuScene::build_with_progress`] is its only
/// non-test consumer.
fn build_gpu_scene_data(
    graph: &Scene,
    mut on_object_flattened: impl FnMut(u64),
) -> Result<SceneData> {
    let mut objects: Vec<ObjectData> = Vec::new();
    for_each_leaf(graph, |node_id, _world_transform, mesh| {
        on_object_flattened(mesh.faces().len() as u64);
        let name = graph
            .node(node_id)
            .and_then(|node| node.meta.as_ref())
            .and_then(|meta| meta.name.clone())
            .or_else(|| mesh.meta.as_ref().and_then(|meta| meta.name.clone()));
        if mesh.faces().is_empty() {
            // A leaf with no geometry contributes nothing to the
            // draw set, but we keep its slot in the model buffer so
            // the per-frame `build_model_entries` walk (which uses
            // the same pre-order traversal) lines up index-by-index
            // with the renderer's per-leaf draw loop. The slot is
            // drawn as a no-op (index_count = 0).
            objects.push(ObjectData {
                node_id,
                name,
                vertices: Vec::new(),
                indices: Vec::new(),
            });
            return Ok(());
        }

        let vertices: Vec<GpuVertex> = mesh
            .vertices()
            .iter()
            .map(|v| GpuVertex {
                position: v.coords(),
            })
            .collect();

        let mut indices: Vec<u32> = Vec::with_capacity(mesh.faces().len() * 3);
        for face in mesh.faces() {
            for vidx in face.vertices {
                indices.push(u32::from(vidx));
            }
        }

        objects.push(ObjectData {
            node_id,
            name,
            vertices,
            indices,
        });
        Ok(())
    })?;

    if objects.is_empty() {
        return Err(anyhow!("Graph has no leaves to render"));
    }

    // World-space scene AABB via `Scene::aabb`, which composes
    // each leaf's ancestor chain via Arvo. If every leaf is empty,
    // fall back to a degenerate bbox at the origin (the renderer's
    // diagonal-clamp `.max(1e-3)` keeps the projection matrix
    // well-defined).
    let (aabb_min, aabb_max) = match graph.aabb()? {
        Some(aabb) => (
            Vec3::from(aabb.min().coords()),
            Vec3::from(aabb.max().coords()),
        ),
        None => (Vec3::ZERO, Vec3::ZERO),
    };

    Ok(SceneData {
        objects,
        aabb_min,
        aabb_max,
    })
}
#[cfg(test)]
mod build_gpu_scene_data_tests {
    use std::sync::Arc;

    use super::*;
    use float_cmp::assert_approx_eq;
    use sc_mesh_core::MeshResourceId;
    use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
    use sc_mesh_core::transform::AffineTransform3D;

    /// Loader-faithful empty graph: identity-Group root, no leaves yet.
    fn graph_with_group_root() -> (Scene, NodeId) {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group root");
        (g, root)
    }

    fn add_inline_leaf(
        g: &mut Scene,
        parent: NodeId,
        mesh: IndexedMesh,
        transform: Transform3D,
    ) -> NodeId {
        let leaf = g
            .add_leaf(None, transform, LeafPayload::Mesh(Arc::new(mesh)))
            .expect("add inline leaf");
        g.add_child(parent, leaf).expect("attach inline leaf");
        leaf
    }

    fn add_resource_leaf(
        g: &mut Scene,
        parent: NodeId,
        rid: MeshResourceId,
        transform: Transform3D,
    ) -> NodeId {
        let leaf = g
            .add_leaf(None, transform, LeafPayload::MeshResource(rid))
            .expect("add resource leaf");
        g.add_child(parent, leaf).expect("attach resource leaf");
        leaf
    }

    fn triangle_mesh(vertices: Vec<[f32; 3]>, face_vertices: [u32; 3]) -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vertices.into_iter().map(Vertex::new).collect(),
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([0.0, 0.0, 1.0]),
                face_vertices,
            )],
        }
    }

    #[test]
    fn empty_graph_returns_error() {
        // Loader-faithful empty graph: a Group root with no children,
        // the same shape an empty-file load produces.
        let (graph, _root) = graph_with_group_root();
        assert!(build_gpu_scene_data(&graph, |_| {}).is_err());
    }

    #[test]
    fn valid_resource_reference_is_resolved() {
        let mesh = triangle_mesh(
            vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [0, 1, 2],
        );
        let (mut g, root) = graph_with_group_root();
        let rid = g.add_mesh_resource(mesh);
        add_resource_leaf(&mut g, root, rid, Transform3D::Identity);
        let data = build_gpu_scene_data(&g, |_| {}).unwrap();
        assert_eq!(data.objects.len(), 1);
        assert_eq!(data.objects[0].vertices.len(), 3);
        assert_eq!(data.objects[0].indices, vec![0, 1, 2]);
    }

    #[test]
    fn vertices_stay_local_under_identity() {
        let mesh = triangle_mesh(
            vec![[1.0, -2.0, 3.0], [4.0, 5.0, -6.0], [0.0, 0.0, 0.0]],
            [0, 1, 2],
        );
        let (mut g, root) = graph_with_group_root();
        add_inline_leaf(&mut g, root, mesh, Transform3D::Identity);
        let data = build_gpu_scene_data(&g, |_| {}).unwrap();
        assert_eq!(data.objects.len(), 1);
        assert_eq!(data.objects[0].vertices[0].position, [1.0, -2.0, 3.0]);
        assert_eq!(data.objects[0].vertices[1].position, [4.0, 5.0, -6.0]);
        assert_eq!(data.objects[0].vertices[2].position, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn vertices_stay_local_even_with_affine_transform() {
        // Vertex buffers are local-space; the transform is applied at
        // draw time via the per-leaf model uniform, so
        // `build_gpu_scene_data` must NOT bake it into the vertex
        // stream.
        let mesh = triangle_mesh(
            vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [0, 1, 2],
        );
        let (mut g, root) = graph_with_group_root();
        add_inline_leaf(
            &mut g,
            root,
            mesh,
            Transform3D::Affine(AffineTransform3D::from_translation([100.0, -50.0, 25.0])),
        );
        let data = build_gpu_scene_data(&g, |_| {}).unwrap();
        assert_eq!(data.objects[0].vertices[0].position, [1.0, 0.0, 0.0]);
        assert_eq!(data.objects[0].vertices[1].position, [0.0, 1.0, 0.0]);
        assert_eq!(data.objects[0].vertices[2].position, [0.0, 0.0, 1.0]);
    }

    #[test]
    fn indices_are_local_per_leaf_not_offset() {
        // With per-leaf draws, each leaf's index buffer references its
        // own vertex buffer, so indices stay local, not offset by
        // previous leaves' vertex counts as the merged build did.
        let a = triangle_mesh(
            vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [0, 1, 2],
        );
        let b = triangle_mesh(
            vec![[2.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 2.0]],
            [0, 1, 2],
        );
        let (mut g, root) = graph_with_group_root();
        add_inline_leaf(&mut g, root, a, Transform3D::Identity);
        add_inline_leaf(&mut g, root, b, Transform3D::Identity);
        let data = build_gpu_scene_data(&g, |_| {}).unwrap();
        assert_eq!(data.objects.len(), 2);
        assert_eq!(data.objects[0].indices, vec![0, 1, 2]);
        assert_eq!(data.objects[1].indices, vec![0, 1, 2]);
    }

    #[test]
    fn graph_aabb_reflects_world_space_under_transforms() {
        let mesh = triangle_mesh(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            [0, 1, 2],
        );
        let (mut g, root) = graph_with_group_root();
        add_inline_leaf(
            &mut g,
            root,
            mesh,
            Transform3D::Affine(AffineTransform3D::from_translation([10.0, 20.0, 30.0])),
        );
        let data = build_gpu_scene_data(&g, |_| {}).unwrap();
        // World AABB = local AABB ([0,0,0]..[1,1,0]) translated by (10,20,30).
        assert_approx_eq!(f32, data.aabb_min.x, 10.0, epsilon = 1e-5);
        assert_approx_eq!(f32, data.aabb_min.y, 20.0, epsilon = 1e-5);
        assert_approx_eq!(f32, data.aabb_min.z, 30.0, epsilon = 1e-5);
        assert_approx_eq!(f32, data.aabb_max.x, 11.0, epsilon = 1e-5);
        assert_approx_eq!(f32, data.aabb_max.y, 21.0, epsilon = 1e-5);
        assert_approx_eq!(f32, data.aabb_max.z, 30.0, epsilon = 1e-5);
    }

    #[test]
    fn graph_aabb_unions_two_leaves() {
        let a = triangle_mesh(
            vec![[1.0, -2.0, 3.0], [4.0, 5.0, -6.0], [0.0, 0.0, 0.0]],
            [0, 1, 2],
        );
        let b = triangle_mesh(
            vec![[-7.0, 8.0, 0.5], [0.0, 0.0, 9.0], [2.0, 2.0, 2.0]],
            [0, 1, 2],
        );
        let (mut g, root) = graph_with_group_root();
        add_inline_leaf(&mut g, root, a, Transform3D::Identity);
        add_inline_leaf(&mut g, root, b, Transform3D::Identity);
        let data = build_gpu_scene_data(&g, |_| {}).unwrap();
        assert_eq!(data.aabb_min, Vec3::new(-7.0, -2.0, -6.0));
        assert_eq!(data.aabb_max, Vec3::new(4.0, 8.0, 9.0));
    }

    #[test]
    fn empty_leaf_keeps_a_slot_in_the_leaf_list() {
        // A leaf with no faces still occupies a slot in the per-leaf
        // pre-order index; the renderer indexes into `models[]` by
        // that index, so the GpuScene must keep a parallel slot. The
        // slot must be drawable as a no-op (index_count = 0).
        let empty = IndexedMesh {
            meta: None,
            vertices: vec![],
            faces: vec![],
        };
        let nonempty = triangle_mesh(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            [0, 1, 2],
        );
        let (mut g, root) = graph_with_group_root();
        add_inline_leaf(&mut g, root, empty, Transform3D::Identity);
        add_inline_leaf(&mut g, root, nonempty, Transform3D::Identity);
        let data = build_gpu_scene_data(&g, |_| {}).unwrap();
        assert_eq!(data.objects.len(), 2);
        assert!(data.objects[0].indices.is_empty());
        assert_eq!(data.objects[1].indices.len(), 3);
    }

    #[test]
    fn singular_transform_is_accepted() {
        // `Scene::aabb` doesn't fail on singular linear parts
        // (it uses Arvo), so this currently succeeds. The check here
        // pins the behaviour: the per-leaf data is built without
        // touching `apply_to_normal`, so even a degenerate transform
        // doesn't bring the build down; degenerate matrices only
        // matter at `merge_meshes` / export time.
        let mesh = triangle_mesh(
            vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [0, 1, 2],
        );
        let singular = AffineTransform3D::from_linear_and_translation(
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 0.0]],
            [0.0, 0.0, 0.0],
        );
        let (mut g, root) = graph_with_group_root();
        add_inline_leaf(&mut g, root, mesh, Transform3D::Affine(singular));
        // Singular transforms aren't rejected here; the GPU path is
        // fine with a singular model matrix (it just collapses the
        // mesh to a plane). Mirror this in the assertion so the test
        // fails loudly if the behaviour changes.
        assert!(build_gpu_scene_data(&g, |_| {}).is_ok());
    }
}

#[cfg(test)]
mod transform_to_mat4_tests {
    use std::sync::Arc;

    use super::*;
    use float_cmp::assert_approx_eq;
    use sc_mesh_core::transform::AffineTransform3D;

    fn assert_mat4_approx(a: Mat4, b: Mat4, eps: f32) {
        let a = a.to_cols_array();
        let b = b.to_cols_array();
        for (x, y) in a.iter().zip(b.iter()) {
            assert_approx_eq!(f32, *x, *y, epsilon = eps);
        }
    }

    #[test]
    fn identity_transform_is_identity_matrix() {
        let m = transform_to_mat4(&Transform3D::Identity);
        assert_mat4_approx(m, Mat4::IDENTITY, 1e-6);
    }

    #[test]
    fn translation_lands_in_the_last_column() {
        let m = transform_to_mat4(&Transform3D::Affine(AffineTransform3D::from_translation([
            10.0, -5.0, 2.5,
        ])));
        // Glam stores column-major; the translation column is index 3.
        let translation = m.col(3);
        assert_approx_eq!(f32, translation.x, 10.0, epsilon = 1e-6);
        assert_approx_eq!(f32, translation.y, -5.0, epsilon = 1e-6);
        assert_approx_eq!(f32, translation.z, 2.5, epsilon = 1e-6);
        assert_approx_eq!(f32, translation.w, 1.0, epsilon = 1e-6);
    }

    #[test]
    fn affine_to_mat4_matches_apply_to_vertex() {
        // The matrix should transform vertices the same way the
        // mesh-core `apply_to_vertex` does; that is the contract the
        // shader relies on.
        let a = AffineTransform3D::from_translation([1.0, 2.0, 3.0]).compose(
            &AffineTransform3D::from_rotation_z(std::f64::consts::FRAC_PI_4)
                .compose(&AffineTransform3D::from_non_uniform_scale([2.0, 3.0, 4.0])),
        );
        let m = affine_to_mat4(&a);
        for v in [[1.0_f32, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
            let by_matrix = m.transform_point3(Vec3::from(v));
            let by_apply = a.apply_to_vertex(sc_mesh_core::primitives::Vertex::new(v));
            assert_approx_eq!(f32, by_matrix.x, by_apply[0], epsilon = 1e-5);
            assert_approx_eq!(f32, by_matrix.y, by_apply[1], epsilon = 1e-5);
            assert_approx_eq!(f32, by_matrix.z, by_apply[2], epsilon = 1e-5);
        }
    }

    /// Roomy envelope that contains every fixture in this module so
    /// tests focused on the selection / model-matrix lanes are not
    /// disturbed by the breach detection added to `build_model_entries`.
    fn permissive_volume() -> BuildVolume {
        BuildVolume::new(1000.0, 1000.0, 1000.0)
    }

    /// Loader-faithful empty graph: identity-Group root, no leaves yet.
    fn graph_with_group_root() -> (Scene, NodeId) {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group root");
        (g, root)
    }

    fn add_inline_leaf(
        g: &mut Scene,
        parent: NodeId,
        mesh: IndexedMesh,
        transform: Transform3D,
    ) -> NodeId {
        let leaf = g
            .add_leaf(None, transform, LeafPayload::Mesh(Arc::new(mesh)))
            .expect("add inline leaf");
        g.add_child(parent, leaf).expect("attach inline leaf");
        leaf
    }

    /// Resolve the `n`-th leaf's [`NodeId`] in pre-order traversal,
    /// used by the selection tests to feed `build_model_entries` a
    /// real id instead of a counter.
    fn nth_leaf(graph: &Scene, n: usize) -> NodeId {
        let mut ids: Vec<NodeId> = Vec::new();
        let _ = for_each_leaf(graph, |id, _, _| {
            ids.push(id);
            Ok(())
        });
        ids.into_iter()
            .nth(n)
            .expect("test fixture should expose enough leaves")
    }

    #[test]
    fn build_model_entries_one_per_leaf() {
        use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};

        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new([0.0, 0.0, 0.0])],
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([0.0, 0.0, 1.0]),
                [0, 0, 0],
            )],
        };
        let (mut g, root) = graph_with_group_root();
        add_inline_leaf(&mut g, root, mesh.clone(), Transform3D::Identity);
        add_inline_leaf(
            &mut g,
            root,
            mesh,
            Transform3D::Affine(AffineTransform3D::from_translation([7.0, 0.0, 0.0])),
        );
        let entries = build_model_entries(&g, None, permissive_volume()).expect("clean graph");
        assert_eq!(entries.len(), 2);
        // First entry is identity.
        assert_approx_eq!(f32, entries[0].model[0][0], 1.0);
        assert_approx_eq!(f32, entries[0].model[3][0], 0.0);
        // Second entry has +7 in the translation slot (column 3, row 0).
        assert_approx_eq!(f32, entries[1].model[3][0], 7.0);
        assert_approx_eq!(f32, entries[1].model[3][1], 0.0);
        assert_approx_eq!(f32, entries[1].model[3][2], 0.0);
    }

    fn two_object_graph() -> Scene {
        use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};

        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new([0.0, 0.0, 0.0])],
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([0.0, 0.0, 1.0]),
                [0, 0, 0],
            )],
        };
        let (mut g, root) = graph_with_group_root();
        add_inline_leaf(&mut g, root, mesh.clone(), Transform3D::Identity);
        add_inline_leaf(&mut g, root, mesh, Transform3D::Identity);
        g
    }

    #[test]
    fn no_selection_leaves_every_flags_x_zero() {
        let graph = two_object_graph();
        let entries = build_model_entries(&graph, None, permissive_volume()).expect("clean graph");
        assert_eq!(entries[0].flags[0], 0);
        assert_eq!(entries[1].flags[0], 0);
    }

    #[test]
    fn selected_node_id_sets_only_that_entrys_flags_x() {
        let graph = two_object_graph();
        let entries = build_model_entries(&graph, Some(nth_leaf(&graph, 1)), permissive_volume())
            .expect("clean graph");
        assert_eq!(entries[0].flags[0], 0);
        assert_eq!(entries[1].flags[0], 1);
    }

    #[test]
    fn non_leaf_selection_falls_back_to_no_highlight() {
        // The loader-faithful Group root that wraps the leaves is not
        // a leaf itself, so the iterator never visits it; flags.x
        // stays 0 on every entry when the selection points at the
        // root.
        let graph = two_object_graph();
        let entries = build_model_entries(&graph, Some(graph.roots()[0]), permissive_volume())
            .expect("clean graph");
        assert_eq!(entries[0].flags[0], 0);
        assert_eq!(entries[1].flags[0], 0);
    }

    fn unit_triangle_graph_at(translation: [f64; 3]) -> Scene {
        use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};

        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([0.0, 0.0, 1.0]),
                [0, 1, 2],
            )],
        };
        let (mut g, root) = graph_with_group_root();
        add_inline_leaf(
            &mut g,
            root,
            mesh,
            Transform3D::Affine(AffineTransform3D::from_translation(translation)),
        );
        g
    }

    /// 100 mm x 60 mm plate, 80 mm tall: envelope is X in [-50, 50],
    /// Y in [-30, 30], Z in [0, 80]. Tight enough that small fixtures
    /// can be placed strictly inside or strictly outside without
    /// arithmetic gymnastics.
    fn snug_volume() -> BuildVolume {
        BuildVolume::new(100.0, 60.0, 80.0)
    }

    #[test]
    fn flags_y_is_zero_for_fully_inside_leaf() {
        let graph = unit_triangle_graph_at([0.0, 0.0, 1.0]);
        let entries = build_model_entries(&graph, None, snug_volume()).expect("clean graph");
        assert_eq!(entries[0].flags[1], 0);
    }

    #[test]
    fn flags_y_is_one_when_any_vertex_is_past_a_side_wall() {
        // Translation +60 mm on X puts the unit triangle's vertices at
        // [60, 0, 0], [61, 0, 0], [60, 1, 0]. All three lie past the
        // +X wall at x = 50.
        let graph = unit_triangle_graph_at([60.0, 0.0, 0.0]);
        let entries = build_model_entries(&graph, None, snug_volume()).expect("clean graph");
        assert_eq!(entries[0].flags[1], 1);
    }

    #[test]
    fn flags_y_is_one_when_any_vertex_dips_below_the_floor() {
        // Translation in Z by -0.5 puts every vertex below z = 0.
        let graph = unit_triangle_graph_at([0.0, 0.0, -0.5]);
        let entries = build_model_entries(&graph, None, snug_volume()).expect("clean graph");
        assert_eq!(entries[0].flags[1], 1);
    }

    #[test]
    fn flags_y_is_one_when_leaf_straddles_a_wall() {
        // Translation +49.5 mm on X: vertex [0,0,0] lands at x = 49.5
        // (inside), [1,0,0] at x = 50.5 (outside +X wall). Per-vertex
        // breach detection must fire on the second vertex.
        let graph = unit_triangle_graph_at([49.5, 0.0, 0.0]);
        let entries = build_model_entries(&graph, None, snug_volume()).expect("clean graph");
        assert_eq!(entries[0].flags[1], 1);
    }

    #[test]
    fn flags_y_is_unset_for_leaf_resting_exactly_on_the_floor() {
        // Every vertex has z = 0 (the floor). The breach test uses a
        // strict comparison, so resting on the plate is "inside".
        let graph = unit_triangle_graph_at([0.0, 0.0, 0.0]);
        let entries = build_model_entries(&graph, None, snug_volume()).expect("clean graph");
        assert_eq!(entries[0].flags[1], 0);
    }

    #[test]
    fn warning_mask_is_zero_when_every_leaf_fits() {
        let graph = unit_triangle_graph_at([0.0, 0.0, 1.0]);
        assert_eq!(
            build_envelope_warning_mask(&graph, snug_volume()).expect("clean graph"),
            0
        );
    }

    #[test]
    fn warning_mask_sets_plus_x_when_a_vertex_passes_the_plus_x_wall() {
        // Vertex [1,0,0] translated to [60,0,0] sits past +X (50).
        let graph = unit_triangle_graph_at([60.0, 0.0, 0.0]);
        let mask = build_envelope_warning_mask(&graph, snug_volume()).expect("clean graph");
        assert_eq!(mask & FACE_BIT_X_POS, FACE_BIT_X_POS);
        assert_eq!(mask & FACE_BIT_X_NEG, 0);
    }

    #[test]
    fn warning_mask_sets_minus_y_when_a_vertex_passes_the_minus_y_wall() {
        // Vertex [0,1,0] translated by [0,-40,0] lands at [0,-39,0],
        // and [0,0,0] lands at [0,-40,0], past the -Y wall (-30).
        let graph = unit_triangle_graph_at([0.0, -40.0, 0.0]);
        let mask = build_envelope_warning_mask(&graph, snug_volume()).expect("clean graph");
        assert_eq!(mask & FACE_BIT_Y_NEG, FACE_BIT_Y_NEG);
        assert_eq!(mask & FACE_BIT_Y_POS, 0);
    }

    #[test]
    fn warning_mask_sets_plus_z_when_a_vertex_passes_the_ceiling() {
        // Vertices land at [0,0,90], past the +Z ceiling (80).
        let graph = unit_triangle_graph_at([0.0, 0.0, 90.0]);
        let mask = build_envelope_warning_mask(&graph, snug_volume()).expect("clean graph");
        assert_eq!(mask & FACE_BIT_Z_POS, FACE_BIT_Z_POS);
    }

    #[test]
    fn warning_mask_sets_minus_z_when_a_vertex_dips_below_the_floor() {
        // The Z = 0 floor breach is still represented in the mask
        // (bit FACE_BIT_Z_NEG); the plate shader consumes it through
        // the floor-overlay path rather than a dedicated wall quad.
        let graph = unit_triangle_graph_at([0.0, 0.0, -1.0]);
        let mask = build_envelope_warning_mask(&graph, snug_volume()).expect("clean graph");
        assert_eq!(mask & FACE_BIT_Z_NEG, FACE_BIT_Z_NEG);
        // None of the lateral bits fire for a purely-vertical breach.
        assert_eq!(
            mask & (FACE_BIT_X_NEG | FACE_BIT_X_POS | FACE_BIT_Y_NEG | FACE_BIT_Y_POS),
            0
        );
    }

    #[test]
    fn warning_mask_unions_bits_across_leaves() {
        // First leaf breaches +X, second breaches -Y. The mask
        // collects both bits.
        use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([0.0, 0.0, 1.0]),
                [0, 1, 2],
            )],
        };
        let (mut g, root) = graph_with_group_root();
        add_inline_leaf(
            &mut g,
            root,
            mesh.clone(),
            Transform3D::Affine(AffineTransform3D::from_translation([60.0, 0.0, 0.0])),
        );
        add_inline_leaf(
            &mut g,
            root,
            mesh,
            Transform3D::Affine(AffineTransform3D::from_translation([0.0, -40.0, 0.0])),
        );
        let mask = build_envelope_warning_mask(&g, snug_volume()).expect("clean graph");
        assert_eq!(mask & FACE_BIT_X_POS, FACE_BIT_X_POS);
        assert_eq!(mask & FACE_BIT_Y_NEG, FACE_BIT_Y_NEG);
    }
}

#[cfg(test)]
mod align_up_tests {
    use super::*;

    #[test]
    fn align_up_pads_to_next_alignment_boundary() {
        assert_eq!(align_up(80, 256), 256);
        assert_eq!(align_up(257, 256), 512);
    }

    #[test]
    fn align_up_is_identity_when_size_is_a_multiple() {
        assert_eq!(align_up(256, 256), 256);
        assert_eq!(align_up(512, 256), 512);
    }

    #[test]
    fn align_up_handles_alignment_one() {
        // alignment = 1 means no padding; size passes through unchanged.
        assert_eq!(align_up(1, 1), 1);
        assert_eq!(align_up(80, 1), 80);
    }
}

#[cfg(test)]
mod for_each_leaf_tests {
    use std::sync::Arc;

    use super::*;
    use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};

    fn unit_triangle() -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([0.0, 0.0, 1.0]),
                [0, 1, 2],
            )],
        }
    }

    fn two_leaf_graph() -> Scene {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("root");
        let a = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_triangle())),
            )
            .expect("first leaf");
        g.add_child(root, a).expect("attach first leaf");
        let b = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_triangle())),
            )
            .expect("second leaf");
        g.add_child(root, b).expect("attach second leaf");
        g
    }

    /// `for_each_leaf` must surface a closure-returned `Err` to its
    /// caller; callers that discard this with `let _ = ...` (the
    /// shape this regression test exists to lock out) silently lose
    /// graph-walk failures and leave downstream buffers in a stale
    /// state. The closure-error path is the only side of the
    /// contract reachable from a public-API-constructed `Scene`; the
    /// `descend_leaves` dangling-id branch is exercised by the
    /// type-system signature change on `build_model_entries` and
    /// `build_envelope_warning_mask` (their `?` cannot be reduced to
    /// `let _ =` without a compile error).
    #[test]
    fn for_each_leaf_propagates_closure_err() {
        let graph = two_leaf_graph();
        let result = for_each_leaf(&graph, |_, _, _| Err(anyhow!("simulated downstream error")));
        assert!(result.is_err());
    }

    /// Once the closure has returned `Err`, the walk must short-circuit:
    /// later leaves are not visited, so a partial state mutated by the
    /// closure before the first failure stays partial rather than
    /// continuing to grow against half-valid input.
    #[test]
    fn for_each_leaf_short_circuits_on_first_closure_err() {
        let graph = two_leaf_graph();
        let mut visited = 0usize;
        let result = for_each_leaf(&graph, |_, _, _| {
            visited += 1;
            Err(anyhow!("simulated"))
        });
        assert!(result.is_err());
        assert_eq!(visited, 1);
    }
}

/// The walk-error arm of `renderable_leaf_count` is not covered here:
/// `Scene::add_leaf` rejects an unbound `MeshResourceId` and
/// `Scene::remove_mesh_resource` rejects a still-referenced one, so a
/// graph with a dangling id cannot be built through the public API.
/// `for_each_leaf_tests` covers the propagation itself.
#[cfg(test)]
mod renderable_leaf_count_tests {
    use std::sync::Arc;

    use super::*;
    use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};

    fn unit_triangle() -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([0.0, 0.0, 1.0]),
                [0, 1, 2],
            )],
        }
    }

    /// Loader-faithful empty graph: identity-Group root, no leaves yet.
    fn graph_with_group_root() -> (Scene, NodeId) {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group root");
        (g, root)
    }

    fn add_inline_leaf(g: &mut Scene, parent: NodeId) -> NodeId {
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_triangle())),
            )
            .expect("add inline leaf");
        g.add_child(parent, leaf).expect("attach inline leaf");
        leaf
    }

    #[test]
    fn graph_without_roots_has_no_leaves() {
        assert_eq!(renderable_leaf_count(&Scene::new()).expect("clean walk"), 0);
    }

    #[test]
    fn group_root_without_children_has_no_leaves() {
        let (graph, _root) = graph_with_group_root();
        assert_eq!(renderable_leaf_count(&graph).expect("clean walk"), 0);
    }

    #[test]
    fn one_leaf_under_the_root_counts_once() {
        let (mut graph, root) = graph_with_group_root();
        add_inline_leaf(&mut graph, root);
        assert_eq!(renderable_leaf_count(&graph).expect("clean walk"), 1);
    }

    #[test]
    fn two_leaves_under_the_root_count_twice() {
        let (mut graph, root) = graph_with_group_root();
        add_inline_leaf(&mut graph, root);
        add_inline_leaf(&mut graph, root);
        assert_eq!(renderable_leaf_count(&graph).expect("clean walk"), 2);
    }

    #[test]
    fn leaves_under_a_nested_group_are_counted() {
        // The count must follow the whole subtree, not just the roots'
        // direct children: one leaf sits under the root, the other two
        // under a Group nested inside it.
        let (mut graph, root) = graph_with_group_root();
        add_inline_leaf(&mut graph, root);
        let nested = graph.add_group(None, Transform3D::Identity);
        graph.add_child(root, nested).expect("attach nested group");
        add_inline_leaf(&mut graph, nested);
        add_inline_leaf(&mut graph, nested);
        assert_eq!(renderable_leaf_count(&graph).expect("clean walk"), 3);
    }

    #[test]
    fn root_leaf_counts_without_a_group_above_it() {
        // The STL-loaded shape: the leaf is a scene root itself rather
        // than a child of an identity Group.
        let mut graph = Scene::new();
        let leaf = graph
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_triangle())),
            )
            .expect("add inline leaf");
        graph.add_root(leaf).expect("leaf root");
        assert_eq!(renderable_leaf_count(&graph).expect("clean walk"), 1);
    }

    #[test]
    fn removing_the_only_leaf_drops_the_count_to_zero() {
        // The shape the scene lands in when the user deletes the last
        // object: a live graph whose leaf population is empty.
        let (mut graph, root) = graph_with_group_root();
        let leaf = add_inline_leaf(&mut graph, root);
        graph.remove_leaf(leaf).expect("remove the only leaf");
        assert_eq!(renderable_leaf_count(&graph).expect("clean walk"), 0);
    }

    #[test]
    fn removing_the_only_root_leaf_drops_the_count_to_zero() {
        // Same, for the STL shape: removing a root leaf leaves a graph
        // with no roots at all.
        let mut graph = Scene::new();
        let leaf = graph
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_triangle())),
            )
            .expect("add inline leaf");
        graph.add_root(leaf).expect("leaf root");
        graph.remove_leaf(leaf).expect("remove the only root leaf");
        assert_eq!(renderable_leaf_count(&graph).expect("clean walk"), 0);
    }
}

#[cfg(test)]
mod leaf_buffer_limit_tests {
    use std::sync::Arc;

    use super::*;
    use sc_mesh_core::MeshMetadata;
    use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
    use sc_mesh_core::scene::node::SceneMetadata;

    /// Small enough that a mesh of a few dozen faces exceeds it.
    const LIMIT: u64 = 1024;

    /// A mesh of `vertex_count` vertices and `face_count` faces on the
    /// first 3 vertices: `12 * vertex_count` vertex bytes, `12 *
    /// face_count` index bytes.
    fn mesh(vertex_count: usize, face_count: usize, name: Option<&str>) -> IndexedMesh {
        IndexedMesh {
            meta: name.map(|n| MeshMetadata {
                name: Some(n.to_string()),
                header_comment: None,
            }),
            vertices: (0..vertex_count)
                .map(|i| Vertex::new([i as f32, 0.0, 0.0]))
                .collect(),
            faces: (0..face_count)
                .map(|_| IndexedTriangle::from_raw_indices(Normal::new([0.0, 0.0, 1.0]), [0, 1, 2]))
                .collect(),
        }
    }

    /// One root leaf per mesh, in order; returns the graph and the leaf ids.
    fn graph_of(leaves: Vec<(IndexedMesh, Option<&str>)>) -> (Scene, Vec<NodeId>) {
        let mut g = Scene::new();
        let mut ids = Vec::new();
        for (m, node_name) in leaves {
            let meta = node_name.map(|n| SceneMetadata {
                name: Some(n.to_string()),
                comment: None,
            });
            let leaf = g
                .add_leaf(meta, Transform3D::Identity, LeafPayload::Mesh(Arc::new(m)))
                .expect("add leaf");
            g.add_root(leaf).expect("leaf root");
            ids.push(leaf);
        }
        (g, ids)
    }

    fn check(graph: &Scene) -> std::result::Result<(), LeafBufferExceedsDeviceLimit> {
        let data = build_gpu_scene_data(graph, |_| {}).expect("flatten");
        check_leaf_buffers_fit(&data.objects, LIMIT)
    }

    #[test]
    fn no_objects_fit_trivially() {
        assert_eq!(check_leaf_buffers_fit(&[], LIMIT), Ok(()));
    }

    #[test]
    fn a_leaf_at_the_limit_fits() {
        // 85 faces: 1020 index bytes.
        let (graph, _) = graph_of(vec![(mesh(3, 85, None), None)]);
        assert_eq!(check(&graph), Ok(()));
    }

    #[test]
    fn a_leaf_one_face_over_the_limit_is_refused_by_its_index_buffer() {
        let (graph, ids) = graph_of(vec![(mesh(3, 86, None), None)]);
        assert_eq!(
            check(&graph),
            Err(LeafBufferExceedsDeviceLimit {
                node_id: ids[0],
                leaf_name: None,
                buffer_kind: LeafBufferKind::Index,
                num_elements: 86,
                num_bytes: 1032,
                max_buffer_size: LIMIT,
            })
        );
    }

    #[test]
    fn a_leaf_with_too_many_vertices_is_refused_by_its_vertex_buffer() {
        let (graph, ids) = graph_of(vec![(mesh(86, 1, None), None)]);
        assert_eq!(
            check(&graph),
            Err(LeafBufferExceedsDeviceLimit {
                node_id: ids[0],
                leaf_name: None,
                buffer_kind: LeafBufferKind::Vertex,
                num_elements: 86,
                num_bytes: 1032,
                max_buffer_size: LIMIT,
            })
        );
    }

    #[test]
    fn the_vertex_buffer_is_reported_when_both_buffers_are_over() {
        let (graph, _) = graph_of(vec![(mesh(86, 86, None), None)]);
        assert_eq!(
            check(&graph).unwrap_err().buffer_kind,
            LeafBufferKind::Vertex
        );
    }

    #[test]
    fn the_first_offending_leaf_in_pre_order_is_reported() {
        let (graph, ids) = graph_of(vec![
            (mesh(3, 10, None), None),
            (mesh(3, 86, None), None),
            (mesh(3, 90, None), None),
        ]);
        assert_eq!(check(&graph).unwrap_err().node_id, ids[1]);
    }

    #[test]
    fn a_refusal_names_the_node_when_it_has_a_name() {
        let (graph, _) = graph_of(vec![(mesh(3, 86, Some("mesh-name")), Some("hull"))]);
        let err = check(&graph).unwrap_err();
        assert_eq!(err.leaf_name.as_deref(), Some("hull"));
        assert_eq!(
            err.to_string(),
            "Object 'hull' has more triangles (86) than this graphics device can hold in one buffer"
        );
    }

    #[test]
    fn a_refusal_falls_back_to_the_mesh_name() {
        let (graph, _) = graph_of(vec![(mesh(3, 86, Some("part")), None)]);
        assert_eq!(
            check(&graph).unwrap_err().leaf_name.as_deref(),
            Some("part")
        );
    }

    #[test]
    fn an_unnamed_leaf_is_reported_as_an_object() {
        let (graph, _) = graph_of(vec![(mesh(3, 86, None), None)]);
        assert_eq!(
            check(&graph).unwrap_err().to_string(),
            "An object has more triangles (86) than this graphics device can hold in one buffer"
        );
    }

    #[test]
    fn a_vertex_refusal_speaks_of_vertices() {
        let (graph, _) = graph_of(vec![(mesh(86, 1, None), Some("shell"))]);
        assert_eq!(
            check(&graph).unwrap_err().to_string(),
            "Object 'shell' has more vertices (86) than this graphics device can hold in one buffer"
        );
    }
}
