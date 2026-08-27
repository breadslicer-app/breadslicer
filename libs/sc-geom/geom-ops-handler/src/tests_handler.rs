/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 EntoMind Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use sc_geom_transport_inprocess::{AlgorithmId, JobRecipe};
use sc_scene::Scene;

use crate::handler::{
    GeomOpAlgorithm, GeomOpError, GeomOpJobContext, GeomOpResult, reproduce_geom_op, run_geom_op,
};

const NOOP_ALGORITHM_ID: &str = "test-noop-v0";

/// Minimal `GeomOpAlgorithm` mock proving the handler's generic
/// dispatch compiles and runs end to end. Not a real operation.
/// No concrete geometry algorithm is implemented in this crate yet. It returns zero meshes.
#[derive(Clone, Copy, Debug, Default)]
struct NoOpAlgorithm;

#[derive(Clone, Debug, Default, PartialEq)]
struct NoOpSettings;

#[derive(Clone, Debug, PartialEq)]
struct NoOpSpecs;

impl GeomOpAlgorithm for NoOpAlgorithm {
    type Settings = NoOpSettings;
    type InferredSpecs = NoOpSpecs;
    type Partial = ();

    fn id(&self) -> AlgorithmId {
        AlgorithmId::new(NOOP_ALGORITHM_ID)
    }

    fn version(&self) -> &str {
        "0.0.0"
    }

    fn start(
        &self,
        _ctx: GeomOpJobContext<Self::Settings>,
    ) -> Result<
        sc_geom_transport_inprocess::JobHandle<GeomOpResult<Self::InferredSpecs>, Self::Partial>,
        GeomOpError,
    > {
        Ok(sc_geom_transport_inprocess::JobHandle::spawn(|_worker| {
            Ok(GeomOpResult {
                meshes: Vec::new(),
                algorithm_specs: NoOpSpecs,
                resolution_trail: Vec::new(),
                recipe: JobRecipe {
                    algorithm_id: AlgorithmId::new(NOOP_ALGORITHM_ID),
                    algorithm_version: "0.0.0".to_string(),
                    resolved_settings: Vec::new(),
                    input_mesh_hash: String::new(),
                    seeds: Vec::new(),
                },
            })
        }))
    }

    fn settings_from_recipe(&self, recipe: &JobRecipe) -> Result<Self::Settings, GeomOpError> {
        if recipe.algorithm_id.as_str() != NOOP_ALGORITHM_ID {
            return Err(GeomOpError::AlgorithmMismatch {
                recipe_algorithm: recipe.algorithm_id.clone(),
                requested_algorithm: self.id(),
            });
        }
        Ok(NoOpSettings)
    }
}

#[test]
fn run_geom_op_and_reproduce_round_trip() {
    let scene = Scene::new();
    let handle = run_geom_op(&NoOpAlgorithm, &scene, &NoOpSettings).unwrap();
    let result = handle.join().unwrap();
    assert!(result.meshes.is_empty());

    let replayed = reproduce_geom_op(&NoOpAlgorithm, &result.recipe, &scene)
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(result.algorithm_specs, replayed.algorithm_specs);
}

#[test]
fn reproduce_rejects_a_recipe_from_another_algorithm() {
    let recipe = JobRecipe {
        algorithm_id: AlgorithmId::new("some-other-algorithm"),
        algorithm_version: "0.0.1".to_string(),
        resolved_settings: Vec::new(),
        input_mesh_hash: String::new(),
        seeds: Vec::new(),
    };
    let scene = Scene::new();
    let err = reproduce_geom_op(&NoOpAlgorithm, &recipe, &scene).unwrap_err();
    assert!(matches!(err, GeomOpError::AlgorithmMismatch { .. }));
}
