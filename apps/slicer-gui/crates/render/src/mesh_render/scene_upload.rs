/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Weak};

use anyhow::Result;
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3, Vec4};
use sc_mesh_core::Aabb;
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::primitives::{CoordAxis, Vertex};
use sc_mesh_core::transform::AffineTransform3D;
use sc_mesh_core::transform::Transform3D;
use sc_print_primitives::{Side, Wall, Walls};
use sc_scene::{CsgPolicy, NodeId, Scene, SceneError};
use slint::wgpu_30::wgpu;

use super::device_loss::DeviceLoss;
use super::device_ops::create_buffer_reporting;
use super::hull_points::HullPointsCache;
use crate::build_volume::BuildVolume;
use crate::i18n::ctr;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
pub(super) struct GpuVertex {
    position: [f32; 3],
    /// The shading normal (smooth with a crease angle, see `smooth_shading`),
    /// in the leaf's local frame; zero shades flat from screen derivatives.
    normal: [f32; 3],
}

/// Per-object entry in the model uniform buffer. Mirrors the WGSL `ModelEntry`
/// struct field by field. `flags.x` is the selection bit (1 for the selected
/// object, 0 otherwise), `flags.y` the envelope-breach bit and `flags.z` what
/// the object is drawn as (an overlay tint, or [`MODEL_OVERHANG_MAP`]); the
/// last lane is unused.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
pub(super) struct ModelEntry {
    model: [[f32; 4]; 4],
    flags: [u32; 4],
}

pub(super) const MODEL_ENTRY_SIZE: u64 = size_of::<ModelEntry>() as u64;

/// `flags.z` of a model the overhang map paints. Not a new lane: `fs_main`
/// never read the tint a scene object carried, so the map adds nothing to what
/// every vertex hands the rasteriser.
const MODEL_OVERHANG_MAP: u32 = 6;

impl ModelEntry {
    /// A preview overlay piece: world coordinates already, so the identity
    /// transform, and its tint in `flags.z`.
    pub(super) fn overlay(tint: u32) -> Self {
        ModelEntry {
            model: Mat4::IDENTITY.to_cols_array_2d(),
            flags: [0, 0, tint, 0],
        }
    }
}

impl GpuVertex {
    pub(super) fn at(position: [f32; 3]) -> Self {
        GpuVertex {
            position,
            normal: [0.0; 3],
        }
    }

    pub(super) fn with_normal(position: [f32; 3], normal: [f32; 3]) -> Self {
        GpuVertex { position, normal }
    }

    #[cfg(test)]
    pub(super) fn normal(&self) -> [f32; 3] {
        self.normal
    }
}

/// Rounds `size` up to the next multiple of `alignment`, which must be a power
/// of 2 for the mask form. Used to space successive `ModelEntry` slots in the
/// per-scene uniform buffer at the device's
/// `min_uniform_buffer_offset_alignment`.
pub(super) const fn align_up(size: u64, alignment: u64) -> u64 {
    (size + alignment - 1) & !(alignment - 1)
}

/// GPU-resident draw data for a single graph leaf: its local-space vertex+index
/// buffers (the leaf's transform is *not* baked in; it arrives at draw time
/// through the model uniform buffer). The `node_id` lets the draw loop key
/// per-leaf state (selection stencil) against the graph's stable id.
pub(super) struct ObjectDraw {
    node_id: NodeId,
    buffers: Arc<MeshBuffers>,
}

/// The local-space vertex and index buffers of one mesh. Every leaf that draws
/// the same mesh the same way holds the same buffers, so a copy of an object
/// costs no upload of its own (#353).
pub(super) struct MeshBuffers {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
}

impl ObjectDraw {
    pub(super) fn node_id(&self) -> NodeId {
        self.node_id
    }

    pub(super) fn vertex_buffer(&self) -> &wgpu::Buffer {
        &self.buffers.vertex_buffer
    }

    pub(super) fn index_buffer(&self) -> &wgpu::Buffer {
        &self.buffers.index_buffer
    }

    pub(super) fn index_count(&self) -> u32 {
        self.buffers.index_count
    }

    /// Whether this object draws from the very buffers `other` draws from.
    #[cfg(test)]
    pub(super) fn shares_buffers_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.buffers, &other.buffers)
    }

    /// The buffers this object draws from.
    #[cfg(test)]
    pub(super) fn buffers_for_test(&self) -> Arc<MeshBuffers> {
        Arc::clone(&self.buffers)
    }
}

/// What the buffers of a mesh depend on: the mesh, by address, and whether it
/// is drawn smooth, which a preference can change between two uploads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct MeshKey {
    mesh: usize,
    smooth: bool,
}

impl MeshKey {
    fn of(mesh: &Arc<IndexedMesh>) -> Self {
        Self {
            mesh: Arc::as_ptr(mesh) as usize,
            smooth: super::smooth_shading::shades_smooth(mesh),
        }
    }
}

/// The meshes a [`GpuScene`] uploaded, with their buffers and local boxes, so
/// that the next upload reuses what it still draws instead of flattening and
/// uploading it again: a paste, a deletion or an undo then uploads only the
/// meshes that are new to the scene (#353).
///
/// An entry holds a [`Weak`] to its mesh. While it lives the allocation cannot
/// be reused by another mesh, and the mesh cannot change in place:
/// `Arc::get_mut` refuses and `Arc::make_mut` moves the mesh to a new address.
/// So a live entry always describes the vertices the mesh has now, as in
/// [`HullPointsCache`].
#[derive(Default)]
pub(super) struct UploadedMeshes {
    entries: HashMap<MeshKey, UploadedMesh>,
}

struct UploadedMesh {
    mesh: Weak<IndexedMesh>,
    buffers: Arc<MeshBuffers>,
    local_aabb: Option<Aabb>,
}

impl UploadedMeshes {
    /// The entry of `key`, when its mesh still lives.
    fn get(&self, key: MeshKey) -> Option<&UploadedMesh> {
        self.entries
            .get(&key)
            .filter(|entry| entry.mesh.strong_count() > 0)
    }

    /// Whether it holds the buffers of the live mesh at address `mesh`.
    fn holds(&self, mesh: usize) -> bool {
        [false, true]
            .into_iter()
            .any(|smooth| self.get(MeshKey { mesh, smooth }).is_some())
    }

    /// Takes in the entries of `other`, built on the same device.
    pub(super) fn absorb(&mut self, other: UploadedMeshes) {
        self.entries.extend(other.entries);
    }

    /// Keeps only the entries of meshes `graph` draws, the way it draws them.
    pub(super) fn retain_drawn(&mut self, graph: &Scene) -> Result<()> {
        let mut drawn: HashSet<MeshKey> = HashSet::new();
        graph.for_each_expanded_leaf::<SceneError, _>(CsgPolicy::Descend, |node_id, _, mesh| {
            if let Some(shared) = shared_mesh(graph, node_id, mesh) {
                drawn.insert(MeshKey::of(shared));
            }
            Ok(())
        })?;
        self.entries
            .retain(|key, entry| drawn.contains(key) && entry.mesh.strong_count() > 0);
        Ok(())
    }

    /// How many meshes it holds the buffers of.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.entries.len()
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
        let count = self.num_elements;
        let text = match (self.buffer_kind, &self.leaf_name) {
            (LeafBufferKind::Vertex, Some(name)) => ctr!(
                "ErrorDialog",
                "Object '{}' has more vertices ({}) than this graphics device can hold in one buffer",
                name,
                count
            ),
            (LeafBufferKind::Vertex, None) => ctr!(
                "ErrorDialog",
                "An object has more vertices ({}) than this graphics device can hold in one buffer",
                count
            ),
            (LeafBufferKind::Index, Some(name)) => ctr!(
                "ErrorDialog",
                "Object '{}' has more triangles ({}) than this graphics device can hold in one buffer",
                name,
                count
            ),
            (LeafBufferKind::Index, None) => ctr!(
                "ErrorDialog",
                "An object has more triangles ({}) than this graphics device can hold in one buffer",
                count
            ),
        };
        f.write_str(&text)
    }
}

impl std::error::Error for LeafBufferExceedsDeviceLimit {}

/// A buffer of the scene upload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UploadBuffer {
    /// The vertex or index buffer of an object, with the object's name when it
    /// has one.
    Object {
        kind: LeafBufferKind,
        name: Option<String>,
    },
    /// The per-object transform table.
    TransformTable,
}

#[derive(Debug)]
pub struct DeviceRefusedBuffer {
    pub what: UploadBuffer,
    pub num_bytes: u64,
    pub source: wgpu::Error,
}

