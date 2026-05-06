/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Typed scene mutation vocabulary.
//!
//! Every observable change to a [`Scene`] is one variant of
//! [`SceneEdit`]. Headless callers apply edits directly with
//! [`SceneEdit::apply_to_scene`]; cache layers (notably
//! `sc_mesh_spatial::SceneSpatial`) dispatch the same enum and add
//! invalidation hooks for the cached state that the variant affects.

use crate::scene::{Scene, Transform3D};

/// Typed scene mutation. The shared edit shape across the workspace:
/// the GUI's undo/redo stack, the headless `apply_to_scene` path, and
/// any cache layer's `apply_edit` all speak this enum.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SceneEdit {
    /// `objects[object_idx].transform` was changed from `before` to
    /// `after`. Applying writes `after`; `inverse()` produces the
    /// undo edit.
    ObjectTransform {
        object_idx: usize,
        before: Transform3D,
        after: Transform3D,
    },
}

impl SceneEdit {
    /// True when applying this edit would not change the scene.
    /// Useful for filtering no-op gestures from the undo stack.
    pub fn is_noop(&self) -> bool {
        match self {
            Self::ObjectTransform { before, after, .. } => before == after,
        }
    }

    /// Inverse edit: applying `self` then `self.inverse()` returns
    /// the scene to its pre-edit state.
    pub fn inverse(&self) -> Self {
        match self {
            Self::ObjectTransform {
                object_idx,
                before,
                after,
            } => Self::ObjectTransform {
                object_idx: *object_idx,
                before: *after,
                after: *before,
            },
        }
    }

    /// Apply this edit directly to a [`Scene`]. Cache-aware callers
    /// should go through `sc_mesh_spatial::SceneSpatial::apply_edit`
    /// instead, which dispatches the same enum and updates spatial
    /// caches before delegating here.
    pub fn apply_to_scene(&self, scene: &mut Scene) {
        match self {
            Self::ObjectTransform {
                object_idx, after, ..
            } => {
                if let Some(obj) = scene.objects.get_mut(*object_idx) {
                    obj.transform = *after;
                }
            }
        }
    }
}

#[cfg(test)]
mod scene_edit_tests {
    use super::*;
    use crate::indexed_mesh::IndexedMesh;
    use crate::scene::{SceneObject, SceneObjectContent};
    use crate::transform::AffineTransform3D;

    fn empty_mesh_object(transform: Transform3D) -> SceneObject {
        SceneObject {
            meta: None,
            transform,
            content: SceneObjectContent::Mesh(IndexedMesh::new(None, vec![], vec![])),
        }
    }

    #[test]
    fn is_noop_detects_identical_before_after() {
        let edit = SceneEdit::ObjectTransform {
            object_idx: 0,
            before: Transform3D::Identity,
            after: Transform3D::Identity,
        };
        assert!(edit.is_noop());
    }

    #[test]
    fn is_noop_false_when_transforms_differ() {
        let edit = SceneEdit::ObjectTransform {
            object_idx: 0,
            before: Transform3D::Identity,
            after: Transform3D::Affine(AffineTransform3D::from_translation([1.0, 0.0, 0.0])),
        };
        assert!(!edit.is_noop());
    }

    #[test]
    fn inverse_swaps_before_and_after() {
        let after = Transform3D::Affine(AffineTransform3D::from_translation([1.0, 2.0, 3.0]));
        let edit = SceneEdit::ObjectTransform {
            object_idx: 7,
            before: Transform3D::Identity,
            after,
        };
        let inv = edit.inverse();
        assert_eq!(
            inv,
            SceneEdit::ObjectTransform {
                object_idx: 7,
                before: after,
                after: Transform3D::Identity,
            }
        );
    }

    #[test]
    fn apply_object_transform_writes_after_to_scene() {
        let after = Transform3D::Affine(AffineTransform3D::from_translation([1.0, 2.0, 3.0]));
        let mut scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![empty_mesh_object(Transform3D::Identity)],
        };
        SceneEdit::ObjectTransform {
            object_idx: 0,
            before: Transform3D::Identity,
            after,
        }
        .apply_to_scene(&mut scene);
        assert_eq!(scene.objects[0].transform, after);
    }

    #[test]
    fn apply_then_inverse_round_trips_scene() {
        let before = Transform3D::Identity;
        let after = Transform3D::Affine(AffineTransform3D::from_translation([1.0, 2.0, 3.0]));
        let mut scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![empty_mesh_object(before)],
        };
        let edit = SceneEdit::ObjectTransform {
            object_idx: 0,
            before,
            after,
        };
        edit.apply_to_scene(&mut scene);
        edit.inverse().apply_to_scene(&mut scene);
        assert_eq!(scene.objects[0].transform, before);
    }
}
