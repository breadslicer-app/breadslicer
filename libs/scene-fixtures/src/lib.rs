/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Scenes for tests, built from the meshes of [`sc_mesh_fixtures`] or from the
//! caller's meshes, shared between sibling crates' tests.

use std::sync::Arc;

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_fixtures::{cuboid_with_y_tunnel, cuboid_with_z_tunnel, named, unit_cube_mesh};
use sc_scene::Scene;
use sc_scene::node::{LeafPayload, NodeId};

#[cfg(test)]
mod tests_group;
#[cfg(test)]
mod tests_roots;

/// Builds a scene in which each mesh, in order, is a root leaf with its
/// transform.
pub fn leaves_as_roots(leaves: impl IntoIterator<Item = (IndexedMesh, Transform3D)>) -> Scene {
    let mut scene = Scene::new();
    for (mesh, transform) in leaves {
        let leaf = scene
            .add_leaf(None, transform, LeafPayload::Mesh(Arc::new(mesh)))
            .expect("inline-mesh leaf");
        scene.add_root(leaf).expect("fresh leaf becomes a root");
    }
    scene
}

/// Builds a scene in which each mesh, in order, is a root leaf with an identity
/// transform.
pub fn meshes_as_roots(meshes: impl IntoIterator<Item = IndexedMesh>) -> Scene {
    leaves_as_roots(meshes.into_iter().map(|mesh| (mesh, Transform3D::Identity)))
}

/// Adds `mesh` as an inline-mesh leaf of `parent` under `transform`, and
/// returns the leaf's id. `parent` must be a group or a CSG node of `scene`;
/// the call panics on any other node.
pub fn add_mesh_leaf(
    scene: &mut Scene,
    parent: NodeId,
    mesh: IndexedMesh,
    transform: Transform3D,
) -> NodeId {
    let leaf = scene
        .add_leaf(None, transform, LeafPayload::Mesh(Arc::new(mesh)))
        .expect("inline-mesh leaf");
    scene
        .add_child(parent, leaf)
        .expect("a fresh leaf attaches to a group or a CSG node");
    leaf
}

/// Builds a scene whose only root is a group with an identity transform,
/// holding each mesh, in order, as a leaf with its transform.
pub fn leaves_under_group_root(
    leaves: impl IntoIterator<Item = (IndexedMesh, Transform3D)>,
) -> Scene {
    let mut scene = Scene::new();
    let root = scene.add_group(None, Transform3D::Identity);
    scene.add_root(root).expect("fresh group becomes a root");
    for (mesh, transform) in leaves {
        add_mesh_leaf(&mut scene, root, mesh, transform);
    }
    scene
}

/// Builds the trio: the unit cube, the Z-tunnel prism and the Y-tunnel prism,
/// in that order, as by [`meshes_as_roots`]. The scene spans x and y in [-1, 1]
/// and z in [-0.5, 2].
pub fn trio_as_roots() -> Scene {
    meshes_as_roots([
        unit_cube_mesh(),
        cuboid_with_z_tunnel(),
        cuboid_with_y_tunnel(),
    ])
}

/// Builds the named trio: a scene whose only root is a group with an identity
/// transform, holding the unit cube, the Z-tunnel prism and the Y-tunnel prism,
/// in that order, named "Cube", "ZTunnel" and "YTunnel", and translated along x
/// by 0, 5 and -5 mm plus `offset`. A leaf whose translation is 0 on every axis
/// gets `Transform3D::Identity`.
pub fn named_trio_under_group_root(offset: [f64; 3]) -> Scene {
    let sources = [
        ("Cube", unit_cube_mesh(), [0.0, 0.0, 0.0]),
        ("ZTunnel", cuboid_with_z_tunnel(), [5.0, 0.0, 0.0]),
        ("YTunnel", cuboid_with_y_tunnel(), [-5.0, 0.0, 0.0]),
    ];
    leaves_under_group_root(sources.map(|(name, mesh, place)| {
        let translation: [f64; 3] = std::array::from_fn(|axis| place[axis] + offset[axis]);
        let transform = if translation == [0.0; 3] {
            Transform3D::Identity
        } else {
            Transform3D::Affine(AffineTransform3D::from_translation(translation))
        };
        (named(mesh, name), transform)
    }))
}