impl fmt::Display for DeviceRefusedBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = self.num_bytes;
        let text = match &self.what {
            UploadBuffer::Object {
                kind: LeafBufferKind::Vertex,
                name: Some(name),
            } => ctr!(
                "ErrorDialog",
                "The graphics device refused the vertex buffer of object '{}' ({} bytes)",
                name,
                bytes
            ),
            UploadBuffer::Object {
                kind: LeafBufferKind::Vertex,
                name: None,
            } => ctr!(
                "ErrorDialog",
                "The graphics device refused the vertex buffer of an object ({} bytes)",
                bytes
            ),
            UploadBuffer::Object {
                kind: LeafBufferKind::Index,
                name: Some(name),
            } => ctr!(
                "ErrorDialog",
                "The graphics device refused the index buffer of object '{}' ({} bytes)",
                name,
                bytes
            ),
            UploadBuffer::Object {
                kind: LeafBufferKind::Index,
                name: None,
            } => ctr!(
                "ErrorDialog",
                "The graphics device refused the index buffer of an object ({} bytes)",
                bytes
            ),
            UploadBuffer::TransformTable => ctr!(
                "ErrorDialog",
                "The graphics device refused the per-object transform table ({} bytes)",
                bytes
            ),
        };
        f.write_str(&text)
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
        f.write_str(&ctr!(
            "ErrorDialog",
            "The graphics device was lost while uploading the scene"
        ))
    }
}

impl std::error::Error for DeviceLostDuringUpload {}

#[derive(Debug)]
pub struct NoLeavesToRender;

impl fmt::Display for NoLeavesToRender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&ctr!("ErrorDialog", "Graph has no leaves to render"))
    }
}

impl std::error::Error for NoLeavesToRender {}

/// Creates a buffer holding `contents`, reporting a refusal instead of
/// panicking. Empty contents give an empty, unmapped buffer. If the device is
/// lost before or during the call, the error is [`DeviceLostDuringUpload`].
fn upload_buffer(
    device: &wgpu::Device,
    device_loss: &DeviceLoss,
    label: &'static str,
    contents: &[u8],
    usage: wgpu::BufferUsages,
    what: impl FnOnce() -> UploadBuffer,
) -> Result<wgpu::Buffer> {
    let unpadded = contents.len() as u64;
    // wgpu requires mapped-at-creation sizes to be a multiple of
    // COPY_BUFFER_ALIGNMENT above 0.
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
        // The buffer is mapped at creation over its whole aligned size, so a
        // map error can only come from a lost device.
        buffer
            .get_mapped_range_mut(..)
            .map_err(|_| DeviceLostDuringUpload)?
            .slice(..contents.len())
            .copy_from_slice(contents);
        buffer.unmap();
    }
    Ok(buffer)
}

/// GPU mirror of a `Scene`. Holds 1 `ObjectDraw` per leaf in pre-order
/// traversal of the graph's roots, plus a uniform buffer of `ModelEntry` slots,
/// padded to the device's `min_uniform_buffer_offset_alignment` so the render
/// loop can address each slot via a dynamic-offset bind. The world-space AABB
/// is captured at build time from `Scene::aabb` and is not refreshed on
/// per-frame transform updates. The renderer reads it every frame for the near
/// and far planes, and camera framing reads it through `scene_bounds`.
///
/// Construction ([`GpuScene::build`]) touches a `wgpu::Device` but not the UI
/// loop, so it can run on the worker thread that parsed the scene; the UI
/// thread then attaches the matching bind group via
/// [`Renderer::install_scene`](super::Renderer::install_scene).
pub struct GpuScene {
    objects: Vec<ObjectDraw>,
    /// The buffers of each mesh in `objects`, for the next upload to reuse.
    meshes: UploadedMeshes,
    model_buffer: wgpu::Buffer,
    /// Distance in bytes between consecutive `ModelEntry` slots in
    /// `model_buffer`. Always at least `MODEL_ENTRY_SIZE` and a multiple of the
    /// device's `min_uniform_buffer_offset_alignment`.
    model_stride: u64,
    /// Identity token of the device this scene's GPU resources were built
    /// against. `Renderer::install_scene` checks this against the renderer's
    /// own token before attaching the scene.
    device_token: super::DeviceToken,
    aabb_min: Vec3,
    aabb_max: Vec3,
}

impl fmt::Debug for GpuScene {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GpuScene")
            .field("objects", &self.objects.len())
            .field("model_stride", &self.model_stride)
            .field("aabb_min", &self.aabb_min)
            .field("aabb_max", &self.aabb_max)
            .finish_non_exhaustive()
    }
}

/// Triangle counts reported while a scene's GPU data is built. Each object's
/// triangles count twice, once when its vertex and index vectors are flattened
/// and once when its device buffers are created, so `total_triangles` is twice
/// the scene's reachable triangle count and the final tick reaches it.
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

    /// Builds the scene as [`GpuScene::build`] does, reporting per-object
    /// triangle progress; see [`UploadProgress`] for the tick contract.
    pub fn build_with_progress(
        device: &wgpu::Device,
        device_token: super::DeviceToken,
        graph: &Scene,
        device_loss: &DeviceLoss,
        progress: impl FnMut(UploadProgress),
    ) -> Result<Self> {
        Self::build_reusing(
            device,
            device_token,
            graph,
            device_loss,
            UploadedMeshes::default(),
            progress,
        )
    }

    /// Builds the scene as [`GpuScene::build_with_progress`] does, taking the
    /// buffers of every mesh `uploaded` holds from there instead of uploading
    /// them again. `uploaded` must come from a scene built on `device`.
    pub(super) fn build_reusing(
        device: &wgpu::Device,
        device_token: super::DeviceToken,
        graph: &Scene,
        device_loss: &DeviceLoss,
        uploaded: UploadedMeshes,
        mut progress: impl FnMut(UploadProgress),
    ) -> Result<Self> {
        let mut scene_triangles: u64 = 0;
        graph.for_each_expanded_leaf::<SceneError, _>(
            CsgPolicy::Descend,
            |_node_id, _world_transform, mesh| {
                scene_triangles += mesh.faces().len() as u64;
                Ok(())
            },
        )?;
        let total_triangles = scene_triangles.saturating_mul(2);
        let mut processed_triangles: u64 = 0;
        let mut tick = |processed: &mut u64, triangles: u64| {
            *processed += triangles;
            progress(UploadProgress {
                processed_triangles: *processed,
                total_triangles,
            });
        };

        let data = build_gpu_scene_data(graph, &uploaded, |triangles| {
            tick(&mut processed_triangles, triangles);
        })?;
        check_leaf_buffers_fit(&data.objects, device.limits().max_buffer_size)?;
        let mut objects: Vec<ObjectDraw> = Vec::with_capacity(data.objects.len());
        for obj in &data.objects {
            let buffers = match &obj.source {
                ObjectSource::Flattened { vertices, indices } => Arc::new(MeshBuffers {
                    vertex_buffer: upload_buffer(
                        device,
                        device_loss,
                        "mesh-viewer object vertex buffer",
                        bytemuck::cast_slice(vertices),
                        wgpu::BufferUsages::VERTEX,
                        || UploadBuffer::Object {
                            kind: LeafBufferKind::Vertex,
                            name: obj.name.clone(),
                        },
                    )?,
                    index_buffer: upload_buffer(
                        device,
                        device_loss,
                        "mesh-viewer object index buffer",
                        bytemuck::cast_slice(indices),
                        wgpu::BufferUsages::INDEX,
                        || UploadBuffer::Object {
                            kind: LeafBufferKind::Index,
                            name: obj.name.clone(),
                        },
                    )?,
                    index_count: indices.len() as u32,
                }),
                ObjectSource::Uploaded(buffers) => Arc::clone(buffers),
                ObjectSource::SameAs(first) => Arc::clone(&objects[*first].buffers),
            };
            objects.push(ObjectDraw {
                node_id: obj.node_id,
                buffers,
            });
            tick(&mut processed_triangles, obj.triangles);
        }
        let meshes = UploadedMeshes {
            entries: data
                .meshes
                .into_iter()
                .map(|record| {
                    let entry = UploadedMesh {
                        mesh: record.mesh,
                        buffers: Arc::clone(&objects[record.object].buffers),
                        local_aabb: record.local_aabb,
                    };
                    (record.key, entry)
                })
                .collect(),
        };

        let alignment = device.limits().min_uniform_buffer_offset_alignment as u64;
        let model_stride = align_up(MODEL_ENTRY_SIZE, alignment);

        // The bind group requires a binding size above 0;
        // `build_gpu_scene_data` already fails above when the scene has no
        // objects.
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
            what: UploadBuffer::TransformTable,
            num_bytes: model_size,
            source,
        })?;

        Ok(Self {
            objects,
            meshes,
            model_buffer,
            model_stride,
            device_token,
            aabb_min: data.aabb_min,
            aabb_max: data.aabb_max,
        })
    }

    /// Returns the identity token of the device this scene was built against.
    /// `Renderer::install_scene` matches it against the renderer's own token
    /// before attaching.
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

    /// The meshes it uploaded.
    pub(super) fn uploaded(&self) -> &UploadedMeshes {
        &self.meshes
    }

    /// Drops the scene but for the buffers of its meshes, for the next upload
    /// to reuse.
    pub(super) fn into_uploaded(self) -> UploadedMeshes {
        self.meshes
    }
}

