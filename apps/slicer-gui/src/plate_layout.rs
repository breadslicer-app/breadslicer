/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The scene's objects on its plates (#545): which plate each stands on,
//! and the edit that moves one to another plate.

use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_spatial::SceneSpatial;
use sc_scene::NodeId;

use crate::build_volume::BuildVolume;
use crate::plates::Plates;
use crate::scene_document::{SceneDocument, SceneEdit};

/// The plate the object `leaf` of `spatial` belongs to: the one whose cell
/// holds the middle of its footprint.
pub(crate) fn plate_of_leaf(
    spatial: &SceneSpatial,
    volume: &BuildVolume,
    plates: Plates,
    leaf: NodeId,
) -> Option<u32> {
    let query = spatial.leaf(leaf)?;
    let aabb = query.transformed_aabb(query.world_transform())?;
    let (min, max) = (aabb.min().coords(), aabb.max().coords());
    Some(crate::plates::plate_of(
        volume,
        plates,
        (min[0] + max[0]) / 2.0,
        (min[1] + max[1]) / 2.0,
    ))
}

/// The edit that moves the object `leaf` of `document` by `by` (mm) in the
/// plate's plane, its supports following it.
pub(crate) fn moved_by(
    document: &SceneDocument,
    leaf: NodeId,
    by: glam::Vec3,
) -> Option<SceneEdit> {
    let before = document
        .with_scene(|spatial| spatial.leaf(leaf).map(|query| query.local_transform()))
        .flatten()?;
    let shift = Transform3D::Affine(AffineTransform3D::from_translation([
        f64::from(by.x),
        f64::from(by.y),
        0.0,
    ]));
    Some(SceneEdit::SetLocalTransform {
        node_id: leaf,
        before,
        after: shift.compose(&before),
    })
}
