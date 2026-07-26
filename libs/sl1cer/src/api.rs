/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Public slicer entry points.
//!
//! [`slice_leaf`] turns one baked, manifold leaf into a vector
//! [`ContourStack`]; [`slice_scene`] applies the same pipeline to
//! every mesh leaf reachable from the scene roots and returns one
//! stack per leaf. Both compose the per-Z slice, stitch, winding
//! canonicalisation, and outer/hole grouping steps from the rest of
//! the crate into one end-to-end pipeline.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use sc_mesh_core::indexed_mesh::{MeshTopology, ValidatedManifoldMesh};
use sc_mesh_core::scene::bake::bake_leaf_to_world;
use sc_mesh_core::scene::node::{NodeId, NodeKind};
use sc_mesh_core::{IndexedMesh, Scene};

use crate::contour::{ContourLayer, ContourStack};
use crate::error::{InvalidLeafGeometry, LeafValidationFailure, SliceError, SliceSceneError};
use crate::grouping::group_into_polygons;
use crate::mesh_slice::segments_at_z;
use crate::profile::SliceProfile;
use crate::stitch::{normalise_to_ccw, stitch_layer};

/// Slice the leaf identified by `leaf` in `scene` into a
/// [`ContourStack`] using the per-layer schedule and other parameters
/// carried by `profile`.
///
/// The leaf is baked to world space, validated for face-index
/// integrity and manifoldness, then each scheduled Z plane is
/// intersected with the mesh's triangles. The resulting crossing
/// segments are stitched into closed rings, canonicalised to CCW,
/// and grouped into outer-plus-holes polygons.
///
/// The returned stack has strictly monotone Z by construction: the
/// schedule yields Z values in strict-monotone order and each is
/// emitted at most once.
///
/// # Determinism
///
/// For a fixed `(scene, leaf, profile)` and a fixed code revision,
/// the output is bitwise identical across runs. No iteration order
/// in the implementation depends on hash-map traversal order of
/// values the user can observe.
///
/// # Errors
///
/// - [`SliceError::NodeNotFound`] if `leaf` does not resolve.
/// - [`SliceError::NotALeaf`] if it resolves to a [`NodeKind::Group`].
/// - [`SliceError::CsgNotYetSupported`] /
///   [`SliceError::InstanceNotYetSupported`] for the deferred kinds.
/// - [`SliceError::BakeFailed`] when [`bake_leaf_to_world`] fails.
/// - [`SliceError::FacesValidationFailed`] /
///   [`SliceError::ManifoldCheckFailed`] when the baked mesh fails
///   validation.
/// - [`SliceError::NonManifoldDuringStitching`] if a layer's segment
///   list has dangling endpoints (a runtime safety net for input
///   that slipped the manifold check).
pub fn slice_leaf(
    scene: &Scene,
    leaf: NodeId,
    profile: &SliceProfile,
) -> Result<ContourStack, SliceError> {
    let node = scene
        .node(leaf)
        .ok_or(SliceError::NodeNotFound { node_id: leaf })?;
    match &node.kind {
        NodeKind::Leaf(_) => {}
        NodeKind::Group { .. } => return Err(SliceError::NotALeaf { node_id: leaf }),
        NodeKind::Csg { .. } => {
            return Err(SliceError::CsgNotYetSupported { node_id: leaf });
        }
        NodeKind::Instance { .. } => {
            return Err(SliceError::InstanceNotYetSupported { node_id: leaf });
        }
    }

    let baked = bake_leaf_to_world(scene, leaf)?;
    let mesh = validate_baked(&baked).map_err(SliceError::from)?;
    slice_manifold_mesh(&mesh, profile)
}

/// Contours produced for one leaf of a scene by [`slice_scene`].
#[derive(Clone, Debug, PartialEq)]
pub struct LeafSlices {
    leaf: NodeId,
    leaf_name: Option<String>,
    stack: ContourStack,
}

impl LeafSlices {
    /// The leaf node these contours belong to.
    #[inline]
    pub fn leaf(&self) -> NodeId {
        self.leaf
    }