/// Translates `AffineTransform3D`'s row-major 3x3 + translation form into
/// glam's column-major 4x4. f64 internals are downcast to f32 here at the GPU
/// boundary.
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

/// Returns the model entry of each reachable leaf, in draw order. `selection`
/// holds the [`NodeId`]s of the selected leaves, sorted; empty for no
/// selection. A `NodeId` that does not name a reachable leaf marks no entry.
/// The shader reads `flags.x` and mixes a warm tint when it is 1;
/// `flags.y` is set when the leaf has any vertex outside the build envelope,
/// driving the mild interior wash that marks the whole object as "offending"
/// even where individual fragments are still inside the envelope. With
/// `overhang_map`, `flags.z` marks every model for it; supports and rafts stay
/// out, being the answer to an overhang rather than one. `flags.w` is 1 for a
/// support mesh: from under the plate, what of it lies below the rafts' top
/// fades with the plate (#481).
pub(super) fn build_model_entries(
    graph: &Scene,
    selection: &[NodeId],
    build_volume: BuildVolume,
    boxes: &mut WorldBoxes,
    overhang_map: bool,
) -> Result<Vec<ModelEntry>> {
    let mut entries: Vec<ModelEntry> = Vec::new();
    graph.for_each_expanded_leaf::<SceneError, _>(
        CsgPolicy::Descend,
        |node_id, world_transform, mesh| {
            let selected = selection.binary_search(&node_id).is_ok();
            let transform_mat = transform_to_mat4(world_transform);
            // The envelope is an axis-aligned box, so it contains the world
            // box if and only if it contains every vertex, and therefore every
            // triangle.
            let breaches = !build_volume.volume().contains(&boxes.world_box(
                entries.len(),
                mesh,
                shared_mesh(graph, node_id, mesh),
                &transform_mat,
            ));
            let supports = sc_geom::df_supports::is_support_mesh(mesh);
            let drawn_as = if overhang_map && !supports {
                MODEL_OVERHANG_MAP
            } else {
                0
            };
            entries.push(ModelEntry {
                model: transform_mat.to_cols_array_2d(),
                flags: [
                    u32::from(selected),
                    u32::from(breaches),
                    drawn_as,
                    u32::from(supports),
                ],
            });
            Ok(())
        },
    )?;
    Ok(entries)
}

/// The world box of each leaf occurrence, remembered between frames so that a
/// frame walks the vertices of a leaf only when its mesh or its matrix changed.
/// Walking every vertex of every leaf twice per frame cost 5 ms for a single
/// 1.9 M-triangle model.
///
/// When the matrix changes (a drag), the box comes from the mesh's hull points
/// once the worker has built them: the same box from 6779 of that model's
/// 949406 vertices, 0.02 ms per frame instead of 2.5 ms.
///
/// Slot `i` holds the `i`-th leaf occurrence of
/// [`Scene::for_each_expanded_leaf`], and a slot is reused only when the mesh
/// at the same address, with the same vertex buffer, is drawn with the
/// bit-identical matrix, so a hit returns exactly the box a fresh walk would.
/// A mesh's vertices can only change in place through `&mut`, which makes the
/// scene owe a GPU upload; the renderer empties the cache on every scene
/// install, so a slot never outlives the geometry it was computed from.
#[derive(Debug, Default)]
pub(super) struct WorldBoxes {
    slots: Vec<Option<WorldBoxSlot>>,
    /// Kept across scene installs: an entry only ever describes a live mesh.
    hulls: HullPointsCache,
}

#[derive(Debug)]
struct WorldBoxSlot {
    key: WorldBoxKey,
    aabb: Aabb,
}

/// What a leaf's world box depends on: its mesh (by address, vertex buffer
/// address and vertex count) and the bits of the f32 matrix it is drawn with.
#[derive(Debug, PartialEq, Eq)]
struct WorldBoxKey {
    mesh: usize,
    vertices: usize,
    vertex_count: usize,
    matrix: [u32; 16],
}

impl WorldBoxKey {
    fn new(mesh: &IndexedMesh, transform_mat: &Mat4) -> Self {
        Self {
            mesh: std::ptr::from_ref(mesh) as usize,
            vertices: mesh.vertices().as_ptr() as usize,
            vertex_count: mesh.vertices().len(),
            matrix: transform_mat.to_cols_array().map(f32::to_bits),
        }
    }
}

impl WorldBoxes {
    /// How many leaf occurrences have a box remembered.
    #[cfg(test)]
    pub(super) fn remembered(&self) -> usize {
        self.slots.iter().flatten().count()
    }

    /// Forgets every box, and the hull points of meshes that are gone. Called
    /// whenever the renderer drops a scene.
    pub(super) fn clear(&mut self) {
        self.slots.clear();
        self.hulls.prune();
    }

    /// Forgets the boxes of meshes `meshes` does not hold, and the hull
    /// points of meshes that are gone. Called whenever the renderer installs
    /// a scene, with that scene's meshes: a box of a mesh the scene before
    /// drew, and this one still draws, stays valid, because the scene before
    /// held a [`Weak`] to it all along and so the mesh could not change. A
    /// paste then walks the vertices of the new copy alone, not of every
    /// object again (#353).
    pub(super) fn keep_for(&mut self, meshes: &UploadedMeshes) {
        for slot in &mut self.slots {
            if slot
                .as_ref()
                .is_some_and(|slot| !meshes.holds(slot.key.mesh))
            {
                *slot = None;
            }
        }
        self.hulls.prune();
    }

    /// Returns the box of `mesh`'s vertices after `transform_mat` for the
    /// `index`-th leaf occurrence. On a miss it comes from the hull points of
    /// `shared` (the `Arc` holding `mesh`) when they are ready and vouch for
    /// the matrix, and from a walk over every vertex otherwise.
    fn world_box(
        &mut self,
        index: usize,
        mesh: &IndexedMesh,
        shared: Option<&Arc<IndexedMesh>>,
        transform_mat: &Mat4,
    ) -> Aabb {
        if self.slots.len() <= index {
            self.slots.resize_with(index + 1, || None);
        }
        let key = WorldBoxKey::new(mesh, transform_mat);
        let slot = &mut self.slots[index];
        if let Some(hit) = slot.as_ref().filter(|s| s.key == key) {
            return hit.aabb;
        }
        let aabb = shared
            .and_then(|shared| self.hulls.get(shared))
            .and_then(|hull| hull.world_box(transform_mat))
            .unwrap_or_else(|| leaf_world_box(mesh, transform_mat));
        *slot = Some(WorldBoxSlot { key, aabb });
        aabb
    }
}

/// Returns the `Arc` that holds the mesh of leaf `node_id`, the one
/// [`Scene::for_each_expanded_leaf`] passed as `mesh`.
fn shared_mesh<'a>(
    graph: &'a Scene,
    node_id: NodeId,
    mesh: &IndexedMesh,
) -> Option<&'a Arc<IndexedMesh>> {
    graph
        .leaf_mesh_arc(node_id)
        .filter(|shared| std::ptr::eq(shared.as_ref(), mesh))
}

/// Returns the box of `mesh`'s vertices after `transform_mat`, computed in f32
/// with the matrix that the GPU draws them with.
fn leaf_world_box(mesh: &IndexedMesh, transform_mat: &Mat4) -> Aabb {
    let mut aabb = Aabb::empty();
    for v in mesh.vertices() {
        let p = transform_mat.transform_point3(Vec3::from(v.coords()));
        aabb.extend(Vertex::new([p.x, p.y, p.z]));
    }
    aabb
}

// Single-bit masks of the per-face breach mask that the plate shader reads.
// Bits 0 to 4 drive the side-wall and top warning quads; bit 5 (-Z, the
// build-plate floor) is read by the floor path of the same shader, which
// overlays a stripe pattern on the grid instead of drawing a wall quad.
pub(super) const FACE_BIT_X_NEG: u32 = 1 << 0;
pub(super) const FACE_BIT_X_POS: u32 = 1 << 1;
pub(super) const FACE_BIT_Y_NEG: u32 = 1 << 2;
pub(super) const FACE_BIT_Y_POS: u32 = 1 << 3;
pub(super) const FACE_BIT_Z_POS: u32 = 1 << 4;
pub(super) const FACE_BIT_Z_NEG: u32 = 1 << 5;

