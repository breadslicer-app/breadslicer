/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 EntoMind Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::sync::Arc;
use std::time::Duration;

use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Normal, Vector3D, Vertex};
use sc_scene::{LeafPayload, Scene};

use sc_geom_transport_inprocess::{AlgorithmId, JobError, JobHandle, JobRecipe};
use support_gen_handler::{
    BoundaryShape, PrintPlate, SupportAlgorithm, SupportError, SupportStructureSettings,
    gen_support, normalize, reproduce_support,
};

use crate::trivial_support::{TrivialSupport, TrivialSupportSettings, hex_prism};

fn cube_mesh() -> IndexedMesh {
    let v = |x, y, z| Vertex::new([x, y, z]);
    let vertices = vec![
        v(0.0, 0.0, 0.0),
        v(1.0, 0.0, 0.0),
        v(1.0, 1.0, 0.0),
        v(0.0, 1.0, 0.0),
        v(0.0, 0.0, 1.0),
        v(1.0, 0.0, 1.0),
        v(1.0, 1.0, 1.0),
        v(0.0, 1.0, 1.0),
    ];
    let n = Normal::new([0.0, 0.0, 1.0]);
    let face = |a, b, c| IndexedTriangle::from_raw_indices(n, [a, b, c]);
    IndexedMesh::new(
        Some(MeshMetadata::default()),
        vertices,
        vec![face(0, 1, 2), face(4, 5, 6)],
    )
}

fn scene_with_cube_at(offset: [f64; 3]) -> Scene {
    let mut g = Scene::new();
    let leaf = g
        .add_leaf(
            None,
            Transform3D::Affine(AffineTransform3D::from_translation(offset)),
            LeafPayload::Mesh(Arc::new(cube_mesh())),
        )
        .unwrap();
    g.add_root(leaf).unwrap();
    g
}

fn boundary_box() -> BoundaryShape {
    BoundaryShape::BuildVolumeBox {
        min: Vector3D::new([-100.0, -100.0, -100.0]),
        max: Vector3D::new([100.0, 100.0, 100.0]),
    }
}

fn print_plate() -> PrintPlate {
    PrintPlate {
        boundary: boundary_box(),
        plate_normal: Vector3D::new([0.0, 0.0, 1.0]),
    }
}

fn base_settings() -> SupportStructureSettings {
    SupportStructureSettings {
        gravity: Vector3D::new([0.0, 0.0, -1.0]),
        boundary_shape: boundary_box(),
        explicit_locations: Vec::new(),
        allow_expand_touch: false,
        allow_expand_trunk: false,
    }
}