    /// The leaf's display name from its scene metadata, when present.
    #[inline]
    pub fn leaf_name(&self) -> Option<&str> {
        self.leaf_name.as_deref()
    }

    #[inline]
    pub fn stack(&self) -> &ContourStack {
        &self.stack
    }

    /// Consume the record, keeping only the contour stack.
    #[inline]
    pub fn into_stack(self) -> ContourStack {
        self.stack
    }
}

/// Cooperative cancellation flag for a slicing run. Cheap to clone;
/// clones share one underlying flag, so a caller can hand a clone to a
/// worker thread and trip the original to request a stop. The slicer
/// polls it between layers; cancellation is a clean stop, not an error.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// A fresh token that has not been cancelled.
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation. Idempotent; visible to every clone.
    #[inline]
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Whether cancellation has been requested.
    #[inline]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Progress of a slicing run: the number of layer-slices finished out of
/// the total across every leaf (leaves times the schedule's layer
/// count). Reported once per completed layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SliceProgress {
    pub completed: usize,
    pub total: usize,
}

impl SliceProgress {
    /// Completed fraction in `[0.0, 1.0]`; `0.0` when there is no work.
    #[inline]
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            self.completed as f32 / self.total as f32
        }
    }
}

/// Outcome of a cancellable slicing run: the finished per-leaf stacks,
/// or a clean cancellation. Cancellation carries no partial data: a
/// cancelled run's contours are discarded.
#[derive(Clone, Debug, PartialEq)]
pub enum SliceOutcome {
    Completed(Vec<LeafSlices>),
    Cancelled,
}

/// Slice every mesh leaf reachable from the roots of `scene` into its
/// own [`ContourStack`], all under the single layer schedule carried
/// by `profile`.
///
/// Leaves are discovered by a pre-order walk over `roots()`, entering
/// `Group` containers at any depth. The returned records preserve
/// that discovery order, and every stack has the same layer count and
/// the same per-layer Z values (the schedule is shared), so stacks
/// can be merged layer-by-layer with
/// [`merge_stacks`](crate::contour::merge_stacks). A leaf shorter
/// than the schedule's span simply carries empty layers above its
/// top.
///
/// Geometry validation aggregates: every leaf is baked and validated
/// even after a failure is found, so
/// [`SliceSceneError::LeavesValidationFailed`] lists all invalid
/// leaves of the scene in one round trip. Slicing work stops at the
/// first failure; the other error variants abort immediately.
///
/// # Determinism
///
/// Same guarantee as [`slice_leaf`]: for a fixed `(scene, profile)`
/// and code revision the output is bitwise identical across runs.
///
/// # Errors
///
/// - [`SliceSceneError::EmptyScene`] when no mesh leaf is reachable.
/// - [`SliceSceneError::CsgNotYetSupported`] /
///   [`SliceSceneError::InstanceNotYetSupported`] when discovery
///   reaches one of the deferred node kinds, anywhere in the scene.
/// - [`SliceSceneError::LeavesValidationFailed`] with every invalid
///   leaf when geometry validation rejects at least one of them.
/// - [`SliceSceneError::LeafSlicingFailed`] when baking or stitching
///   one leaf fails.
pub fn slice_scene(
    scene: &Scene,
    profile: &SliceProfile,
) -> Result<Vec<LeafSlices>, SliceSceneError> {
    match slice_scene_core(scene, profile, &CancelToken::new(), &mut |_| {})? {
        SliceOutcome::Completed(slices) => Ok(slices),
        // The token created above is never cancelled, so a completed run
        // is the only reachable outcome here; the arm exists only to
        // satisfy the type (panic-free per this crate's no-panic rule).
        SliceOutcome::Cancelled => Ok(Vec::new()),
    }
}