/// Returns the bitmask of the envelope walls that some reachable leaf crosses,
/// in the shader's bit layout (the `FACE_BIT_*` constants). A leaf crosses a
/// wall when the box of its world-transformed vertices reaches strictly past
/// it, which is when some vertex does.
pub(super) fn build_envelope_warning_mask(
    graph: &Scene,
    build_volume: BuildVolume,
    boxes: &mut WorldBoxes,
) -> Result<u32> {
    let volume = build_volume.volume();
    let mut crossed = Walls::default();
    let mut index = 0;
    graph.for_each_expanded_leaf::<SceneError, _>(
        CsgPolicy::Descend,
        |node_id, world_transform, mesh| {
            let world_box = boxes.world_box(
                index,
                mesh,
                shared_mesh(graph, node_id, mesh),
                &transform_to_mat4(world_transform),
            );
            crossed |= volume.walls_crossed_by(&world_box);
            index += 1;
            Ok(())
        },
    )?;
    Ok(crossed.iter().fold(0, |mask, wall| mask | face_bit(wall)))
}

/// Returns the bit of `wall` in the mask that the plate shader reads.
fn face_bit(wall: Wall) -> u32 {
    match (wall.axis, wall.side) {
        (CoordAxis::X, Side::Min) => FACE_BIT_X_NEG,
        (CoordAxis::X, Side::Max) => FACE_BIT_X_POS,
        (CoordAxis::Y, Side::Min) => FACE_BIT_Y_NEG,
        (CoordAxis::Y, Side::Max) => FACE_BIT_Y_POS,
        (CoordAxis::Z, Side::Min) => FACE_BIT_Z_NEG,
        (CoordAxis::Z, Side::Max) => FACE_BIT_Z_POS,
    }
}

/// Counts the leaf occurrences [`Scene::for_each_expanded_leaf`] visits under
/// [`CsgPolicy::Descend`]: the population [`build_gpu_scene_data`] turns into
/// draws, so an answer of 0 means there is nothing for the GPU to mirror. A
/// dangling id surfaces as an error, not as a count of 0.
pub(super) fn renderable_leaf_count(graph: &Scene) -> Result<usize> {
    let mut count = 0usize;
    graph.for_each_expanded_leaf::<SceneError, _>(
        CsgPolicy::Descend,
        |_node_id, _world_transform, _mesh| {
            count += 1;
            Ok(())
        },
    )?;
    Ok(count)
}

/// The draw data of 1 object, before its buffers are created.
struct ObjectData {
    node_id: NodeId,
    name: Option<String>,
    /// The triangles of its mesh, for the progress ticks.
    triangles: u64,
    source: ObjectSource,
}

/// Where the buffers of an object come from.
enum ObjectSource {
    /// Its mesh, flattened here for an upload of its own.
    Flattened {
        vertices: Vec<GpuVertex>,
        indices: Vec<u32>,
    },
    /// The buffers the scene before uploaded for the same mesh.
    Uploaded(Arc<MeshBuffers>),
    /// The buffers of the earlier object at this index, which draws the same
    /// mesh.
    SameAs(usize),
}

impl ObjectData {
    /// The vertices this object uploads: none when it shares another's.
    fn vertices(&self) -> &[GpuVertex] {
        match &self.source {
            ObjectSource::Flattened { vertices, .. } => vertices,
            ObjectSource::Uploaded(_) | ObjectSource::SameAs(_) => &[],
        }
    }

    /// The indices this object uploads: none when it shares another's.
    fn indices(&self) -> &[u32] {
        match &self.source {
            ObjectSource::Flattened { indices, .. } => indices,
            ObjectSource::Uploaded(_) | ObjectSource::SameAs(_) => &[],
        }
    }
}

/// A mesh the scene draws, with the first object that draws it.
struct MeshRecord {
    key: MeshKey,
    mesh: Weak<IndexedMesh>,
    local_aabb: Option<Aabb>,
    object: usize,
}

/// Refuses the first leaf (pre-order, vertex buffer before index buffer) whose
/// buffer would exceed `max_buffer_size`. Both element sizes are multiples of
/// wgpu's copy alignment, so the byte counts here are the sizes that will be
/// requested.
fn check_leaf_buffers_fit(
    objects: &[ObjectData],
    max_buffer_size: u64,
) -> std::result::Result<(), LeafBufferExceedsDeviceLimit> {
    for obj in objects {
        let vertex_bytes = size_of_val(obj.vertices()) as u64;
        if vertex_bytes > max_buffer_size {
            return Err(LeafBufferExceedsDeviceLimit {
                node_id: obj.node_id,
                leaf_name: obj.name.clone(),
                buffer_kind: LeafBufferKind::Vertex,
                num_elements: obj.vertices().len(),
                num_bytes: vertex_bytes,
                max_buffer_size,
            });
        }
        let index_bytes = size_of_val(obj.indices()) as u64;
        if index_bytes > max_buffer_size {
            return Err(LeafBufferExceedsDeviceLimit {
                node_id: obj.node_id,
                leaf_name: obj.name.clone(),
                buffer_kind: LeafBufferKind::Index,
                num_elements: obj.indices().len() / 3,
                num_bytes: index_bytes,
                max_buffer_size,
            });
        }
    }
    Ok(())
}

struct SceneData {
    objects: Vec<ObjectData>,
    meshes: Vec<MeshRecord>,
    aabb_min: Vec3,
    aabb_max: Vec3,
}

/// Builds, on the CPU, the per-object draw data and the world-space scene AABB,
/// apart from buffer creation so that it can be unit-tested without a
/// `wgpu::Device`. A mesh that `uploaded` holds, or that an earlier leaf draws
/// the same way, is not flattened again: its object takes those buffers.
/// `on_object_flattened` receives each object's triangle count (0 for
/// geometry-less leaves), in traversal order, whether flattened or shared; the
/// progress plumbing in [`GpuScene::build_with_progress`] is its only non-test
/// consumer.
fn build_gpu_scene_data(
    graph: &Scene,
    uploaded: &UploadedMeshes,
    mut on_object_flattened: impl FnMut(u64),
) -> Result<SceneData> {
    let mut objects: Vec<ObjectData> = Vec::new();
    let mut meshes: Vec<MeshRecord> = Vec::new();
    let mut record_of: HashMap<MeshKey, usize> = HashMap::new();
    // The union of each leaf's local box placed by Arvo's method under its
    // world transform: what `Scene::aabb` computes, from the boxes kept with
    // the uploaded meshes instead of a walk over every vertex.
    let mut scene_box = Aabb::empty();
    let mut has_box = false;
    graph.for_each_expanded_leaf::<SceneError, _>(
        CsgPolicy::Descend,
        |node_id, world_transform, mesh| {
            let triangles = mesh.faces().len() as u64;
            on_object_flattened(triangles);
            let name = graph
                .node(node_id)
                .and_then(|node| node.meta.as_ref())
                .and_then(|meta| meta.name.clone())
                .or_else(|| mesh.meta.as_ref().and_then(|meta| meta.name.clone()));
            let shared = shared_mesh(graph, node_id, mesh);
            let key = shared.map(MeshKey::of);
            let earlier = key
                .and_then(|key| record_of.get(&key).copied())
                .map(|record| (meshes[record].object, meshes[record].local_aabb));
            let (source, local_aabb) = match earlier {
                Some((object, local_aabb)) => (ObjectSource::SameAs(object), local_aabb),
                None => {
                    let (source, local_aabb) = match key.and_then(|key| uploaded.get(key)) {
                        Some(entry) => (
                            ObjectSource::Uploaded(Arc::clone(&entry.buffers)),
                            entry.local_aabb,
                        ),
                        None => {
                            let smooth = key.map_or_else(
                                || super::smooth_shading::shades_smooth(mesh),
                                |key| key.smooth,
                            );
                            (flatten(mesh, smooth), mesh.local_aabb())
                        }
                    };
                    if let (Some(key), Some(shared)) = (key, shared) {
                        record_of.insert(key, meshes.len());
                        meshes.push(MeshRecord {
                            key,
                            mesh: Arc::downgrade(shared),
                            local_aabb,
                            object: objects.len(),
                        });
                    }
                    (source, local_aabb)
                }
            };
            if let Some(local) = local_aabb {
                let world = match world_transform {
                    Transform3D::Identity => local,
                    Transform3D::Affine(a) => local.transformed_arvo(a),
                };
                scene_box.union(&world);
                has_box = true;
            }
            objects.push(ObjectData {
                node_id,
                name,
                triangles,
                source,
            });
            Ok(())
        },
    )?;

    if objects.is_empty() {
        return Err(NoLeavesToRender.into());
    }

    // If every leaf is empty, fall back to a degenerate AABB at the origin
    // (the renderer's diagonal-clamp `.max(1e-3)` keeps the projection matrix
    // well-defined).
    let (aabb_min, aabb_max) = if has_box {
        (
            Vec3::from(scene_box.min().coords()),
            Vec3::from(scene_box.max().coords()),
        )
    } else {
        (Vec3::ZERO, Vec3::ZERO)
    };

    Ok(SceneData {
        objects,
        meshes,
        aabb_min,
        aabb_max,
    })
}