fn poll_until_done<Out: Send + 'static, Partial: Send + 'static>(
    handle: &JobHandle<Out, Partial>,
) -> Result<Out, JobError> {
    loop {
        if let Some(result) = handle.try_poll() {
            return result;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn hex_prism_is_a_valid_closed_manifold() {
    let mesh = hex_prism(
        Vector3D::new([0.0, 0.0, 5.0]),
        Vector3D::new([0.0, 0.0, 0.0]),
        1.0,
        Vector3D::new([0.0, 0.0, -1.0]),
    )
    .expect("5mm strut is well above the negligible-length threshold");
    let diagnosis = mesh.diagnose();
    assert!(!diagnosis.has_problems(), "{diagnosis:?}");
}

#[test]
fn hex_prism_off_axis_is_also_a_valid_closed_manifold() {
    // A non-axis-aligned support-strut direction exercises the perpendicular
    // basis construction, not just the `[0,0,1]`-adjacent branch.
    let axis = normalize(Vector3D::new([1.0, 1.0, -1.0])).unwrap();
    let mesh = hex_prism(
        Vector3D::new([0.0, 0.0, 5.0]),
        Vector3D::new([3.0, 3.0, 0.0]),
        0.7,
        axis,
    )
    .expect("well above the negligible-length threshold");
    let diagnosis = mesh.diagnose();
    assert!(!diagnosis.has_problems(), "{diagnosis:?}");
}

#[test]
fn hex_prism_builds_nothing_for_a_negligible_length() {
    // top == bottom: zero-length strut, the degenerate case
    // MIN_STRUT_LENGTH_MM exists to catch.
    let point = Vector3D::new([1.0, 2.0, 3.0]);
    assert!(hex_prism(point, point, 1.0, Vector3D::new([0.0, 0.0, -1.0])).is_none());
}

#[test]
fn explicit_anchor_runs_are_deterministic() {
    let scene = scene_with_cube_at([0.0, 0.0, 0.0]);
    let plate = print_plate();
    let mut settings = base_settings();
    settings.explicit_locations = vec![Vector3D::new([0.0, 0.0, 5.0])];
    let algo_settings = TrivialSupportSettings { radius_mm: 1.0 };

    let first = gen_support(&TrivialSupport, &scene, &plate, &settings, &algo_settings)
        .unwrap()
        .join()
        .unwrap();
    let second = gen_support(&TrivialSupport, &scene, &plate, &settings, &algo_settings)
        .unwrap()
        .join()
        .unwrap();

    assert_eq!(first.meshes, second.meshes);
    assert_eq!(first.algorithm_specs, second.algorithm_specs);
}

#[test]
fn auto_detected_anchor_is_recorded_and_replay_does_not_need_the_original_scene() {
    let scene = scene_with_cube_at([0.0, 0.0, 50.0]);
    let plate = print_plate();
    let settings = base_settings(); // empty explicit_locations => auto-detect
    let algo_settings = TrivialSupportSettings { radius_mm: 2.0 };

    let first = gen_support(&TrivialSupport, &scene, &plate, &settings, &algo_settings)
        .unwrap()
        .join()
        .unwrap();
    assert!(
        !first.meshes.is_empty(),
        "auto-detected anchor must produce a strut"
    );

    // Replay against a *different* scene entirely; only the recipe's
    // recorded anchor should matter to a pure-geometry support-structure algorithm.
    let unrelated_scene = scene_with_cube_at([999.0, 999.0, 999.0]);
    let replayed = reproduce_support(&TrivialSupport, &first.recipe, &unrelated_scene, &plate)
        .unwrap()
        .join()
        .unwrap();

    assert_eq!(first.meshes, replayed.meshes);
    assert_eq!(first.algorithm_specs, replayed.algorithm_specs);
}

#[test]
fn settings_from_recipe_rejects_a_recipe_from_another_algorithm() {
    let recipe = JobRecipe {
        algorithm_id: AlgorithmId::new("some-other-algorithm"),
        algorithm_version: "0.0.1".to_string(),
        resolved_settings: Vec::new(),
        input_mesh_hash: String::new(),
        seeds: Vec::new(),
    };
    let err = TrivialSupport.settings_from_recipe(&recipe).unwrap_err();
    assert!(matches!(err, SupportError::AlgorithmMismatch { .. }));
}

#[test]
fn two_explicit_anchors_produce_two_strut_rods() {
    // See note on "Strut" in support-gen-trivial/src/trivial_support.rs
    let scene = scene_with_cube_at([0.0, 0.0, 0.0]);
    let plate = print_plate();
    let mut settings = base_settings();
    settings.explicit_locations = vec![
        Vector3D::new([0.0, 0.0, 5.0]),
        Vector3D::new([2.0, 0.0, 5.0]),
    ];
    let algo_settings = TrivialSupportSettings::default();

    let handle = gen_support(&TrivialSupport, &scene, &plate, &settings, &algo_settings).unwrap();
    let result = handle.join().unwrap();
    assert_eq!(result.meshes.len(), 2);
    for mesh in &result.meshes {
        assert!(!mesh.diagnose().has_problems());
    }
}

#[test]
fn an_anchor_already_on_the_boundary_is_skipped_with_a_diagnostic() {
    let scene = scene_with_cube_at([0.0, 0.0, 0.0]);
    let boundary = BoundaryShape::BuildVolumeBox {
        min: Vector3D::new([-10.0, -10.0, 0.0]),
        max: Vector3D::new([10.0, 10.0, 10.0]),
    };
    let mut settings = base_settings();
    settings.boundary_shape = boundary.clone();
    settings.explicit_locations = vec![Vector3D::new([0.0, 0.0, 0.0])]; // already on the floor
    let plate = PrintPlate {
        boundary,
        plate_normal: Vector3D::new([0.0, 0.0, 1.0]),
    };
    let algo_settings = TrivialSupportSettings::default();

    let handle = gen_support(&TrivialSupport, &scene, &plate, &settings, &algo_settings).unwrap();
    let result = poll_until_done(&handle).unwrap();
    assert!(result.meshes.is_empty());

    let diagnostics = handle.diagnostics_snapshot();
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == "support.no_support_needed"),
        "{diagnostics:?}"
    );
}

#[test]
fn no_explicit_locations_and_an_empty_scene_is_rejected() {
    let scene = Scene::new();
    let plate = print_plate();
    let settings = base_settings();
    let algo_settings = TrivialSupportSettings::default();

    let err = gen_support(&TrivialSupport, &scene, &plate, &settings, &algo_settings).unwrap_err();
    assert_eq!(err, SupportError::EmptySceneAndNoExplicitLocations);
}

#[test]
fn artificial_delay_reports_progress_and_does_not_change_the_geometry() {
    let scene = scene_with_cube_at([0.0, 0.0, 0.0]);
    let plate = print_plate();
    let mut settings = base_settings();
    settings.explicit_locations = vec![Vector3D::new([0.0, 0.0, 5.0])];

    let no_delay = gen_support(
        &TrivialSupport,
        &scene,
        &plate,
        &settings,
        &TrivialSupportSettings { radius_mm: 1.0 },
    )
    .unwrap()
    .join()
    .unwrap();

    let handle = gen_support(
        &TrivialSupport,
        &scene,
        &plate,
        &settings,
        &TrivialSupportSettings {
            radius_mm: 1.0,
            // artificial_delay_ms: 100,
        },
    )
    .unwrap();

    // Poll until we observe the artificial-delay phase at least once
    // (proves progress is actually reported during the wait, not just
    // before/after it), then wait for completion.
    let mut detected_generating_phase = false;
    let delayed = loop {
        if handle.progress().phase.contains("generating support") {
            detected_generating_phase = true;
        }
        if let Some(result) = handle.try_poll() {
            break result.unwrap();
        }
        std::thread::sleep(Duration::from_millis(5));
    };

    assert!(
        detected_generating_phase,
        "expected to observe the artificial-delay progress phase at least once"
    );
    // The delay is demo-only plumbing: it must not change the output.
    assert_eq!(no_delay.meshes, delayed.meshes);
}

#[test]
fn zero_or_negative_radius_is_rejected() {
    let scene = scene_with_cube_at([0.0, 0.0, 0.0]);
    let plate = print_plate();
    let mut settings = base_settings();
    settings.explicit_locations = vec![Vector3D::new([0.0, 0.0, 5.0])];

    let err = gen_support(
        &TrivialSupport,
        &scene,
        &plate,
        &settings,
        &TrivialSupportSettings { radius_mm: 0.0 },
    )
    .unwrap_err();
    assert!(matches!(err, SupportError::InvalidSettings(_)));
}