/// Like [`slice_scene`], but reports per-layer progress and can be
/// cancelled cooperatively through `cancel`.
///
/// `on_progress` is called once per completed layer-slice with the
/// running count and the total (`leaves * schedule layers`).
/// `cancel` is polled between layers and between leaves: once tripped,
/// slicing stops and the call returns [`SliceOutcome::Cancelled`] with
/// no partial contours. Leaf discovery, validation aggregation,
/// determinism, and the error variants are exactly those of
/// [`slice_scene`]; a validation failure still returns
/// [`SliceSceneError::LeavesValidationFailed`] regardless of `cancel`.
pub fn slice_scene_with_progress<F>(
    scene: &Scene,
    profile: &SliceProfile,
    cancel: &CancelToken,
    mut on_progress: F,
) -> Result<SliceOutcome, SliceSceneError>
where
    F: FnMut(SliceProgress),
{
    slice_scene_core(scene, profile, cancel, &mut on_progress)
}

/// The shared slice-scene body for both public entry points: collect
/// leaves, aggregate validation over all of them, then slice each valid
/// leaf under the shared schedule while polling `cancel` and reporting
/// each completed layer through `on_progress`.
fn slice_scene_core<F>(
    scene: &Scene,
    profile: &SliceProfile,
    cancel: &CancelToken,
    on_progress: &mut F,
) -> Result<SliceOutcome, SliceSceneError>
where
    F: FnMut(SliceProgress),
{
    let leaves = collect_mesh_leaves(scene)?;
    let total = leaves
        .len()
        .saturating_mul(profile.layer_schedule().total_layers() as usize);
    let mut completed: usize = 0;

    let mut failures: Vec<LeafValidationFailure> = Vec::new();
    let mut slices: Vec<LeafSlices> = Vec::new();
    for (leaf, leaf_name) in leaves {
        if cancel.is_cancelled() {
            return Ok(SliceOutcome::Cancelled);
        }
        let baked =
            bake_leaf_to_world(scene, leaf).map_err(|err| SliceSceneError::LeafSlicingFailed {
                leaf,
                leaf_name: leaf_name.clone(),
                error: SliceError::BakeFailed(err),
            })?;
        let mesh = match validate_baked(&baked) {
            Ok(mesh) => mesh,
            Err(geometry) => {
                failures.push(LeafValidationFailure {
                    leaf,
                    leaf_name,
                    geometry,
                });
                continue;
            }
        };
        // Once any leaf has failed validation the loop keeps
        // validating (the failure report must be complete) but stops
        // slicing: those contours would be discarded anyway.
        if failures.is_empty() {
            let sliced = slice_manifold_mesh_observed(
                &mesh,
                profile,
                cancel,
                on_progress,
                &mut completed,
                total,
            )
            .map_err(|error| SliceSceneError::LeafSlicingFailed {
                leaf,
                leaf_name: leaf_name.clone(),
                error,
            })?;
            match sliced {
                Some(stack) => slices.push(LeafSlices {
                    leaf,
                    leaf_name,
                    stack,
                }),
                None => return Ok(SliceOutcome::Cancelled),
            }
        }
    }

    if failures.is_empty() {
        Ok(SliceOutcome::Completed(slices))
    } else {
        Err(SliceSceneError::LeavesValidationFailed(failures))
    }
}

/// Collect every mesh leaf reachable from the scene roots, in
/// pre-order, together with its display name: the node's metadata
/// name when present, else the name on the leaf's mesh (where format
/// readers such as OBJ attach object names). Dangling child ids are
/// skipped, matching the scene's own traversal helpers. `Csg` and
/// `Instance` nodes abort the walk: their leaves are operands, not
/// independent solids, and slicing them as such would silently apply
/// union semantics regardless of the operator.
fn collect_mesh_leaves(scene: &Scene) -> Result<Vec<(NodeId, Option<String>)>, SliceSceneError> {
    let mut stack: Vec<NodeId> = scene.roots().iter().rev().copied().collect();
    let mut leaves: Vec<(NodeId, Option<String>)> = Vec::new();
    while let Some(id) = stack.pop() {
        let Some(node) = scene.node(id) else {
            continue;
        };
        match &node.kind {
            NodeKind::Leaf(_) => {
                let name = node.meta.as_ref().and_then(|m| m.name.clone()).or_else(|| {
                    scene
                        .mesh_at_leaf(id)
                        .ok()
                        .and_then(|mesh| mesh.meta.as_ref())
                        .and_then(|meta| meta.name.clone())
                });
                leaves.push((id, name));
            }
            NodeKind::Group { children } => {
                for child in children.iter().rev() {
                    stack.push(*child);
                }
            }
            NodeKind::Csg { .. } => {
                return Err(SliceSceneError::CsgNotYetSupported { node_id: id });
            }
            NodeKind::Instance { .. } => {
                return Err(SliceSceneError::InstanceNotYetSupported { node_id: id });
            }
        }
    }
    if leaves.is_empty() {
        return Err(SliceSceneError::EmptyScene);
    }
    Ok(leaves)
}