/// The vertices and indices of `mesh` for its own upload, shaded smooth when
/// `smooth`.
fn flatten(mesh: &IndexedMesh, smooth: bool) -> ObjectSource {
    if mesh.faces().is_empty() {
        // A leaf with no geometry contributes nothing to the draw set, but we
        // keep its slot in the model buffer so the per-frame
        // `build_model_entries` walk (which uses the same pre-order traversal)
        // lines up index-by-index with the renderer's per-leaf draw loop. The
        // slot is drawn as a no-op (an `index_count` of 0).
        return ObjectSource::Flattened {
            vertices: Vec::new(),
            indices: Vec::new(),
        };
    }
    // Supports always shade smooth; models only when the preference asks for
    // it.
    if smooth {
        let smooth = super::smooth_shading::smooth_mesh(mesh);
        return ObjectSource::Flattened {
            vertices: smooth.vertices.clone(),
            indices: smooth.indices.clone(),
        };
    }
    let vertices: Vec<GpuVertex> = mesh
        .vertices()
        .iter()
        .map(|v| GpuVertex::at(v.coords()))
        .collect();
    let mut indices: Vec<u32> = Vec::with_capacity(mesh.faces().len() * 3);
    for face in mesh.faces() {
        for vidx in face.vertices {
            indices.push(u32::from(vidx));
        }
    }
    ObjectSource::Flattened { vertices, indices }
}
#[cfg(test)]
mod build_gpu_scene_data_tests {
    use std::sync::Arc;

    use super::*;
    use float_cmp::assert_approx_eq;
    use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
    use sc_mesh_core::transform::AffineTransform3D;
    use sc_scene::{LeafPayload, MeshResourceId};

