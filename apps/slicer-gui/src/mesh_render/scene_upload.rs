/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use anyhow::{Result, anyhow};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3, Vec4};
use sc_mesh_core::scene::{Scene, SceneObjectContent, Transform3D};
use sc_mesh_core::transform::AffineTransform3D;
use slint::wgpu_28::wgpu;
use wgpu::util::DeviceExt;

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

/// GPU-resident draw data for a single `SceneObject`: its local-space
/// vertex+index buffers (the object's transform is *not* baked in — it
/// arrives at draw time through the model storage buffer).
pub(super) struct ObjectDraw {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
}

impl ObjectDraw {
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

/// GPU mirror of a `Scene`. Holds one [`ObjectDraw`] per object plus a
/// uniform buffer of [`ModelEntry`] slots, padded to the device's
/// `min_uniform_buffer_offset_alignment` so the render loop can address
/// each slot via a dynamic-offset bind. The world-space AABB is captured
/// at build time from `Scene::aabb`; it is used once for camera framing
/// and is not refreshed on per-frame transform updates (the camera
/// persists across edits, so a post-edit AABB is irrelevant for
/// framing).
///
/// Construction ([`GpuScene::build`]) touches a `wgpu::Device` but not
/// the UI loop, so it can run on the worker thread that parsed the
/// scene; the UI thread then attaches the matching bind group via
/// [`Renderer::install_scene`].
pub(crate) struct GpuScene {
    objects: Vec<ObjectDraw>,
    model_buffer: wgpu::Buffer,
    /// Distance in bytes between consecutive `ModelEntry` slots in
    /// `model_buffer`. Always ≥ `MODEL_ENTRY_SIZE` and a multiple of
    /// the device's `min_uniform_buffer_offset_alignment`.
    model_stride: u64,
    aabb_min: Vec3,
    aabb_max: Vec3,
}

impl GpuScene {
    pub(crate) fn build(device: &wgpu::Device, scene: &Scene) -> Result<Self> {
        let data = build_gpu_scene_data(scene)?;
        let mut objects = Vec::with_capacity(data.objects.len());
        for obj in &data.objects {
            let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("mesh-viewer object vertex buffer"),
                contents: bytemuck::cast_slice(&obj.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
            let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("mesh-viewer object index buffer"),
                contents: bytemuck::cast_slice(&obj.indices),
                usage: wgpu::BufferUsages::INDEX,
            });
            objects.push(ObjectDraw {
                vertex_buffer,
                index_buffer,
                index_count: obj.indices.len() as u32,
            });
        }

        let alignment = device.limits().min_uniform_buffer_offset_alignment as u64;
        let model_stride = align_up(MODEL_ENTRY_SIZE, alignment);

