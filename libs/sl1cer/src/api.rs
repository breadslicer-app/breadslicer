/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Public slicer entry points.
//!
//! [`slice_leaf`] turns 1 baked, edge-manifold leaf into a vector
//! [`ContourStack`]; [`slice_scene`] applies the same pipeline to every mesh
//! leaf reachable from the scene roots and returns 1 stack per leaf. Both
//! compose the per-Z slice, stitch, winding canonicalisation, and outer and
//! hole grouping steps from the rest of the crate into 1 end-to-end pipeline.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rayon::prelude::*;
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::indexed_mesh::{MeshTopology, ValidatedEdgeManifoldMesh};
use sc_scene::bake::bake_leaf_to_world;
use sc_scene::node::{NodeId, NodeKind};
use sc_scene::{CsgPolicy, ReferencePolicy, Scene, SceneError};

use crate::contour::{ClosedRing, ContourLayer, ContourStack, Polygon};
use crate::error::{InvalidLeafGeometry, LeafValidationFailure, SliceError, SliceSceneError};
use crate::grouping::{group_by_facing, point_in_ring};
use crate::mesh_slice::segments_at_z_for_faces;
use crate::profile::SliceProfile;
use crate::shells::FaceShells;
use crate::stitch::{normalise_to_ccw, stitch_layer_with_faces};
use crate::z_bins::LayerBuckets;

/// Slices the leaf identified by `leaf` in `scene` into a [`ContourStack`],
/// using the per-layer schedule and the other parameters carried by `profile`.
///
/// The leaf is baked to world space, validated for vertex finiteness,
/// face-index integrity and edge-manifoldness, then each scheduled Z plane is
/// intersected with the triangles of the mesh. The resulting crossing segments
/// are stitched into closed rings, canonicalised to CCW, and grouped into
/// outer-plus-holes polygons.
///
/// The returned stack has strictly monotone Z by construction: the schedule
/// yields Z values in strict-monotone order and each is emitted at most once.
///
/// # Determinism
///
/// For a fixed `(scene, leaf, profile)` and a fixed code revision, the output
/// is bitwise identical across runs. No iteration order in the implementation
/// depends on the hash-map traversal order of values the user can observe.
///
/// # Errors
///
/// - [`SliceError::NodeNotFound`] if `leaf` does not resolve.
/// - [`SliceError::NotALeaf`] if it resolves to a [`NodeKind::Group`].
/// - [`SliceError::CsgNotYetSupported`] or
///   [`SliceError::SubtreeReferenceNotYetSupported`] for the deferred kinds.
/// - [`SliceError::BakeFailed`] when [`bake_leaf_to_world`] fails.
/// - [`SliceError::VerticesValidationFailed`],
///   [`SliceError::FacesValidationFailed`] or
///   [`SliceError::ManifoldCheckFailed`] when the baked mesh fails validation.
/// - [`SliceError::NonManifoldDuringStitching`] if the segment list of a layer
///   has dangling endpoints (a runtime safety net for input that got past the
///   manifold check).
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
        NodeKind::Reference { .. } => {
            return Err(SliceError::SubtreeReferenceNotYetSupported { node_id: leaf });
        }
    }

    let baked = bake_leaf_to_world(scene, leaf)?;
    let mesh = validate_baked(&baked).map_err(SliceError::from)?;
    slice_manifold_mesh(&mesh, profile)
}

/// Contours produced for 1 leaf of a scene by [`slice_scene`].
#[derive(Clone, Debug, PartialEq)]
pub struct LeafSlices {
    leaf: NodeId,
    leaf_name: Option<String>,
    stack: ContourStack,
}

impl LeafSlices {
    /// Returns the leaf node these contours belong to.
    #[inline]
    pub fn leaf(&self) -> NodeId {
        self.leaf
    }

    /// Returns the display name of the leaf, when present.
    #[inline]
    pub fn leaf_name(&self) -> Option<&str> {
        self.leaf_name.as_deref()
    }