    /// Builds an empty graph shaped as the loader builds it: an identity group
    /// root and no leaves.
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
            faces: vec![IndexedTriangle::from_raw_indices(face_vertices)],
        }
    }

    /// Every leaf after the first that draws a mesh shares the first one's
    /// buffers instead of flattening the mesh again (#353).
    #[test]
    fn a_mesh_drawn_by_several_leaves_is_flattened_once() {
        let (mut g, root) = graph_with_group_root();
        let rid = g.add_mesh_resource(triangle_mesh(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            [0, 1, 2],
        ));
        add_resource_leaf(&mut g, root, rid, Transform3D::Identity);
        add_resource_leaf(
            &mut g,
            root,
            rid,
            Transform3D::Affine(AffineTransform3D::from_translation([5.0, 0.0, 0.0])),
        );
        let mut ticks = Vec::new();
        let data = build_gpu_scene_data(&g, &UploadedMeshes::default(), |t| ticks.push(t)).unwrap();
        assert_eq!(data.objects[0].indices(), vec![0, 1, 2]);
        assert!(matches!(data.objects[1].source, ObjectSource::SameAs(0)));
        assert_eq!(data.meshes.len(), 1);
        // Both objects still count their triangles for the progress.
        assert_eq!(ticks, vec![1, 1]);
        assert_eq!(data.objects[1].triangles, 1);
    }

    /// The scene box comes from the local boxes kept with the meshes, and is
    /// exactly the box `Scene::aabb` walks the vertices for.
    #[test]
    fn scene_box_is_bit_identical_to_the_scene_aabb() {
        let mut g = Scene::new();
        let tilt = Transform3D::Affine(
            AffineTransform3D::from_rotation_z(0.3)
                .compose(&AffineTransform3D::from_translation([1.25, -3.5, 0.75])),
        );
        let group = g.add_group(None, tilt);
        g.add_root(group).unwrap();
        let rid = g.add_mesh_resource(triangle_mesh(
            vec![[0.1, 0.2, 0.3], [4.7, -1.3, 2.2], [-0.9, 3.3, 1.1]],
            [0, 1, 2],
        ));
        add_resource_leaf(
            &mut g,
            group,
            rid,
            Transform3D::Affine(AffineTransform3D::from_rotation_x(1.1)),
        );
        add_resource_leaf(&mut g, group, rid, Transform3D::Identity);
        add_inline_leaf(
            &mut g,
            group,
            triangle_mesh(
                vec![[-7.5, 2.0, 0.0], [3.0, 8.25, -6.0], [0.0, 0.0, 9.0]],
                [0, 1, 2],
            ),
            Transform3D::Affine(AffineTransform3D::from_rotation_y(-0.7)),
        );
        let data = build_gpu_scene_data(&g, &UploadedMeshes::default(), |_| {}).unwrap();
        let aabb = g.aabb().unwrap().unwrap();
        assert_eq!(data.aabb_min, Vec3::from(aabb.min().coords()));
        assert_eq!(data.aabb_max, Vec3::from(aabb.max().coords()));
    }

    #[test]
    fn empty_graph_returns_error() {
        // A Group root with no children: the shape an empty-file load produces.
        let (graph, _root) = graph_with_group_root();
        let Err(err) = build_gpu_scene_data(&graph, &UploadedMeshes::default(), |_| {}) else {
            panic!("a graph with no leaf has nothing to render");
        };
        assert_eq!(err.to_string(), "Graph has no leaves to render");
        // A value, not a text: the dialog formats it again after a language
        // change.
        assert!(err.downcast_ref::<NoLeavesToRender>().is_some(), "{err:?}");
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
        let data = build_gpu_scene_data(&g, &UploadedMeshes::default(), |_| {}).unwrap();
        assert_eq!(data.objects.len(), 1);
        assert_eq!(data.objects[0].vertices().len(), 3);
        assert_eq!(data.objects[0].indices(), vec![0, 1, 2]);
    }

    #[test]
    fn vertices_stay_local_under_identity() {
        let mesh = triangle_mesh(
            vec![[1.0, -2.0, 3.0], [4.0, 5.0, -6.0], [0.0, 0.0, 0.0]],
            [0, 1, 2],
        );
        let (mut g, root) = graph_with_group_root();
        add_inline_leaf(&mut g, root, mesh, Transform3D::Identity);
        let data = build_gpu_scene_data(&g, &UploadedMeshes::default(), |_| {}).unwrap();
        assert_eq!(data.objects.len(), 1);
        assert_eq!(data.objects[0].vertices()[0].position, [1.0, -2.0, 3.0]);
        assert_eq!(data.objects[0].vertices()[1].position, [4.0, 5.0, -6.0]);
        assert_eq!(data.objects[0].vertices()[2].position, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn vertices_stay_local_even_with_affine_transform() {
        // Vertex buffers are local-space; the transform is applied at draw time
        // via the per-leaf model uniform, so `build_gpu_scene_data` must not
        // bake it into the vertex stream.
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
        let data = build_gpu_scene_data(&g, &UploadedMeshes::default(), |_| {}).unwrap();
        assert_eq!(data.objects[0].vertices()[0].position, [1.0, 0.0, 0.0]);
        assert_eq!(data.objects[0].vertices()[1].position, [0.0, 1.0, 0.0]);
        assert_eq!(data.objects[0].vertices()[2].position, [0.0, 0.0, 1.0]);
    }

    #[test]
    fn indices_are_local_per_leaf_not_offset() {
        // With per-leaf draws, each leaf's index buffer references its own
        // vertex buffer, so indices stay local, not offset by previous leaves'
        // vertex counts.
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
        let data = build_gpu_scene_data(&g, &UploadedMeshes::default(), |_| {}).unwrap();
        assert_eq!(data.objects.len(), 2);
        assert_eq!(data.objects[0].indices(), vec![0, 1, 2]);
        assert_eq!(data.objects[1].indices(), vec![0, 1, 2]);
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
        let data = build_gpu_scene_data(&g, &UploadedMeshes::default(), |_| {}).unwrap();
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
        let data = build_gpu_scene_data(&g, &UploadedMeshes::default(), |_| {}).unwrap();
        assert_eq!(data.aabb_min, Vec3::new(-7.0, -2.0, -6.0));
        assert_eq!(data.aabb_max, Vec3::new(4.0, 8.0, 9.0));
    }

    #[test]
    fn empty_leaf_keeps_a_slot_in_the_leaf_list() {
        // A leaf with no faces still occupies a slot in the per-leaf pre-order
        // index; the renderer binds the model-buffer slot at that index for
        // each draw, so the GpuScene must keep a parallel slot. The slot must
        // be drawable as a no-op (an `index_count` of 0).
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
        let data = build_gpu_scene_data(&g, &UploadedMeshes::default(), |_| {}).unwrap();
        assert_eq!(data.objects.len(), 2);
        assert!(data.objects[0].indices().is_empty());
        assert_eq!(data.objects[1].indices().len(), 3);
    }

    #[test]
    fn singular_transform_is_accepted() {
        // `Scene::aabb` uses Arvo's formula, which does not fail on a singular
        // linear part, and the per-leaf data does not invert the transform, so
        // a degenerate transform does not stop the build. Only the bake behind
        // `merge_meshes` and export rejects a singular transform.
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
        // Singular transforms are not rejected here; the GPU path accepts a
        // singular model matrix (it collapses the mesh onto a plane). The
        // assertion pins this, so the test fails if the behaviour changes.
        assert!(build_gpu_scene_data(&g, &UploadedMeshes::default(), |_| {}).is_ok());
    }
}

#[cfg(test)]
mod transform_to_mat4_tests {
    use std::sync::Arc;

    use super::*;
    use float_cmp::assert_approx_eq;
    use sc_mesh_core::transform::AffineTransform3D;
    use sc_scene::LeafPayload;

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
        // The matrix should transform vertices the same way the mesh-core
        // `apply_to_vertex` does; that is the contract the shader relies on.
        let a = AffineTransform3D::from_translation([1.0, 2.0, 3.0]).compose(
            &AffineTransform3D::from_rotation_z(std::f64::consts::FRAC_PI_4)
                .compose(&AffineTransform3D::from_non_uniform_scale([2.0, 3.0, 4.0])),
        );
        let m = affine_to_mat4(&a);
        for v in [[1.0_f32, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
            let by_matrix = m.transform_point3(Vec3::from(v));
            let by_apply = a.apply_to_vertex(Vertex::new(v));
            assert_approx_eq!(f32, by_matrix.x, by_apply[0], epsilon = 1e-5);
            assert_approx_eq!(f32, by_matrix.y, by_apply[1], epsilon = 1e-5);
            assert_approx_eq!(f32, by_matrix.z, by_apply[2], epsilon = 1e-5);
        }
    }

    /// Returns a roomy envelope that contains every fixture in this module, so
    /// tests focused on the selection and model-matrix lanes are not disturbed
    /// by the breach detection of `build_model_entries`.
    fn permissive_volume() -> BuildVolume {
        BuildVolume::new(1000.0, 1000.0, 1000.0)
    }

    /// Builds an empty graph shaped as the loader builds it: an identity group
    /// root and no leaves.
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

    /// Returns the [`NodeId`] of the `n`-th leaf in pre-order traversal, used
    /// by the selection tests to feed `build_model_entries` a real id instead
    /// of a counter.
    fn nth_leaf(graph: &Scene, n: usize) -> NodeId {
        let mut ids: Vec<NodeId> = Vec::new();
        let _ = graph.for_each_expanded_leaf::<SceneError, _>(CsgPolicy::Descend, |id, _, _| {
            ids.push(id);
            Ok(())
        });
        ids.into_iter()
            .nth(n)
            .expect("test fixture should expose enough leaves")
    }

    #[test]
    fn build_model_entries_one_per_leaf() {
        use sc_mesh_core::primitives::{IndexedTriangle, Vertex};

        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new([0.0, 0.0, 0.0])],
            faces: vec![IndexedTriangle::from_raw_indices([0, 0, 0])],
        };
        let (mut g, root) = graph_with_group_root();
        add_inline_leaf(&mut g, root, mesh.clone(), Transform3D::Identity);
        add_inline_leaf(
            &mut g,
            root,
            mesh,
            Transform3D::Affine(AffineTransform3D::from_translation([7.0, 0.0, 0.0])),
        );
        let entries = build_model_entries(
            &g,
            &[],
            permissive_volume(),
            &mut WorldBoxes::default(),
            false,
        )
        .expect("clean graph");
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
        use sc_mesh_core::primitives::{IndexedTriangle, Vertex};

        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new([0.0, 0.0, 0.0])],
            faces: vec![IndexedTriangle::from_raw_indices([0, 0, 0])],
        };
        let (mut g, root) = graph_with_group_root();
        add_inline_leaf(&mut g, root, mesh.clone(), Transform3D::Identity);
        add_inline_leaf(&mut g, root, mesh, Transform3D::Identity);
        g
    }

    #[test]
    fn no_selection_leaves_every_flags_x_zero() {
        let graph = two_object_graph();
        let entries = build_model_entries(
            &graph,
            &[],
            permissive_volume(),
            &mut WorldBoxes::default(),
            false,
        )
        .expect("clean graph");
        assert_eq!(entries[0].flags[0], 0);
        assert_eq!(entries[1].flags[0], 0);
    }

    #[test]
    fn selected_node_id_sets_only_that_entrys_flags_x() {
        let graph = two_object_graph();
        let entries = build_model_entries(
            &graph,
            &[nth_leaf(&graph, 1)],
            permissive_volume(),
            &mut WorldBoxes::default(),
            false,
        )
        .expect("clean graph");
        assert_eq!(entries[0].flags[0], 0);
        assert_eq!(entries[1].flags[0], 1);
    }

    #[test]
    fn non_leaf_selection_falls_back_to_no_highlight() {
        // The Group root that wraps the leaves, as the loader builds it, is not
        // a leaf itself, so the iterator never visits it; flags.x stays 0 on
        // every entry when the selection points at the root.
        let graph = two_object_graph();
        let entries = build_model_entries(
            &graph,
            &[graph.roots()[0]],
            permissive_volume(),
            &mut WorldBoxes::default(),
            false,
        )
        .expect("clean graph");
        assert_eq!(entries[0].flags[0], 0);
        assert_eq!(entries[1].flags[0], 0);
    }

    fn unit_triangle_graph_at(translation: [f64; 3]) -> Scene {
        use sc_mesh_core::primitives::{IndexedTriangle, Vertex};

        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
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

    /// Returns a 100 mm x 60 mm plate, 80 mm tall: envelope is X in [-50, 50],
    /// Y in [-30, 30], Z in [0, 80]. Tight enough that small fixtures can be
    /// placed strictly inside or strictly outside with simple translations.
    fn snug_volume() -> BuildVolume {
        BuildVolume::new(100.0, 60.0, 80.0)
    }

    #[test]
    fn flags_y_is_zero_for_fully_inside_leaf() {
        let graph = unit_triangle_graph_at([0.0, 0.0, 1.0]);
        let entries = build_model_entries(
            &graph,
            &[],
            snug_volume(),
            &mut WorldBoxes::default(),
            false,
        )
        .expect("clean graph");
        assert_eq!(entries[0].flags[1], 0);
    }

    #[test]
    fn flags_y_is_one_when_any_vertex_is_past_a_side_wall() {
        // Translation +60 mm on X puts the unit triangle's vertices at
        // [60,0,0], [61,0,0] and [60,1,0], all 3 past the +X wall at x = 50.
        let graph = unit_triangle_graph_at([60.0, 0.0, 0.0]);
        let entries = build_model_entries(
            &graph,
            &[],
            snug_volume(),
            &mut WorldBoxes::default(),
            false,
        )
        .expect("clean graph");
        assert_eq!(entries[0].flags[1], 1);
    }

    #[test]
    fn flags_y_is_one_when_any_vertex_dips_below_the_floor() {
        // Translation in Z by -0.5 puts every vertex below z = 0.
        let graph = unit_triangle_graph_at([0.0, 0.0, -0.5]);
        let entries = build_model_entries(
            &graph,
            &[],
            snug_volume(),
            &mut WorldBoxes::default(),
            false,
        )
        .expect("clean graph");
        assert_eq!(entries[0].flags[1], 1);
    }

    #[test]
    fn flags_y_is_one_when_leaf_straddles_a_wall() {
        // Translation +49.5 mm on X: vertex [0,0,0] lands at x = 49.5 (inside),
        // [1,0,0] at x = 50.5 (outside +X wall), so the leaf breaches through
        // the second vertex.
        let graph = unit_triangle_graph_at([49.5, 0.0, 0.0]);
        let entries = build_model_entries(
            &graph,
            &[],
            snug_volume(),
            &mut WorldBoxes::default(),
            false,
        )
        .expect("clean graph");
        assert_eq!(entries[0].flags[1], 1);
    }

    #[test]
    fn flags_y_is_unset_for_leaf_resting_exactly_on_the_floor() {
        // Every vertex has z = 0 (the floor). The breach test uses a strict
        // comparison, so resting on the plate is "inside".
        let graph = unit_triangle_graph_at([0.0, 0.0, 0.0]);
        let entries = build_model_entries(
            &graph,
            &[],
            snug_volume(),
            &mut WorldBoxes::default(),
            false,
        )
        .expect("clean graph");
        assert_eq!(entries[0].flags[1], 0);
    }

    #[test]
    fn warning_mask_is_zero_when_every_leaf_fits() {
        let graph = unit_triangle_graph_at([0.0, 0.0, 1.0]);
        assert_eq!(
            build_envelope_warning_mask(&graph, snug_volume(), &mut WorldBoxes::default())
                .expect("clean graph"),
            0
        );
    }

    #[test]
    fn warning_mask_sets_plus_x_when_a_vertex_passes_the_plus_x_wall() {
        // Translation +60 on X puts every vertex past the +X wall (x = 50);
        // vertex [1,0,0] lands at [61,0,0].
        let graph = unit_triangle_graph_at([60.0, 0.0, 0.0]);
        let mask = build_envelope_warning_mask(&graph, snug_volume(), &mut WorldBoxes::default())
            .expect("clean graph");
        assert_eq!(mask & FACE_BIT_X_POS, FACE_BIT_X_POS);
        assert_eq!(mask & FACE_BIT_X_NEG, 0);
    }

    #[test]
    fn warning_mask_sets_minus_y_when_a_vertex_passes_the_minus_y_wall() {
        // Vertex [0,1,0] translated by [0,-40,0] lands at [0,-39,0], and
        // [0,0,0] lands at [0,-40,0], past the -Y wall (-30).
        let graph = unit_triangle_graph_at([0.0, -40.0, 0.0]);
        let mask = build_envelope_warning_mask(&graph, snug_volume(), &mut WorldBoxes::default())
            .expect("clean graph");
        assert_eq!(mask & FACE_BIT_Y_NEG, FACE_BIT_Y_NEG);
        assert_eq!(mask & FACE_BIT_Y_POS, 0);
    }

    #[test]
    fn warning_mask_sets_plus_z_when_a_vertex_passes_the_ceiling() {
        // Every vertex lands at z = 90, past the +Z ceiling (80).
        let graph = unit_triangle_graph_at([0.0, 0.0, 90.0]);
        let mask = build_envelope_warning_mask(&graph, snug_volume(), &mut WorldBoxes::default())
            .expect("clean graph");
        assert_eq!(mask & FACE_BIT_Z_POS, FACE_BIT_Z_POS);
    }

    #[test]
    fn warning_mask_sets_minus_z_when_a_vertex_dips_below_the_floor() {
        // The Z = 0 floor breach is still represented in the mask (bit
        // FACE_BIT_Z_NEG); the plate shader consumes it through the
        // floor-overlay path rather than a dedicated wall quad.
        let graph = unit_triangle_graph_at([0.0, 0.0, -1.0]);
        let mask = build_envelope_warning_mask(&graph, snug_volume(), &mut WorldBoxes::default())
            .expect("clean graph");
        assert_eq!(mask & FACE_BIT_Z_NEG, FACE_BIT_Z_NEG);
        // None of the lateral bits fire for a purely-vertical breach.
        assert_eq!(
            mask & (FACE_BIT_X_NEG | FACE_BIT_X_POS | FACE_BIT_Y_NEG | FACE_BIT_Y_POS),
            0
        );
    }

    #[test]
    fn warning_mask_unions_bits_across_leaves() {
        // First leaf breaches +X, second breaches -Y. The mask collects both
        // bits.
        use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
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
        let mask = build_envelope_warning_mask(&g, snug_volume(), &mut WorldBoxes::default())
            .expect("clean graph");
        assert_eq!(mask & FACE_BIT_X_POS, FACE_BIT_X_POS);
        assert_eq!(mask & FACE_BIT_Y_NEG, FACE_BIT_Y_NEG);
    }

    fn unit_triangle() -> IndexedMesh {
        use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
        IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
        }
    }

    /// Replaces the box remembered in slot `index` with a box no walk can
    /// produce, so a later hit is told apart from a fresh walk.
    fn plant_marker(boxes: &mut WorldBoxes, index: usize) -> Aabb {
        let marker = Aabb::from_min_max(
            Vertex::new([-123.0, -123.0, -123.0]),
            Vertex::new([123.0, 123.0, 123.0]),
        );
        boxes.slots[index].as_mut().expect("a remembered box").aabb = marker;
        marker
    }

    #[test]
    fn world_box_is_reused_only_for_the_same_mesh_and_matrix() {
        let mesh = unit_triangle();
        let other_mesh = unit_triangle();
        let a = transform_to_mat4(&Transform3D::Affine(
            AffineTransform3D::from_translation([1.0, 2.0, 3.0])
                .compose(&AffineTransform3D::from_rotation_z(0.3)),
        ));
        let b = transform_to_mat4(&Transform3D::Affine(AffineTransform3D::from_translation([
            1.0, 2.0, 3.5,
        ])));
        let mut boxes = WorldBoxes::default();

        assert_eq!(
            boxes.world_box(0, &mesh, None, &a),
            leaf_world_box(&mesh, &a)
        );
        let marker = plant_marker(&mut boxes, 0);
        assert_eq!(
            boxes.world_box(0, &mesh, None, &a),
            marker,
            "same mesh and matrix"
        );

        assert_eq!(
            boxes.world_box(0, &mesh, None, &b),
            leaf_world_box(&mesh, &b)
        );
        plant_marker(&mut boxes, 0);
        assert_eq!(
            boxes.world_box(0, &other_mesh, None, &b),
            leaf_world_box(&other_mesh, &b),
            "an equal mesh elsewhere is another mesh"
        );

        plant_marker(&mut boxes, 0);
        boxes.clear();
        assert_eq!(
            boxes.world_box(0, &other_mesh, None, &b),
            leaf_world_box(&other_mesh, &b)
        );
    }

    #[test]
    fn world_boxes_keep_each_occurrence_of_a_shared_mesh_apart() {
        let mesh = unit_triangle();
        let near = transform_to_mat4(&Transform3D::Identity);
        let far = transform_to_mat4(&Transform3D::Affine(AffineTransform3D::from_translation([
            60.0, 0.0, 0.0,
        ])));
        let mut boxes = WorldBoxes::default();
        for _ in 0..2 {
            assert_eq!(
                boxes.world_box(0, &mesh, None, &near),
                leaf_world_box(&mesh, &near)
            );
            assert_eq!(
                boxes.world_box(1, &mesh, None, &far),
                leaf_world_box(&mesh, &far)
            );
        }
    }

    #[test]
    fn moving_a_leaf_updates_its_warning_across_frames() {
        // One cache across the frames, as the renderer keeps it.
        let mut graph = unit_triangle_graph_at([0.0, 0.0, 1.0]);
        let leaf = nth_leaf(&graph, 0);
        let mut boxes = WorldBoxes::default();
        let frame = |graph: &Scene, boxes: &mut WorldBoxes| {
            let entries =
                build_model_entries(graph, &[], snug_volume(), boxes, false).expect("clean graph");
            let mask =
                build_envelope_warning_mask(graph, snug_volume(), boxes).expect("clean graph");
            (entries[0].flags[1], mask)
        };
        assert_eq!(frame(&graph, &mut boxes), (0, 0));
        assert_eq!(frame(&graph, &mut boxes), (0, 0));

        let past_plus_x =
            Transform3D::Affine(AffineTransform3D::from_translation([60.0, 0.0, 0.0]));
        graph
            .set_local_transform(leaf, past_plus_x)
            .expect("live leaf");
        assert_eq!(frame(&graph, &mut boxes), (1, FACE_BIT_X_POS));

        let inside = Transform3D::Affine(AffineTransform3D::from_translation([0.0, 0.0, 1.0]));
        graph.set_local_transform(leaf, inside).expect("live leaf");
        assert_eq!(frame(&graph, &mut boxes), (0, 0));
    }

    #[test]
    fn dragging_a_turned_scaled_model_across_the_edge_warns_as_a_full_walk() {
        let mesh = Arc::new(super::super::hull_points::lumpy_mesh(160, 200));
        let mut graph = Scene::new();
        let pose = |x: f64, y: f64, z: f64| {
            Transform3D::Affine(
                AffineTransform3D::from_translation([x, y, z]).compose(
                    &AffineTransform3D::from_rotation_z(0.7)
                        .compose(&AffineTransform3D::from_rotation_x(0.4))
                        .compose(&AffineTransform3D::from_non_uniform_scale([1.3, 0.8, 1.1])),
                ),
            )
        };
        let leaf = graph
            .add_leaf(None, pose(0.0, 0.0, 0.0), LeafPayload::Mesh(mesh.clone()))
            .expect("add leaf");
        graph.add_root(leaf).expect("root");
        let volume = snug_volume();
        let mut boxes = WorldBoxes::default();
        build_model_entries(&graph, &[leaf], volume, &mut boxes, false).expect("clean graph");
        let start = std::time::Instant::now();
        while boxes.hulls.get(&mesh).is_none() {
            assert!(start.elapsed().as_secs() < 30, "the hull never arrived");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        // Rest the model on the floor, then put a box face exactly on each
        // wall it is dragged across, and step around those spots.
        let rest = leaf_world_box(&mesh, &transform_to_mat4(&pose(0.0, 0.0, 0.0)));
        let z = -f64::from(rest.min()[2]);
        let on_plus_x = 50.0 - f64::from(rest.max()[0]);
        let on_minus_y = -30.0 - f64::from(rest.min()[1]);
        let mut poses = Vec::new();
        for step in -400..=400 {
            let d = f64::from(step) * 0.0125;
            poses.push(pose(on_plus_x + d, 0.0, z));
            poses.push(pose(0.0, on_minus_y + d, z));
            poses.push(pose(on_plus_x + d, on_minus_y - d, z + d / 50.0));
        }
        for tiny in [-1e-5, -1e-6, 0.0, 1e-6, 1e-5] {
            poses.push(pose(on_plus_x + tiny, 0.0, z));
            poses.push(pose(0.0, on_minus_y + tiny, z + tiny));
        }

        let mut warned = 0;
        for transform in poses {
            graph
                .set_local_transform(leaf, transform)
                .expect("live leaf");
            let matrix = transform_to_mat4(&transform);
            let hull = boxes.hulls.get(&mesh).expect("a ready hull");
            assert!(hull.world_box(&matrix).is_some(), "the hull answers");
            let walked = leaf_world_box(&mesh, &matrix);
            let entries = build_model_entries(&graph, &[leaf], volume, &mut boxes, false)
                .expect("clean graph");
            let mask =
                build_envelope_warning_mask(&graph, volume, &mut boxes).expect("clean graph");
            let expected_mask = volume
                .volume()
                .walls_crossed_by(&walked)
                .iter()
                .fold(0, |mask, wall| mask | face_bit(wall));
            let expected_flag = u32::from(!volume.volume().contains(&walked));
            assert_eq!(
                (entries[0].flags[1], mask),
                (expected_flag, expected_mask),
                "{transform:?}"
            );
            assert_eq!(boxes.slots[0].as_ref().map(|s| s.aabb), Some(walked));
            warned += expected_flag;
        }
        assert!(
            warned > 1000,
            "the sweep crosses the walls ({warned} warnings)"
        );
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

/// The walk-error arm of `renderable_leaf_count` is not covered here:
/// `Scene::add_leaf` rejects an unbound `MeshResourceId` and
/// `Scene::remove_mesh_resource` rejects a still-referenced one, so a graph
/// with a dangling id cannot be built through the public API. The walk's own
/// error propagation is pinned by `sc-scene`'s suite.
#[cfg(test)]
mod renderable_leaf_count_tests {
    use std::sync::Arc;

    use super::*;
    use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
    use sc_scene::LeafPayload;

    fn unit_triangle() -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
        }
    }

    /// Builds an empty graph shaped as the loader builds it: an identity group
    /// root and no leaves.
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
        // The count must follow the whole subtree, not just the roots' direct
        // children: 1 leaf sits under the root, the other 2 under a Group
        // nested inside it.
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
        // The STL-loaded shape: the leaf is a scene root itself rather than a
        // child of an identity Group.
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
        // The shape the scene lands in when the user deletes the last object: a
        // live graph whose leaf population is empty.
        let (mut graph, root) = graph_with_group_root();
        let leaf = add_inline_leaf(&mut graph, root);
        graph.remove_leaf(leaf).expect("remove the only leaf");
        assert_eq!(renderable_leaf_count(&graph).expect("clean walk"), 0);
    }

    #[test]
    fn removing_the_only_root_leaf_drops_the_count_to_zero() {
        // Same, for the STL shape: removing a root leaf leaves a graph with no
        // roots at all.
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
    use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
    use sc_scene::node::{LeafPayload, SceneMetadata};

    /// Small enough that a mesh of 86 faces exceeds it.
    const LIMIT: u64 = 1024;

    /// Builds a mesh of `vertex_count` vertices and `face_count` faces on the
    /// first 3 vertices: only those 3 are uploaded, and `12 * face_count`
    /// index bytes.
    fn mesh(vertex_count: usize, face_count: usize, name: Option<&str>) -> IndexedMesh {
        IndexedMesh {
            meta: name.map(|n| MeshMetadata {
                name: Some(n.to_string()),
                ..Default::default()
            }),
            vertices: (0..vertex_count)
                .map(|i| Vertex::new([i as f32, 0.0, 0.0]))
                .collect(),
            faces: (0..face_count)
                .map(|_| IndexedTriangle::from_raw_indices([0, 1, 2]))
                .collect(),
        }
    }

    /// A fan of `vertex_count` vertices, every one of them used: `24 *
    /// vertex_count` vertex bytes (position and normal) and `12 *
    /// (vertex_count - 2)` index bytes.
    fn fan(vertex_count: usize) -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: (0..vertex_count)
                .map(|i| Vertex::new([i as f32, 0.0, 0.0]))
                .collect(),
            faces: (1..vertex_count as u32 - 1)
                .map(|k| IndexedTriangle::from_raw_indices([0, k, k + 1]))
                .collect(),
        }
    }

    /// Builds a graph with 1 root leaf per mesh, in order; returns the graph
    /// and the leaf ids.
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
        let data =
            build_gpu_scene_data(graph, &UploadedMeshes::default(), |_| {}).expect("flatten");
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
        let (graph, ids) = graph_of(vec![(fan(43), None)]);
        assert_eq!(
            check(&graph),
            Err(LeafBufferExceedsDeviceLimit {
                node_id: ids[0],
                leaf_name: None,
                buffer_kind: LeafBufferKind::Vertex,
                num_elements: 43,
                num_bytes: 1032,
                max_buffer_size: LIMIT,
            })
        );
    }

    #[test]
    fn the_vertex_buffer_is_reported_when_both_buffers_are_over() {
        let (graph, _) = graph_of(vec![(fan(90), None)]);
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
        let (graph, _) = graph_of(vec![(fan(43), Some("shell"))]);
        assert_eq!(
            check(&graph).unwrap_err().to_string(),
            "Object 'shell' has more vertices (43) than this graphics device can hold in one buffer"
        );
    }

    #[test]
    fn an_unnamed_vertex_refusal_speaks_of_an_object() {
        let (graph, _) = graph_of(vec![(fan(43), None)]);
        assert_eq!(
            check(&graph).unwrap_err().to_string(),
            "An object has more vertices (43) than this graphics device can hold in one buffer"
        );
    }
}