        let model_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mesh-viewer model uniform"),
            // The bind group requires a non-zero binding size; we already
            // bail above if the scene has no objects.
            size: model_stride * objects.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(Self {
            objects,
            model_buffer,
            model_stride,
            aabb_min: data.aabb_min,
            aabb_max: data.aabb_max,
        })
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
    let l = &a.linear;
    let t = &a.translation;
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

/// `selection` is an index into `scene.objects`; out-of-bounds values
/// (or `None`) mean nothing is selected and every entry's `flags.x`
/// stays 0. The shader reads `flags.x` and mixes a warm tint when it
/// is 1.
pub(super) fn build_model_entries(scene: &Scene, selection: Option<usize>) -> Vec<ModelEntry> {
    scene
        .objects
        .iter()
        .enumerate()
        .map(|(idx, obj)| {
            let selected = selection == Some(idx);
            ModelEntry {
                model: transform_to_mat4(&obj.transform).to_cols_array_2d(),
                flags: [u32::from(selected), 0, 0, 0],
            }
        })
        .collect()
}

/// CPU-only build of the per-object draw data plus the world-space
/// scene AABB. Kept separate from buffer creation so the transformation
/// can be unit-tested without a `wgpu::Device`.
struct ObjectData {
    vertices: Vec<GpuVertex>,
    indices: Vec<u32>,
}

struct SceneData {
    objects: Vec<ObjectData>,
    aabb_min: Vec3,
    aabb_max: Vec3,
}

fn build_gpu_scene_data(scene: &Scene) -> Result<SceneData> {
    if scene.objects.is_empty() {
        return Err(anyhow!("Scene has no geometry to render"));
    }

    let mut objects: Vec<ObjectData> = Vec::with_capacity(scene.objects.len());

    for object in &scene.objects {
        let mesh = match &object.content {
            SceneObjectContent::Mesh(m) => m,
            SceneObjectContent::Resource(idx) => scene
                .resources
                .get(*idx)
                .ok_or_else(|| anyhow!("Resource index {idx} out of bounds"))?,
        };

        if mesh.faces.is_empty() {
            // An object with no geometry contributes nothing to the
            // draw set — skip it but keep its slot in the model
            // buffer so `instance_index` keeps lining up with
            // `Scene::objects` indices in later phases. We achieve
            // that by emitting a zero-vertex / zero-index ObjectData,
            // which the renderer's per-object loop draws as a no-op
            // (index_count = 0).
            objects.push(ObjectData {
                vertices: Vec::new(),
                indices: Vec::new(),
            });
            continue;
        }

        let vertices: Vec<GpuVertex> = mesh
            .vertices
            .iter()
            .map(|v| GpuVertex {
                position: v.coords(),
            })
            .collect();

        let mut indices: Vec<u32> = Vec::with_capacity(mesh.faces.len() * 3);
        for face in &mesh.faces {
            for &vidx in &face.vertices {
                indices.push(vidx);
            }
        }

        objects.push(ObjectData { vertices, indices });
    }

    // World-space scene AABB via `Scene::aabb`, which already does Arvo
    // per object and respects each object's `Transform3D`. If every
    // object is empty, fall back to a degenerate bbox at the origin
    // (the renderer's diagonal-clamp `.max(1e-3)` keeps the projection
    // matrix well-defined).
    let (aabb_min, aabb_max) = match scene.aabb()? {
        Some((min, max)) => (Vec3::from(min.coords()), Vec3::from(max.coords())),
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
    use super::*;
    use float_cmp::assert_approx_eq;
    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
    use sc_mesh_core::scene::{Scene, SceneObject, SceneObjectContent, Transform3D};
    use sc_mesh_core::transform::AffineTransform3D;

    fn triangle_mesh(vertices: Vec<[f32; 3]>, face_vertices: [u32; 3]) -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: vertices.into_iter().map(Vertex::new).collect(),
            faces: vec![IndexedTriangle {
                normal: Normal::new([0.0, 0.0, 1.0]),
                vertices: face_vertices,
            }],
        }
    }

    fn inline_object(mesh: IndexedMesh) -> SceneObject {
        SceneObject {
            meta: None,
            transform: Transform3D::Identity,
            content: SceneObjectContent::Mesh(mesh),
        }
    }

    fn affine_object(mesh: IndexedMesh, t: AffineTransform3D) -> SceneObject {
        SceneObject {
            meta: None,
            transform: Transform3D::Affine(t),
            content: SceneObjectContent::Mesh(mesh),
        }
    }

    fn resource_object(idx: usize) -> SceneObject {
        SceneObject {
            meta: None,
            transform: Transform3D::Identity,
            content: SceneObjectContent::Resource(idx),
        }
    }

    #[test]
    fn empty_scene_returns_error() {
        let scene = Scene::default();
        assert!(build_gpu_scene_data(&scene).is_err());
    }

