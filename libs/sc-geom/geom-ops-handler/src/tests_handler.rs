/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 EntoMind Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use sc_geom_transport_inprocess::{AlgorithmId, JobHandle, JobRecipe};
use sc_scene::Scene;

use crate::handler::{
    AlgorithmFamilyCommonSpecs, GeomAlgorithm, GeomAlgorithmError, GeomAlgorithmJobContext,
    GeomAlgorithmResult, reproduce_geom_algorithm, run_geom_algorithm,
};

const NOOP_ALGORITHM_ID: &str = "test-noop-v0";

/// This mock family has nothing to share across algorithms yet -- `()`
/// plays that role, same as a real, still-empty operation family would.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct NoOpAlgError;

impl std::fmt::Display for NoOpAlgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "noop family error")
    }
}

impl std::error::Error for NoOpAlgError {}

impl AlgorithmFamilyCommonSpecs<NoOpAlgError> for () {
    fn validate(&self) -> Result<(), NoOpAlgError> {
        Ok(())
    }
}

/// Minimal `GeomOpAlgorithm` mock proving the handler's generic
/// Minimal `GeomAlgorithm` mock proving the generic
/// dispatch compiles and runs end to end. Not a real operation.
/// No concrete geometry algorithm is implemented in this crate yet. It returns zero meshes.
#[derive(Clone, Copy, Debug, Default)]
struct NoOpAlgorithm;

#[derive(Clone, Debug, Default, PartialEq)]
struct NoOpSpecs;

#[derive(Clone, Debug, PartialEq)]
struct NoOpSpecs;

impl GeomAlgorithm for NoOpAlgorithm {
    type GeomAlgCommonSpecs = ();
    type GeomAlgSpecificSpecs = NoOpSpecs;
    type InferredSpecs = NoOpSpecs;
    type Partial = ();
    type Error = NoOpAlgError;

    fn id(&self) -> AlgorithmId {
        AlgorithmId::new(NOOP_ALGORITHM_ID)
    }

    fn version(&self) -> &str {
        "0.0.0"
    }

    fn start(
        &self,
        _ctx: GeomAlgorithmJobContext<Self::GeomAlgCommonSpecs, Self::GeomAlgSpecificSpecs>,
    ) -> Result<
        JobHandle<GeomAlgorithmResult<Self::InferredSpecs>, Self::Partial>,
        GeomAlgorithmError<Self::Error>,
    > {
        Ok(JobHandle::spawn(|_worker| {
            Ok(GeomAlgorithmResult {
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

    fn specs_pair_from_recipe(
        &self,
        _recipe: &JobRecipe,
    ) -> Result<(Self::GeomAlgCommonSpecs, Self::GeomAlgSpecificSpecs), GeomAlgorithmError<Self::Error>> {
        Ok(((), NoOpSpecs))
    }
}

#[test]
fn run_geom_algorithm_and_reproduce_round_trip() {
    let scene = Scene::new();
    let handle = run_geom_algorithm(&NoOpAlgorithm, &scene, &(), &NoOpSpecs).unwrap();
    let result = handle.join().unwrap();
    assert!(result.meshes.is_empty());

    let replayed = reproduce_geom_algorithm(&NoOpAlgorithm, &result.recipe, &scene)
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
    let err = reproduce_geom_algorithm(&NoOpAlgorithm, &recipe, &scene).unwrap_err();
    assert!(matches!(err, GeomAlgorithmError::AlgorithmMismatch { .. }));
}
