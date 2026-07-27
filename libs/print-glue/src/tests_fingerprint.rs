/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Tests for [`slice_input_fingerprint`]: determinism, sensitivity to
//! every slicing input, insensitivity to non-inputs, and error
//! propagation.

use std::sync::Arc;

use sc_mesh_core::scene::node::{LeafPayload, SceneMetadata};
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_core::{Scene, Vertex};
use sc_mesh_fixtures::unit_cube_mesh;
use sc_sl1cer::error::SliceSceneError;
use sc_sl1cer::profile::LayerSchedule;

use crate::fingerprint::{
    SliceInputFingerprint, SliceInputFingerprintError, slice_input_fingerprint,
};

fn schedule() -> LayerSchedule {
    LayerSchedule::uniform(0.05, 0.05, 10).expect("a positive, non-empty schedule")
}

fn cube_leaf(scene: &mut Scene, name: Option<&str>, transform: Transform3D) {
    let meta = name.map(|n| SceneMetadata {
        name: Some(n.to_string()),
        ..Default::default()
    });
    let leaf = scene
        .add_leaf(
            meta,
            transform,
            LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
        )
        .expect("inline-mesh leaf");
    scene.add_root(leaf).expect("fresh leaf becomes a root");
}

fn identity_cube_scene() -> Scene {
    let mut scene = Scene::new();
    cube_leaf(&mut scene, None, Transform3D::Identity);
    scene
}

fn fingerprint(scene: &Scene) -> SliceInputFingerprint {
    slice_input_fingerprint(scene, &schedule()).expect("a cube scene fingerprints")
}

#[cfg(test)]
mod sensitivity_tests {
    use super::*;

    #[test]
    fn is_deterministic_for_the_same_inputs() {
        let scene = identity_cube_scene();
        assert_eq!(fingerprint(&scene), fingerprint(&scene));
    }

    #[test]
    fn ignores_leaf_names() {
        let mut named = Scene::new();
        cube_leaf(&mut named, Some("cube"), Transform3D::Identity);
        assert_eq!(fingerprint(&named), fingerprint(&identity_cube_scene()));
    }

    #[test]
    fn changes_when_a_leaf_moves() {
        let mut moved = Scene::new();
        cube_leaf(
            &mut moved,
            None,
            Transform3D::Affine(AffineTransform3D::from_translation([1.0, 0.0, 0.0])),
        );
        assert_ne!(fingerprint(&moved), fingerprint(&identity_cube_scene()));
    }

    #[test]
    fn changes_when_geometry_changes() {
        let mut mesh = unit_cube_mesh();
        mesh.vertices[0] = Vertex::new([9.0, 9.0, 9.0]);
        let mut scene = Scene::new();
        let leaf = scene
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .expect("inline-mesh leaf");
        scene.add_root(leaf).expect("fresh leaf becomes a root");
        assert_ne!(fingerprint(&scene), fingerprint(&identity_cube_scene()));
    }

    #[test]
    fn changes_when_a_leaf_is_added() {
        let mut two = Scene::new();
        cube_leaf(&mut two, None, Transform3D::Identity);
        cube_leaf(&mut two, None, Transform3D::Identity);
        assert_ne!(fingerprint(&identity_cube_scene()), fingerprint(&two));
    }

    #[test]
    fn changes_with_the_layer_height() {
        let scene = identity_cube_scene();
        let a = slice_input_fingerprint(&scene, &LayerSchedule::uniform(0.05, 0.05, 10).unwrap())
            .unwrap();
        let b = slice_input_fingerprint(&scene, &LayerSchedule::uniform(0.10, 0.05, 10).unwrap())
            .unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn changes_with_the_first_layer_height() {
        let scene = identity_cube_scene();
        let a = slice_input_fingerprint(&scene, &LayerSchedule::uniform(0.05, 0.05, 10).unwrap())
            .unwrap();
        let b = slice_input_fingerprint(&scene, &LayerSchedule::uniform(0.05, 0.08, 10).unwrap())
            .unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn changes_with_the_layer_count() {
        let scene = identity_cube_scene();
        let a = slice_input_fingerprint(&scene, &LayerSchedule::uniform(0.05, 0.05, 10).unwrap())
            .unwrap();
        let b = slice_input_fingerprint(&scene, &LayerSchedule::uniform(0.05, 0.05, 11).unwrap())
            .unwrap();
        assert_ne!(a, b);
    }
}

#[cfg(test)]
mod error_tests {
    use super::*;

    #[test]
    fn empty_scene_is_an_enumeration_error() {
        let scene = Scene::new();
        assert!(matches!(
            slice_input_fingerprint(&scene, &schedule()).unwrap_err(),
            SliceInputFingerprintError::Enumeration(SliceSceneError::EmptyScene),
        ));
    }
}