/// Run both geometry validation stages on a baked mesh.
fn validate_baked(baked: &IndexedMesh) -> Result<ValidatedManifoldMesh<'_>, InvalidLeafGeometry> {
    let faces = baked
        .validated_faces()
        .map_err(InvalidLeafGeometry::Faces)?;
    faces
        .validated_manifold()
        .map_err(InvalidLeafGeometry::Manifold)
}

/// The shared per-leaf pipeline: intersect every scheduled Z plane
/// with the validated mesh, stitch the crossing segments into rings,
/// canonicalise winding, and group outers with their holes.
fn slice_manifold_mesh(
    mesh: &ValidatedManifoldMesh<'_>,
    profile: &SliceProfile,
) -> Result<ContourStack, SliceError> {
    let topology = MeshTopology::from_manifold(mesh);
    let mut layers = Vec::new();
    for z_mm in profile.layer_schedule().iter_z() {
        layers.push(slice_layer(mesh, z_mm, &topology)?);
    }
    Ok(ContourStack::new_unchecked(layers))
}

/// [`slice_manifold_mesh`] with a cancellation poll and a per-layer
/// progress callback woven into the Z loop. Returns `Ok(None)` when
/// `cancel` is tripped before the leaf finishes (its partial contours
/// are dropped). `completed` accumulates across every leaf of the run
/// so the reported progress spans the whole scene.
fn slice_manifold_mesh_observed<F>(
    mesh: &ValidatedManifoldMesh<'_>,
    profile: &SliceProfile,
    cancel: &CancelToken,
    on_progress: &mut F,
    completed: &mut usize,
    total: usize,
) -> Result<Option<ContourStack>, SliceError>
where
    F: FnMut(SliceProgress),
{
    let topology = MeshTopology::from_manifold(mesh);
    let mut layers = Vec::new();
    for z_mm in profile.layer_schedule().iter_z() {
        if cancel.is_cancelled() {
            return Ok(None);
        }
        layers.push(slice_layer(mesh, z_mm, &topology)?);
        *completed += 1;
        on_progress(SliceProgress {
            completed: *completed,
            total,
        });
    }
    Ok(Some(ContourStack::new_unchecked(layers)))
}

/// Slice one scheduled Z plane of a validated mesh into a
/// [`ContourLayer`]. The single per-layer step shared by the plain and
/// progress-reporting leaf pipelines, so both stay bitwise identical.
fn slice_layer(
    mesh: &ValidatedManifoldMesh<'_>,
    z_mm: f64,
    topology: &MeshTopology,
) -> Result<ContourLayer, SliceError> {
    let segments = segments_at_z(mesh, z_mm, topology);
    let mut rings = stitch_layer(&segments, z_mm, topology)?;
    normalise_to_ccw(&mut rings);
    let polygons = group_into_polygons(rings);
    Ok(ContourLayer::new(z_mm, polygons))
}

#[cfg(test)]
mod api_tests {
    use std::sync::Arc;

    use sc_mesh_core::scene::node::LeafPayload;
    use sc_mesh_core::transform::Transform3D;
    use sc_mesh_core::{IndexedMesh, IndexedTriangle, Normal, Scene, Vertex};

    use crate::profile::{
        AaLevel, BuildVolume, LayerSchedule, Mirror, PackingMode, PixelResolution, SliceProfile,
    };

    use super::*;