    #[inline]
    pub fn stack(&self) -> &ContourStack {
        &self.stack
    }

    /// Consumes the record and keeps only the contour stack.
    #[inline]
    pub fn into_stack(self) -> ContourStack {
        self.stack
    }
}

/// Cooperative cancellation flag for a slicing run. Cheap to clone; clones
/// share 1 underlying flag, so a caller can hand a clone to a worker thread and
/// trip the original to request a stop. The slicer polls it between layers;
/// cancellation is a clean stop, not an error.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// Returns a fresh token that has not been cancelled.
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation. Idempotent; visible to every clone.
    #[inline]
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Returns whether cancellation has been requested.
    #[inline]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Progress of a slicing run: the number of layer-slices finished out of the
/// total across every leaf (leaves times the layer count of the schedule).
/// Reported once per completed layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SliceProgress {
    pub completed: usize,
    pub total: usize,
}

impl SliceProgress {
    /// Returns the completed fraction in `[0.0, 1.0]`; `0.0` when there is no
    /// work.
    #[inline]
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            self.completed as f32 / self.total as f32
        }
    }
}

/// Outcome of a cancellable slicing run: the finished per-leaf stacks, or a
/// clean cancellation. Cancellation carries no partial data: the contours of a
/// cancelled run are discarded.
#[derive(Clone, Debug, PartialEq)]
pub enum SliceOutcome {
    Completed(Vec<LeafSlices>),
    Cancelled,
}

/// Slices every mesh leaf reachable from the roots of `scene` into its own
/// [`ContourStack`], all under the single layer schedule carried by `profile`.
///
/// Leaves are discovered by a pre-order walk over `roots()`, entering `Group`
/// containers at any depth. The returned records keep that discovery order, and
/// every stack has the same layer count and the same per-layer Z values (the
/// schedule is shared), so stacks can be merged layer by layer with
/// [`merge_stacks`](crate::contour::merge_stacks). A leaf shorter than the span
/// of the schedule carries empty layers above its top.
///
/// Geometry validation aggregates: every leaf is baked and validated even after
/// a failure is found, so [`SliceSceneError::LeavesValidationFailed`] lists all
/// invalid leaves of the scene in 1 round trip. Slicing work stops at the first
/// failure; the other error variants abort immediately.
///
/// # Determinism
///
/// Same guarantee as [`slice_leaf`]: for a fixed `(scene, profile)` and code
/// revision the output is bitwise identical across runs.
///
/// # Errors
///
/// - [`SliceSceneError::EmptyScene`] when no mesh leaf is reachable.
/// - [`SliceSceneError::CsgNotYetSupported`] or
///   [`SliceSceneError::SubtreeReferenceNotYetSupported`] when discovery
///   reaches a deferred node kind, anywhere in the scene.
/// - [`SliceSceneError::LeavesValidationFailed`] with every invalid leaf when
///   geometry validation rejects at least 1 of them.
/// - [`SliceSceneError::LeafSlicingFailed`] when baking or stitching a leaf
///   fails.
pub fn slice_scene(
    scene: &Scene,
    profile: &SliceProfile,
) -> Result<Vec<LeafSlices>, SliceSceneError> {
    match slice_scene_core(scene, profile, &CancelToken::new(), &|_| {})? {
        SliceOutcome::Completed(slices) => Ok(slices),
        // The token created above is never cancelled, so a completed run is the
        // only reachable outcome here; the arm exists only to satisfy the type
        // (panic-free per the no-panic rule of this crate).
        SliceOutcome::Cancelled => Ok(Vec::new()),
    }
}