#[cfg(test)]
mod upload_refusal_text_tests {
    use super::*;

    fn refusal(what: UploadBuffer) -> String {
        DeviceRefusedBuffer {
            what,
            num_bytes: 64,
            source: wgpu::Error::Validation {
                source: Box::new(std::io::Error::other("x")),
                description: "x".to_string(),
            },
        }
        .to_string()
    }

    fn object(kind: LeafBufferKind, name: Option<&str>) -> UploadBuffer {
        UploadBuffer::Object {
            kind,
            name: name.map(str::to_string),
        }
    }

    #[test]
    fn each_refused_buffer_has_its_own_sentence() {
        assert_eq!(
            refusal(object(LeafBufferKind::Vertex, Some("hull"))),
            "The graphics device refused the vertex buffer of object 'hull' (64 bytes)"
        );
        assert_eq!(
            refusal(object(LeafBufferKind::Vertex, None)),
            "The graphics device refused the vertex buffer of an object (64 bytes)"
        );
        assert_eq!(
            refusal(object(LeafBufferKind::Index, Some("hull"))),
            "The graphics device refused the index buffer of object 'hull' (64 bytes)"
        );
        assert_eq!(
            refusal(object(LeafBufferKind::Index, None)),
            "The graphics device refused the index buffer of an object (64 bytes)"
        );
        assert_eq!(
            refusal(UploadBuffer::TransformTable),
            "The graphics device refused the per-object transform table (64 bytes)"
        );
    }

    #[test]
    fn a_device_lost_during_the_upload_says_so() {
        assert_eq!(
            DeviceLostDuringUpload.to_string(),
            "The graphics device was lost while uploading the scene"
        );
    }
}
