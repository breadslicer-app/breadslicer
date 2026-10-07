/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Scenes for tests, built from the meshes of [`sc_mesh_fixtures`] or from the
//! caller's meshes, shared between sibling crates' tests.

use std::sync::Arc;

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::Transform3D;
use sc_mesh_fixtures::{cuboid_with_y_tunnel, cuboid_with_z_tunnel, unit_cube_mesh};
use sc_scene::Scene;
use sc_scene::node::LeafPayload;

#[cfg(test)]
mod tests_roots;

/// Builds a scene in which each mesh, in order, is a root leaf with an identity
/// transform.
pub fn meshes_as_roots(meshes: impl IntoIterator<Item = IndexedMesh>) -> Scene {
    let mut scene = Scene::new();
    for mesh in meshes {
        let leaf = scene
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .expect("inline-mesh leaf");
        scene.add_root(leaf).expect("fresh leaf becomes a root");
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
