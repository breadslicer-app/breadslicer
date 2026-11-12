/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The scene's objects on its plates (#545): which plate each stands on,
//! and the edit that moves one to another plate.

use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_spatial::SceneSpatial;
use sc_scene::{NodeId, Scene};

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

/// The plate the object `leaf` belongs to (#547): a support leaf the plate of
/// the model it stands on, else the one under its own footprint.
pub(crate) fn plate_holding(
    spatial: &SceneSpatial,
    volume: &BuildVolume,
    plates: Plates,
    leaf: NodeId,
    model_of: &dyn Fn(NodeId) -> Option<NodeId>,
) -> Option<u32> {
    model_of(leaf)
        .and_then(|model| plate_of_leaf(spatial, volume, plates, model))
        .or_else(|| plate_of_leaf(spatial, volume, plates, leaf))
}

/// The scene as plate `plate` of `plates` prints (#547): the objects on it
/// alone, each support leaf with the model it stands on, moved so the plate's
/// centre is the origin, as a printer's one plate is. And where that centre
/// stands in the scene, mm.
pub(crate) fn plate_scene(
    spatial: &SceneSpatial,
    volume: &BuildVolume,
    plates: Plates,
    plate: u32,
    model_of: &dyn Fn(NodeId) -> Option<NodeId>,
) -> (Scene, [f64; 2]) {
    let mut scene = spatial.graph().clone();
    let leaves: Vec<NodeId> = (0..).map_while(|i| spatial.leaf_id_at_index(i)).collect();
    for leaf in leaves {
        let on = plate_holding(spatial, volume, plates, leaf, model_of);
        if on != Some(plate) {
            let _ = scene.remove_leaf(leaf);
        }
    }
    let at = crate::plates::offset(volume, plate);
    let origin = [f64::from(at.x), f64::from(at.y)];
    let shift = Transform3D::Affine(AffineTransform3D::from_translation([
        -origin[0], -origin[1], 0.0,
    ]));
    for root in scene.roots().to_vec() {
        if let Some(node) = scene.node(root) {
            let moved = shift.compose(&node.local_transform);
            let _ = scene.set_local_transform(root, moved);
        }
    }
    (scene, origin)
}

/// The scene as the pictures of plate `plate`'s printer file show it (#547):
/// every object where it stands, but those of the other plates sent far below
/// the plate, out of the pictures' sight, each in its own place in the list as
/// the drawing needs; and the box the plate's objects take, mm.
pub(crate) fn plate_pictures(
    spatial: &SceneSpatial,
    volume: &BuildVolume,
    plates: Plates,
    plate: u32,
    model_of: &dyn Fn(NodeId) -> Option<NodeId>,
) -> (Scene, Option<(glam::Vec3, glam::Vec3)>) {
    let mut scene = spatial.graph().clone();
    let away = Transform3D::Affine(AffineTransform3D::from_translation([0.0, 0.0, -1.0e6]));
    let mut bounds: Option<(glam::Vec3, glam::Vec3)> = None;
    let leaves: Vec<NodeId> = (0..).map_while(|i| spatial.leaf_id_at_index(i)).collect();
    for leaf in leaves {
        let on = plate_holding(spatial, volume, plates, leaf, model_of);
        if on == Some(plate) {
            let aabb = spatial
                .leaf(leaf)
                .and_then(|query| query.transformed_aabb(query.world_transform()));
            if let Some(aabb) = aabb {
                let (lo, hi) = (aabb.min().coords(), aabb.max().coords());
                let (lo, hi) = (
                    glam::Vec3::new(lo[0], lo[1], lo[2]),
                    glam::Vec3::new(hi[0], hi[1], hi[2]),
                );
                bounds = Some(bounds.map_or((lo, hi), |(a, b)| (a.min(lo), b.max(hi))));
            }
        } else if let Some(node) = scene.node(leaf) {
            let sent = away.compose(&node.local_transform);
            let _ = scene.set_local_transform(leaf, sent);
        }
    }
    (scene, bounds)
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
