/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Public slicer entry point.
//!
//! [`slice_leaf`] is the single function the rest of the workspace
//! calls to turn a baked, manifold leaf into a vector
//! [`ContourStack`]. It composes the per-Z slice, stitch, winding
//! canonicalisation, and outer/hole grouping steps from the rest of
//! the crate into one end-to-end pipeline.

use sc_mesh_core::Scene;
use sc_mesh_core::indexed_mesh::MeshTopology;
use sc_mesh_core::scene::bake::bake_leaf_to_world;
use sc_mesh_core::scene::node::{NodeId, NodeKind};

use crate::contour::{ContourLayer, ContourStack};
use crate::error::SliceError;
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
    let faces = baked
        .validated_faces()
        .map_err(SliceError::FacesValidationFailed)?;
    let mesh = faces
        .validated_manifold()
        .map_err(SliceError::ManifoldCheckFailed)?;

    let topology = MeshTopology::from_manifold(&mesh);
    let mut layers = Vec::new();
    for z_mm in profile.layer_schedule().iter_z() {
        let segments = segments_at_z(&mesh, z_mm, &topology);
        let mut rings = stitch_layer(&segments, z_mm, &topology)?;
        normalise_to_ccw(&mut rings);
        let polygons = group_into_polygons(rings);
        layers.push(ContourLayer::new(z_mm, polygons));
    }
    Ok(ContourStack::new_unchecked(layers))
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