    #[test]
    fn dangling_resource_index_returns_error() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![resource_object(7)],
        };
        assert!(build_gpu_scene_data(&scene).is_err());
    }

    #[test]
    fn valid_resource_reference_is_resolved() {
        let mesh = triangle_mesh(
            vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [0, 1, 2],
        );
        let scene = Scene {
            meta: None,
            resources: vec![mesh.clone()],
            objects: vec![resource_object(0)],
        };
        let data = build_gpu_scene_data(&scene).unwrap();
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
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![inline_object(mesh)],
        };
        let data = build_gpu_scene_data(&scene).unwrap();
        assert_eq!(data.objects.len(), 1);
        assert_eq!(data.objects[0].vertices[0].position, [1.0, -2.0, 3.0]);
        assert_eq!(data.objects[0].vertices[1].position, [4.0, 5.0, -6.0]);
        assert_eq!(data.objects[0].vertices[2].position, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn vertices_stay_local_even_with_affine_transform() {
        // Vertex buffers are local-space; the transform is applied at
        // draw time via the per-object model uniform, so
        // `build_gpu_scene_data` must NOT bake it into the vertex
        // stream.
        let mesh = triangle_mesh(
            vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [0, 1, 2],
        );
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![affine_object(
                mesh,
                AffineTransform3D::from_translation([100.0, -50.0, 25.0]),
            )],
        };
        let data = build_gpu_scene_data(&scene).unwrap();
        assert_eq!(data.objects[0].vertices[0].position, [1.0, 0.0, 0.0]);
        assert_eq!(data.objects[0].vertices[1].position, [0.0, 1.0, 0.0]);
        assert_eq!(data.objects[0].vertices[2].position, [0.0, 0.0, 1.0]);
    }

    #[test]
    fn indices_are_local_per_object_not_offset() {
        // With per-object draws, each object's index buffer references its
        // own vertex buffer, so indices stay local — not offset by
        // previous objects' vertex counts as the merged build did.
        let a = triangle_mesh(
            vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [0, 1, 2],
        );
        let b = triangle_mesh(
            vec![[2.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 2.0]],
            [0, 1, 2],
        );
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![inline_object(a), inline_object(b)],
        };
        let data = build_gpu_scene_data(&scene).unwrap();
        assert_eq!(data.objects.len(), 2);
        assert_eq!(data.objects[0].indices, vec![0, 1, 2]);
        assert_eq!(data.objects[1].indices, vec![0, 1, 2]);
    }

    #[test]
    fn scene_aabb_reflects_world_space_under_transforms() {
        let mesh = triangle_mesh(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            [0, 1, 2],
        );
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![affine_object(
                mesh,
                AffineTransform3D::from_translation([10.0, 20.0, 30.0]),
            )],
        };
        let data = build_gpu_scene_data(&scene).unwrap();
        // Scene AABB = local AABB ([0,0,0]..[1,1,0]) translated by (10,20,30).
        assert_approx_eq!(f32, data.aabb_min.x, 10.0, epsilon = 1e-5);
        assert_approx_eq!(f32, data.aabb_min.y, 20.0, epsilon = 1e-5);
        assert_approx_eq!(f32, data.aabb_min.z, 30.0, epsilon = 1e-5);
        assert_approx_eq!(f32, data.aabb_max.x, 11.0, epsilon = 1e-5);
        assert_approx_eq!(f32, data.aabb_max.y, 21.0, epsilon = 1e-5);
        assert_approx_eq!(f32, data.aabb_max.z, 30.0, epsilon = 1e-5);
    }

    #[test]
    fn scene_aabb_unions_two_objects() {
        let a = triangle_mesh(
            vec![[1.0, -2.0, 3.0], [4.0, 5.0, -6.0], [0.0, 0.0, 0.0]],
            [0, 1, 2],
        );
        let b = triangle_mesh(
            vec![[-7.0, 8.0, 0.5], [0.0, 0.0, 9.0], [2.0, 2.0, 2.0]],
            [0, 1, 2],
        );
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![inline_object(a), inline_object(b)],
        };
        let data = build_gpu_scene_data(&scene).unwrap();
        assert_eq!(data.aabb_min, Vec3::new(-7.0, -2.0, -6.0));
        assert_eq!(data.aabb_max, Vec3::new(4.0, 8.0, 9.0));
    }

    #[test]
    fn empty_object_keeps_a_slot_in_the_object_list() {
        // An object with no faces still occupies an index in
        // `Scene::objects`; the renderer indexes into `models[]` by that
        // index, so the GpuScene must keep a parallel slot. The slot
        // must be drawable as a no-op (index_count = 0).
        let empty = IndexedMesh {
            meta: None,
            vertices: vec![],
            faces: vec![],
        };
        let nonempty = triangle_mesh(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            [0, 1, 2],
        );
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![inline_object(empty), inline_object(nonempty)],
        };
        let data = build_gpu_scene_data(&scene).unwrap();
        assert_eq!(data.objects.len(), 2);
        assert!(data.objects[0].indices.is_empty());
        assert_eq!(data.objects[1].indices.len(), 3);
    }

    #[test]
    fn singular_transform_propagates_error() {
        // `Scene::aabb` itself doesn't fail on singular linear parts
        // (it uses Arvo), so this currently succeeds. The check here
        // pins the behaviour: the per-object data is built without
        // touching `apply_to_normal`, so even a degenerate transform
        // doesn't bring the build down — degenerate matrices only
        // matter at `merge_meshes` / export time.
        let mesh = triangle_mesh(
            vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [0, 1, 2],
        );
        let singular = AffineTransform3D::from_linear_and_translation(
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 0.0]],
            [0.0, 0.0, 0.0],
        );
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![affine_object(mesh, singular)],
        };
        // Singular transforms aren't rejected here — the GPU path is
        // fine with a singular model matrix (it just collapses the
        // mesh to a plane). Mirror this in the assertion so the test
        // fails loudly if the behaviour changes.
        assert!(build_gpu_scene_data(&scene).is_ok());
    }
}