/// Slices as [`slice_scene`] does, but reports per-layer progress and can be
/// cancelled cooperatively through `cancel`.
///
/// The layers of each leaf are sliced in parallel and the results are assembled
/// in schedule order, so parallelism never affects the output bytes.
/// `on_progress` is called once per completed layer-slice with the running
/// count and the total (`leaves * schedule layers`); it is invoked from worker
/// threads (hence the `Fn + Sync` bound) and reports can reach the observer out
/// of order. Over a completed run the reported counts are a permutation of
/// `1..=total`.
///
/// `cancel` is polled when each layer task starts and between leaves: once
/// tripped, no further layer starts; layers already in flight finish (and may
/// still report progress) and the call returns [`SliceOutcome::Cancelled`] with
/// no partial contours. Leaf discovery, validation aggregation, determinism,
/// and the error variants are exactly those of [`slice_scene`]; a validation
/// failure still returns [`SliceSceneError::LeavesValidationFailed`] regardless
/// of `cancel`.
pub fn slice_scene_with_progress<F>(
    scene: &Scene,
    profile: &SliceProfile,
    cancel: &CancelToken,
    on_progress: F,
) -> Result<SliceOutcome, SliceSceneError>
where
    F: Fn(SliceProgress) + Sync,
{
    slice_scene_core(scene, profile, cancel, &on_progress)
}

