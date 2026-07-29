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
    PersistedSliceValidity, SliceInputFingerprint, SliceInputFingerprintError,
    slice_input_fingerprint, validate_persisted_slices,
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
mod nesting_tests {
    use super::*;

    // A placement translation like the one the loader bakes onto a
    // freshly opened mesh when it drops it on the build plate.
    fn placement() -> Transform3D {
        Transform3D::Affine(AffineTransform3D::from_translation([-67.9, -67.9, 0.002]))
    }

    /// Leaf placed as the scene's direct root, carrying `transform`.
    /// This is the shape the plain-mesh loader produces: the mesh leaf
    /// is the root, then plate placement writes the translation onto it.
    fn leaf_as_root(transform: Transform3D) -> Scene {
        let mut scene = Scene::new();
        cube_leaf(&mut scene, None, transform);
        scene
    }

    /// The same leaf and `transform`, wrapped under an identity Group
    /// root. This is the shape a 3MF reader produces on reopen: build
    /// items become leaves under a fresh anonymous identity group.
    fn leaf_under_identity_group(transform: Transform3D) -> Scene {
        let mut scene = Scene::new();
        let root = scene.add_group(None, Transform3D::Identity);
        scene.add_root(root).expect("group as root");
        let leaf = scene
            .add_leaf(
                None,
                transform,
                LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
            )
            .expect("inline-mesh leaf");
        scene.add_child(root, leaf).expect("attach leaf under root");
        scene
    }

    #[test]
    fn ignores_an_identity_group_wrapping_a_leaf() {
        // The mesh sits at the same world coordinates in both scenes;
        // only the intermediate grouping differs. A save then reopen
        // round trip changes exactly this nesting (the plain loader's
        // leaf-as-root becomes leaf-under-identity-group when read back
        // from 3MF), so the fingerprint must not depend on it, or the
        // reopened slices are wrongly rejected as stale.
        assert_eq!(
            fingerprint(&leaf_as_root(placement())),
            fingerprint(&leaf_under_identity_group(placement())),
        );
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

#[cfg(test)]
mod token_tests {
    use super::*;

    #[test]
    fn a_token_is_stable_for_the_same_fingerprint() {
        let scene = identity_cube_scene();
        assert_eq!(
            fingerprint(&scene).to_token(),
            fingerprint(&scene).to_token(),
        );
    }

    #[test]
    fn equal_fingerprints_share_a_token_and_different_ones_do_not() {
        let mut moved = Scene::new();
        cube_leaf(
            &mut moved,
            None,
            Transform3D::Affine(AffineTransform3D::from_translation([1.0, 0.0, 0.0])),
        );
        assert_eq!(
            fingerprint(&identity_cube_scene()).to_token(),
            fingerprint(&identity_cube_scene()).to_token(),
        );
        assert_ne!(
            fingerprint(&identity_cube_scene()).to_token(),
            fingerprint(&moved).to_token(),
        );
    }

    #[test]
    fn a_token_is_ninety_six_lowercase_hex_characters() {
        let token = fingerprint(&identity_cube_scene()).to_token();
        assert_eq!(token.len(), 96);
        assert!(
            token
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
            "got: {token}",
        );
    }
}

#[cfg(test)]
mod guard_tests {
    use super::*;

    fn token(scene: &Scene) -> String {
        fingerprint(scene).to_token()
    }

    #[test]
    fn a_matching_token_is_in_sync() {
        let scene = identity_cube_scene();
        assert_eq!(
            validate_persisted_slices(&token(&scene), &scene, &schedule()),
            PersistedSliceValidity::InSync,
        );
    }

    #[test]
    fn a_moved_leaf_makes_the_slices_stale() {
        let stored = token(&identity_cube_scene());
        let mut moved = Scene::new();
        cube_leaf(
            &mut moved,
            None,
            Transform3D::Affine(AffineTransform3D::from_translation([1.0, 0.0, 0.0])),
        );
        assert_eq!(
            validate_persisted_slices(&stored, &moved, &schedule()),
            PersistedSliceValidity::Stale,
        );
    }

    #[test]
    fn changed_geometry_makes_the_slices_stale() {
        let stored = token(&identity_cube_scene());
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
        assert_eq!(
            validate_persisted_slices(&stored, &scene, &schedule()),
            PersistedSliceValidity::Stale,
        );
    }

    #[test]
    fn a_changed_schedule_makes_the_slices_stale() {
        let scene = identity_cube_scene();
        let stored = token(&scene);
        let other = LayerSchedule::uniform(0.10, 0.05, 10).expect("a positive, non-empty schedule");
        assert_eq!(
            validate_persisted_slices(&stored, &scene, &other),
            PersistedSliceValidity::Stale,
        );
    }

    #[test]
    fn an_unrelated_token_makes_the_slices_stale() {
        let scene = identity_cube_scene();
        assert_eq!(
            validate_persisted_slices("not-a-real-token", &scene, &schedule()),
            PersistedSliceValidity::Stale,
        );
    }

    #[test]
    fn a_fingerprint_that_cannot_be_recomputed_makes_the_slices_stale() {
        // An empty scene has no sliceable leaf, so the fingerprint cannot
        // be recomputed; unverifiable slices are treated as stale.
        let stored = token(&identity_cube_scene());
        assert_eq!(
            validate_persisted_slices(&stored, &Scene::new(), &schedule()),
            PersistedSliceValidity::Stale,
        );
    }
}
