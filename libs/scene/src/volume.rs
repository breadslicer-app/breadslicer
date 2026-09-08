/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Enclosed-volume queries for [`Scene`].

use crate::Scene;
use crate::error::SceneError;
use crate::node::{LeafPayload, NodeId, NodeKind};
use sc_mesh_core::transform::Transform3D;

impl Scene {
    /// Sum the volume every scene root encloses in world space, in
    /// cubic millimetres. An empty scene encloses nothing and returns
    /// `0.0`.
    ///
    /// `Group` contributes the sum of its children. `Csg` does the
    /// same, which over-reports every operator: overlapping `Union`
    /// members are counted twice, and `Difference` and `Intersection`
    /// ignore the operator entirely. That matches the conservative
    /// stance [`Self::aabb`] takes, and holds until CSG is evaluated
    /// rather than carried as a placeholder. `Reference` expands the
    /// referenced subtree, so an instanced subtree counts once per
    /// occurrence, which is what it costs in resin.
    ///
    /// A leaf contributes the magnitude of its enclosed volume scaled
    /// by the determinant of its composed transform, both taken
    /// absolute: neither an inward-wound mesh nor a mirroring
    /// transform makes material disappear, and leaves must not cancel
    /// each other out.
    ///
    /// Only a closed mesh encloses anything, so an open one
    /// contributes whatever its boundary leaves unbalanced.
    ///
    /// # Errors
    ///
    /// Propagates dangling-id errors, and reports
    /// [`SceneError::InvalidMeshFaces`] for a leaf whose mesh cannot
    /// be indexed by face.
    pub fn world_volume_mm3(&self) -> Result<f64, SceneError> {
        let mut total = 0.0;
        for &root in self.roots() {
            self.fold_world_volume(root, Transform3D::Identity, &mut total)?;
        }
        Ok(total)
    }

    fn fold_world_volume(
        &self,
        node_id: NodeId,
        parent_world: Transform3D,
        accum: &mut f64,
    ) -> Result<(), SceneError> {
        let node = self
            .node(node_id)
            .ok_or(SceneError::DanglingNodeId { id: node_id })?;
        let world_from_self = parent_world.compose(&node.local_transform);
        match &node.kind {
            NodeKind::Group { children } | NodeKind::Csg { children, .. } => {
                for &c in children {
                    self.fold_world_volume(c, world_from_self, accum)?;
                }
            }
            NodeKind::Leaf(payload) => {
                let mesh = match payload {
                    LeafPayload::Mesh(m) => m.as_ref(),
                    LeafPayload::MeshResource(rid) => self
                        .mesh_resource(*rid)
                        .ok_or(SceneError::DanglingMeshResource { id: *rid })?,
                };
                let local = mesh
                    .validated_faces()
                    .map_err(|errors| SceneError::InvalidMeshFaces { node_id, errors })?
                    .signed_volume_mm3();
                // An affine map scales volume by its determinant.
                let scale = match &world_from_self {
                    Transform3D::Identity => 1.0,
                    Transform3D::Affine(a) => a.determinant().abs(),
                };
                *accum += local.abs() * scale;
            }
            NodeKind::Reference { resource } => {
                let root = self
                    .subtree_resource_root(*resource)
                    .ok_or(SceneError::DanglingSubtreeResource { id: *resource })?;
                self.fold_world_volume(root, world_from_self, accum)?;
            }
        }
        Ok(())
    }
}