    /// Closed unit cube centred at origin (side 1). Manifold by
    /// construction. Inline copy of the fixture in `tests_mesh_slice`
    /// because the smoke test must stay self-contained.
    fn unit_cube_mesh() -> IndexedMesh {
        let vertices = vec![
            Vertex::new([-0.5, -0.5, -0.5]),
            Vertex::new([0.5, -0.5, -0.5]),
            Vertex::new([0.5, 0.5, -0.5]),
            Vertex::new([-0.5, 0.5, -0.5]),
            Vertex::new([-0.5, -0.5, 0.5]),
            Vertex::new([0.5, -0.5, 0.5]),
            Vertex::new([0.5, 0.5, 0.5]),
            Vertex::new([-0.5, 0.5, 0.5]),
        ];
        let nz = Normal::new([0.0, 0.0, -1.0]);
        let pz = Normal::new([0.0, 0.0, 1.0]);
        let ny = Normal::new([0.0, -1.0, 0.0]);
        let py = Normal::new([0.0, 1.0, 0.0]);
        let nx = Normal::new([-1.0, 0.0, 0.0]);
        let px = Normal::new([1.0, 0.0, 0.0]);
        let f = |n: Normal, abc: [u32; 3]| IndexedTriangle::from_raw_indices(n, abc);
        IndexedMesh {
            meta: None,
            vertices,
            faces: vec![
                f(nz, [0, 3, 2]),
                f(nz, [0, 2, 1]),
                f(pz, [4, 5, 6]),
                f(pz, [4, 6, 7]),
                f(py, [3, 7, 6]),
                f(py, [3, 6, 2]),
                f(ny, [0, 1, 5]),
                f(ny, [0, 5, 4]),
                f(px, [1, 2, 6]),
                f(px, [1, 6, 5]),
                f(nx, [0, 4, 7]),
                f(nx, [0, 7, 3]),
            ],
        }
    }

    fn slice_profile_for_cube() -> SliceProfile {
        SliceProfile::new(
            BuildVolume::new(2.0, 2.0, 2.0).unwrap(),
            PixelResolution::new(100, 100).unwrap(),
            LayerSchedule::uniform(0.1, 0.1, 4).unwrap(),
            Mirror::default(),
            AaLevel::Off,
            PackingMode::None,
        )
    }

    fn scene_with_cube_leaf() -> (Scene, NodeId) {
        let mut scene = Scene::new();
        let mesh = Arc::new(unit_cube_mesh());
        let leaf_id = scene
            .add_leaf(None, Transform3D::Identity, LeafPayload::Mesh(mesh))
            .expect("add_leaf accepts inline mesh");
        scene
            .add_root(leaf_id)
            .expect("add_root accepts a freshly added leaf");
        (scene, leaf_id)
    }

    #[test]
    fn slice_leaf_on_a_cube_returns_a_non_empty_contour_stack() {
        let (scene, leaf_id) = scene_with_cube_leaf();
        let profile = slice_profile_for_cube();
        let stack = slice_leaf(&scene, leaf_id, &profile).expect("cube slices cleanly");
        assert_eq!(stack.layers().len(), 4);
        assert!(
            stack
                .layers()
                .iter()
                .all(|layer| !layer.polygons().is_empty()),
            "every centred Z falls inside the cube, so every layer must carry a polygon",
        );
    }

    #[test]
    fn slice_leaf_is_deterministic_across_runs() {
        let (scene, leaf_id) = scene_with_cube_leaf();
        let profile = slice_profile_for_cube();
        let a = slice_leaf(&scene, leaf_id, &profile).unwrap();
        let b = slice_leaf(&scene, leaf_id, &profile).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn slice_leaf_rejects_a_group_node() {
        let (mut scene, _) = scene_with_cube_leaf();
        let group_id = scene.add_group(None, Transform3D::Identity);
        let profile = slice_profile_for_cube();
        let err = slice_leaf(&scene, group_id, &profile).unwrap_err();
        assert!(
            matches!(err, SliceError::NotALeaf { node_id } if node_id == group_id),
            "expected NotALeaf, got {err:?}",
        );
    }
}