/// Runs the slice-scene body that both public entry points share: collects the
/// leaves, aggregates validation over all of them, then slices each valid leaf
/// under the shared schedule while it polls `cancel` and reports each completed
/// layer through `on_progress`.
fn slice_scene_core<F>(
    scene: &Scene,
    profile: &SliceProfile,
    cancel: &CancelToken,
    on_progress: &F,
) -> Result<SliceOutcome, SliceSceneError>
where
    F: Fn(SliceProgress) + Sync,
{
    let leaves = collect_mesh_leaves(scene)?;
    let total = leaves
        .len()
        .saturating_mul(profile.layer_schedule().total_layers() as usize);
    let completed = AtomicUsize::new(0);

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
        // Once any leaf has failed validation the loop keeps validating (the
        // failure report must be complete) but stops slicing: those contours
        // would be discarded anyway.
        if failures.is_empty() {
            let sliced = slice_manifold_mesh_observed(
                &mesh,
                profile,
                cancel,
                on_progress,
                &completed,
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

/// Lists the mesh leaves reachable from the scene roots, in the pre-order the
/// slicer visits them. [`slice_scene`] and any caller that must reason about
/// the same leaf set without slicing share this walk, so they cannot disagree
/// on which leaves a scene slices. The walk is [`Scene::reachable_leaves`] with
/// both policies at `Refuse`: `Csg` and `Reference` nodes abort it, since their
/// leaves are operands, not independent solids, and slicing them as such would
/// silently apply union semantics regardless of the operator. An empty result
/// is reported as [`SliceSceneError::EmptyScene`].
pub fn sliceable_leaves(scene: &Scene) -> Result<Vec<NodeId>, SliceSceneError> {
    let leaves = scene
        .reachable_leaves(CsgPolicy::Refuse, ReferencePolicy::Refuse)
        .map_err(|err| match err {
            SceneError::UnevaluatedCsg { node_id } => {
                SliceSceneError::CsgNotYetSupported { node_id }
            }
            SceneError::UnexpandedReference { node_id } => {
                SliceSceneError::SubtreeReferenceNotYetSupported { node_id }
            }
            other => unreachable!(
                "a (Refuse, Refuse) structural walk errors only on Csg and Reference nodes: {other}"
            ),
        })?;
    if leaves.is_empty() {
        return Err(SliceSceneError::EmptyScene);
    }
    Ok(leaves)
}

/// Returns the sliceable leaves, each with its display name, in the same order
/// as [`sliceable_leaves`].
fn collect_mesh_leaves(scene: &Scene) -> Result<Vec<(NodeId, Option<String>)>, SliceSceneError> {
    let leaves = sliceable_leaves(scene)?;
    Ok(leaves
        .into_iter()
        .map(|id| (id, leaf_display_name(scene, id)))
        .collect())
}

/// Returns the display name of a leaf: the metadata name of the node when
/// present, else the name on the mesh of the leaf (where format readers such as
/// OBJ attach object names).
fn leaf_display_name(scene: &Scene, id: NodeId) -> Option<String> {
    scene
        .node(id)
        .and_then(|node| node.meta.as_ref())
        .and_then(|m| m.name.clone())
        .or_else(|| {
            scene
                .mesh_at_leaf(id)
                .ok()
                .and_then(|mesh| mesh.meta.as_ref())
                .and_then(|meta| meta.name.clone())
        })
}

/// Runs the 3 geometry validation stages on a baked mesh, in order: vertices
/// (NaN or infinite coordinates), faces (index range, collapsed edges),
/// edge-manifoldness. Each stage short-circuits, so a leaf reports the errors
/// of exactly 1 stage.
///
/// The vertex stage runs on the baked vertices on purpose: the f64 transform
/// chain downcasts to f32 per vertex, and a finite transform can saturate
/// coordinates to infinity, so validating the source mesh alone would miss
/// overflow introduced by the bake.
fn validate_baked(
    baked: &IndexedMesh,
) -> Result<ValidatedEdgeManifoldMesh<'_>, InvalidLeafGeometry> {
    baked
        .validate_vertices()
        .map_err(InvalidLeafGeometry::Vertices)?;
    let faces = baked
        .validated_faces()
        .map_err(InvalidLeafGeometry::Faces)?;
    faces
        .validated_edge_manifold()
        .map_err(InvalidLeafGeometry::Manifold)
}

/// Runs the shared per-leaf pipeline: intersects every scheduled Z plane with
/// the validated mesh, stitches the crossing segments into rings, canonicalises
/// the winding, and groups outers with their holes. Wraps
/// [`slice_manifold_mesh_observed`] with a token that is never cancelled and a
/// no-op progress callback.
fn slice_manifold_mesh(
    mesh: &ValidatedEdgeManifoldMesh<'_>,
    profile: &SliceProfile,
) -> Result<ContourStack, SliceError> {
    let total = profile.layer_schedule().total_layers() as usize;
    let sliced = slice_manifold_mesh_observed(
        mesh,
        profile,
        &CancelToken::new(),
        &|_| {},
        &AtomicUsize::new(0),
        total,
    )?;
    match sliced {
        Some(stack) => Ok(stack),
        // The token created above is never cancelled, so a finished stack is
        // the only reachable outcome here; the arm exists only to satisfy the
        // type (panic-free per the no-panic rule of this crate).
        None => Ok(ContourStack::new_unchecked(Vec::new())),
    }
}

/// Runs [`slice_manifold_mesh`] with a cancellation poll and a per-layer
/// progress callback. Layers are sliced in parallel, 1 task per scheduled Z;
/// the finished layers are assembled in schedule order, so the stack is bitwise
/// identical to a sequential pass. When more than 1 layer fails, the error of
/// the lowest failing layer is returned, matching the first failure a
/// sequential pass would hit.
///
/// `cancel` is polled when each layer task starts. Returns `Ok(None)` when the
/// poll saw a tripped token before every layer was computed (the finished
/// layers are dropped); tasks already past the poll still complete and report
/// progress. `completed` accumulates across every leaf of the run, so the
/// reported progress spans the whole scene.
fn slice_manifold_mesh_observed<F>(
    mesh: &ValidatedEdgeManifoldMesh<'_>,
    profile: &SliceProfile,
    cancel: &CancelToken,
    on_progress: &F,
    completed: &AtomicUsize,
    total: usize,
) -> Result<Option<ContourStack>, SliceError>
where
    F: Fn(SliceProgress) + Sync,
{
    let topology = MeshTopology::from_edge_manifold(mesh);
    let shells = FaceShells::build(mesh);
    let zs: Vec<f64> = profile.layer_schedule().iter_z().collect();
    let buckets = LayerBuckets::build(mesh, &zs);
    // `Ok(None)` marks a layer skipped by the cancellation poll. The collect is
    // positional, so `outcomes` is in schedule order.
    let outcomes: Vec<Result<Option<ContourLayer>, SliceError>> = zs
        .par_iter()
        .enumerate()
        .map(|(layer_idx, &z_mm)| {
            if cancel.is_cancelled() {
                return Ok(None);
            }
            let layer = slice_layer(mesh, z_mm, &topology, &shells, buckets.faces_at(layer_idx))?;
            let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
            on_progress(SliceProgress {
                completed: done,
                total,
            });
            Ok(Some(layer))
        })
        .collect();

    let mut layers = Vec::with_capacity(outcomes.len());
    for outcome in outcomes {
        match outcome? {
            Some(layer) => layers.push(layer),
            None => return Ok(None),
        }
    }
    Ok(Some(ContourStack::new_unchecked(layers)))
}

/// Slices 1 scheduled Z plane of a validated mesh into a [`ContourLayer`],
/// testing only the faces listed in `candidates` (ascending; the production
/// pipeline passes the Z-bucket of the plane, the test reference passes every
/// face). Pure function of its arguments: the layer tasks run in parallel and
/// rely on this producing the same bytes no matter which thread or order runs
/// them.
///
/// Each ring is an outer or a hole by its facing: the side its faces put the
/// solid on (see [`crate::grouping`]), so rings of one shell that overlap,
/// as where a posed mesh runs through itself, both stay solid and the
/// non-zero raster unites them. A void shell (a sealed cavity, facing inward)
/// counts its facing reversed, as do the rings of a mesh turned inside out,
/// which still slices solid.
///
/// Outers and holes are grouped per shell (`shells`), in order of each
/// shell's first ring, so shells that overlap do not read as holes of each
/// other. A void shell is grouped the same way and then subtracted: each of
/// its outers becomes a hole of the solid polygon that encloses it, and each
/// of its holes (solid inside the void) a polygon of its own. A hole of a
/// solid shell that no outer of its shell contains is subtracted the same
/// way, and a solid ring of a void shell that none of its outers contains is
/// a polygon of its own.
pub(crate) fn slice_layer(
    mesh: &ValidatedEdgeManifoldMesh<'_>,
    z_mm: f64,
    topology: &MeshTopology,
    shells: &FaceShells,
    candidates: &[u32],
) -> Result<ContourLayer, SliceError> {
    let segments = segments_at_z_for_faces(mesh, z_mm, topology, candidates);
    let vertices = mesh.vertices();
    let faces = mesh.faces();
    // How far a segment runs with its face's solid on its left: the face
    // normal's XY part against the segment's right-hand side.
    let facing = |i: usize| {
        let seg = &segments[i];
        let [p0, p1, p2] = faces[seg.source_face.get()]
            .vertices
            .map(|v| vertices[v.get() as usize].coords());
        let e1 = [0, 1, 2].map(|k| f64::from(p1[k]) - f64::from(p0[k]));
        let e2 = [0, 1, 2].map(|k| f64::from(p2[k]) - f64::from(p0[k]));
        let nx = e1[1] * e2[2] - e1[2] * e2[1];
        let ny = e1[2] * e2[0] - e1[0] * e2[2];
        nx * (seg.b.y - seg.a.y) - ny * (seg.b.x - seg.a.x)
    };
    let tagged = stitch_layer_with_faces(&segments, z_mm, topology, facing)?;
    let mut by_shell: Vec<(u32, Vec<ClosedRing>, Vec<f64>)> = Vec::new();
    for stitched in tagged {
        let shell = if shells.count() > 1 {
            shells.of(stitched.face)
        } else {
            0
        };
        // Measured along the ring's vertex order, which `normalise_to_ccw`
        // reverses when the ring runs clockwise.
        let mut facing = if stitched.ring.signed_area() < 0.0 {
            -stitched.facing
        } else {
            stitched.facing
        };
        if shells.is_void(shell) {
            facing = -facing;
        }
        match by_shell.iter_mut().find(|(s, _, _)| *s == shell) {
            Some((_, rings, facings)) => {
                rings.push(stitched.ring);
                facings.push(facing);
            }
            None => by_shell.push((shell, vec![stitched.ring], vec![facing])),
        }
    }
    let mut polygons = Vec::new();
    let mut voids = Vec::new();
    let mut loose = Vec::new();
    for (shell, mut rings, facing) in by_shell {
        normalise_to_ccw(&mut rings);
        let (grouped, orphans) = group_by_facing(rings, &facing);
        let orphans = orphans
            .into_iter()
            .map(|ring| Polygon::new_unchecked(ring, Vec::new()));
        if shells.count() > 1 && shells.is_void(shell) {
            voids.extend(grouped);
            loose.extend(orphans);
        } else {
            polygons.extend(grouped);
            voids.extend(orphans);
        }
    }
    if !voids.is_empty() {
        polygons = subtract_voids(polygons, voids);
    }
    polygons.extend(loose);
    Ok(ContourLayer::new(z_mm, polygons))
}

/// Cuts void polygons out of solid ones: a void's outer becomes a hole of the
/// smallest solid polygon whose outer encloses it (and outside its holes);
/// the void's own holes are solid islands and become polygons. A void that
/// no solid encloses is not a cavity but a part with inverted faces (a bad
/// export); it stays solid, as it sliced before voids were recognised.
fn subtract_voids(mut solids: Vec<Polygon>, voids: Vec<Polygon>) -> Vec<Polygon> {
    let mut islands = Vec::new();
    for void in voids {
        let (mut outer, holes) = (void.outer().clone(), void.holes().to_vec());
        let rep = outer.vertices()[0];
        let host = solids
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                point_in_ring(rep, s.outer()) && !s.holes().iter().any(|h| point_in_ring(rep, h))
            })
            .min_by(|(_, a), (_, b)| {
                a.outer()
                    .signed_area()
                    .abs()
                    .total_cmp(&b.outer().signed_area().abs())
            })
            .map(|(i, _)| i);
        let Some(i) = host else {
            islands.push(void);
            continue;
        };
        outer.reverse();
        let s = &solids[i];
        let mut cut = s.holes().to_vec();
        cut.push(outer);
        solids[i] = Polygon::new_unchecked(s.outer().clone(), cut);
        for mut hole in holes {
            hole.reverse();
            islands.push(Polygon::new_unchecked(hole, Vec::new()));
        }
    }
    solids.extend(islands);
    solids
}

#[cfg(test)]
mod api_tests {
    use std::sync::Arc;

    use sc_mesh_core::transform::Transform3D;
    use sc_mesh_core::{IndexedMesh, IndexedTriangle, Vertex};
    use sc_scene::Scene;
    use sc_scene::node::LeafPayload;

    use crate::profile::{
        AaLevel, BuildVolume, LayerSchedule, Mirror, PackingMode, PixelResolution, SliceProfile,
    };

    use super::*;

    /// Returns a closed unit cube centred at the origin (side 1). Manifold by
    /// construction. Inline copy of the fixture in `tests_mesh_slice`, because
    /// the smoke test must stay self-contained.
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
        let f = IndexedTriangle::from_raw_indices;
        IndexedMesh {
            meta: None,
            vertices,
            faces: vec![
                f([0, 3, 2]),
                f([0, 2, 1]),
                f([4, 5, 6]),
                f([4, 6, 7]),
                f([3, 7, 6]),
                f([3, 6, 2]),
                f([0, 1, 5]),
                f([0, 5, 4]),
                f([1, 2, 6]),
                f([1, 6, 5]),
                f([0, 4, 7]),
                f([0, 7, 3]),
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