#[cfg(test)]
mod transform_to_mat4_tests {
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
        // mesh-core `apply_to_vertex` does — that is the contract the
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

    #[test]
    fn build_model_entries_one_per_object() {
        use sc_mesh_core::IndexedMesh;
        use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
        use sc_mesh_core::scene::{Scene, SceneObject, SceneObjectContent};

        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new([0.0, 0.0, 0.0])],
            faces: vec![IndexedTriangle {
                normal: Normal::new([0.0, 0.0, 1.0]),
                vertices: [0, 0, 0],
            }],
        };
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![
                SceneObject {
                    meta: None,
                    transform: Transform3D::Identity,
                    content: SceneObjectContent::Mesh(mesh.clone()),
                },
                SceneObject {
                    meta: None,
                    transform: Transform3D::Affine(AffineTransform3D::from_translation([
                        7.0, 0.0, 0.0,
                    ])),
                    content: SceneObjectContent::Mesh(mesh),
                },
            ],
        };
        let entries = build_model_entries(&scene, None);
        assert_eq!(entries.len(), 2);
        // First entry is identity.
        assert_approx_eq!(f32, entries[0].model[0][0], 1.0);
        assert_approx_eq!(f32, entries[0].model[3][0], 0.0);
        // Second entry has +7 in the translation slot (column 3, row 0).
        assert_approx_eq!(f32, entries[1].model[3][0], 7.0);
        assert_approx_eq!(f32, entries[1].model[3][1], 0.0);
        assert_approx_eq!(f32, entries[1].model[3][2], 0.0);
    }

    fn two_object_scene() -> Scene {
        use sc_mesh_core::IndexedMesh;
        use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
        use sc_mesh_core::scene::{SceneObject, SceneObjectContent};

        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new([0.0, 0.0, 0.0])],
            faces: vec![IndexedTriangle {
                normal: Normal::new([0.0, 0.0, 1.0]),
                vertices: [0, 0, 0],
            }],
        };
        Scene {
            meta: None,
            resources: vec![],
            objects: vec![
                SceneObject {
                    meta: None,
                    transform: Transform3D::Identity,
                    content: SceneObjectContent::Mesh(mesh.clone()),
                },
                SceneObject {
                    meta: None,
                    transform: Transform3D::Identity,
                    content: SceneObjectContent::Mesh(mesh),
                },
            ],
        }
    }

    #[test]
    fn no_selection_leaves_every_flags_x_zero() {
        let scene = two_object_scene();
        let entries = build_model_entries(&scene, None);
        assert_eq!(entries[0].flags[0], 0);
        assert_eq!(entries[1].flags[0], 0);
    }

    #[test]
    fn selected_index_sets_only_that_entrys_flags_x() {
        let scene = two_object_scene();
        let entries = build_model_entries(&scene, Some(1));
        assert_eq!(entries[0].flags[0], 0);
        assert_eq!(entries[1].flags[0], 1);
    }

    #[test]
    fn out_of_bounds_selection_falls_back_to_no_highlight() {
        let scene = two_object_scene();
        let entries = build_model_entries(&scene, Some(99));
        assert_eq!(entries[0].flags[0], 0);
        assert_eq!(entries[1].flags[0], 0);
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
