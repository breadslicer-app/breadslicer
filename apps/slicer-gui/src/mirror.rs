/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Mirror (#648): the selected models flipped left to right (X) or front to
//! back (Y), together, about the middle of the box they take, so they stay
//! where they stand.
//!
//! Each mesh is mirrored, not its transform: a transform that mirrors would
//! turn the model inside out wherever a face's side is read from its
//! winding (the view culls back faces, supports and hollows read the
//! model's frame). The new mesh is the old one under `W⁻¹ · M · W` (`W` the
//! leaf's world transform, `M` the reflection), baked with its winding
//! reversed, and the transform stays as it was. Imported supports are
//! mirrored with their model; Breadslicer's own supports of a mirrored model
//! go, and new ones are made for it in the same Undo step (#153).

use std::sync::Arc;

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_scene::NodeId;

use crate::scene_document::{SceneDocument, SceneEdit, leaf_mesh_swap};

/// An axis a selection is mirrored along: what the reflection turns over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MirrorAxis {
    /// Left to right: X turns over.
    X,
    /// Front to back: Y turns over.
    Y,
}

impl MirrorAxis {
    fn index(self) -> usize {
        match self {
            MirrorAxis::X => 0,
            MirrorAxis::Y => 1,
        }
    }
}

/// The reflection across the plane `axis = at`, in the world.
fn reflection(axis: MirrorAxis, at: f64) -> Transform3D {
    let mut linear = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let mut translation = [0.0; 3];
    let i = axis.index();
    linear[i][i] = -1.0;
    translation[i] = 2.0 * at;
    Transform3D::Affine(AffineTransform3D::from_linear_and_translation(
        linear,
        translation,
    ))
}

/// Mirrors the models `models` of `document` along `axis`, about the middle
/// of the box they take together, as one Undo step. Supports in `models`
/// are left out. Whether anything was mirrored.
pub(crate) fn mirror(document: &SceneDocument, models: &[NodeId], axis: MirrorAxis) -> bool {
    let sessions = document.support_sessions();
    let built = document.with_scene(|spatial| {
        let graph = spatial.graph();
        let models: Vec<NodeId> = models
            .iter()
            .copied()
            .filter(|&id| {
                spatial
                    .leaf(id)
                    .is_some_and(|q| !sc_geom::supports::is_any_support_mesh(q.mesh()))
            })
            .collect();
        if models.is_empty() {
            return None;
        }
        // Imported supports turn over with their model; Breadslicer's own go
        // and are made again.
        let mut turned = models.clone();
        let mut gone = Vec::new();
        for &model in &models {
            for set in crate::support_persist::sets_of_model(spatial, &sessions, model) {
                if !gone.contains(&set) {
                    gone.push(set);
                }
            }
            for imported in crate::imported_supports::imported_of(spatial, model, None) {
                if !turned.contains(&imported) {
                    turned.push(imported);
                }
            }
        }
        // The middle of the box the models take, along `axis`.
        let i = axis.index();
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for &id in &models {
            let query = spatial.leaf(id)?;
            let aabb = query.transformed_aabb(query.world_transform())?;
            lo = lo.min(f64::from(aabb.min().coords()[i]));
            hi = hi.max(f64::from(aabb.max().coords()[i]));
        }
        if !matches!(
            lo.partial_cmp(&hi),
            Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
        ) {
            return None;
        }
        let flip = reflection(axis, (lo + hi) / 2.0);
        // Each edit is made on the scene as the ones before it leave it:
        // copies that share a mesh are each given their own.
        let mut scratch = graph.clone();
        let mut edits = Vec::new();
        for &id in &turned {
            let world = scratch.world_transform_of(id).ok()?;
            let mesh = scratch
                .leaf_mesh_arc(id)
                .cloned()
                .or_else(|| spatial.leaf(id).map(|q| Arc::new(q.mesh().clone())))?;
            let local = world.inverse().ok()?.compose(&flip).compose(&world);
            let mut after: IndexedMesh = sc_scene::bake_mesh_to_world(&mesh, &local).ok()?;
            after.set_meta(mesh.meta.clone());
            let edit = leaf_mesh_swap(&scratch, id, Arc::new(after))?;
            edit.apply_to_scene(&mut scratch).ok()?;
            edits.push(edit);
        }
        for &set in &gone {
            if let Some(edit) = SceneEdit::remove_leaf_with_gc(&scratch, set)
                && edit.apply_to_scene(&mut scratch).is_ok()
            {
                edits.push(edit);
            }
        }
        let wanting: Vec<NodeId> = models
            .iter()
            .copied()
            .filter(|&m| {
                crate::support_persist::sets_of_model(spatial, &sessions, m)
                    .iter()
                    .any(|set| gone.contains(set))
            })
            .collect();
        Some((SceneEdit::Compound(edits), wanting))
    });
    let Some(Some((edit, wanting))) = built else {
        return false;
    };
    if document.apply_edit(edit.clone()).is_err() {
        return false;
    }
    document.push_edit_wanting(edit, &wanting);
    true
}
