/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 EntoMind Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Dispatches a geometry-operation job to a concrete algorithm:
//! such as CSG subtract/intersect, mesh fixing, and future variants.
//! Based on the algorithm the caller (app/cli) picks.
//! Same protocol/shape/contract as `support-gen-handler`'s [`GeomOpAlgorithm`]. It may be merged.
//! See `SupportAlgorithm`. Currently the two handlers share nothing but the underlying
//! `sc-geom-transport-inprocess` job, but a caller sees the same "submit, poll, get one result"
//! protocol/shape/contract.
//!
//! Implementation detail: Currently (this may change), unlike support generation (which gets one algorithm-family
//! handler per concern) every geometry operation shares this one handler;
//! `GeomOpAlgorithm::id()` names which operation *and* which
//! variant of it a given implementation is (e.g. a future
//! `"csg-subtract-v0"`). No concrete operation is registered yet: see
//! the crate README.
//!
//! The protocol/contract stays uniform with support generation on purpose:
//! Multiple meshes input and multiple mehes can be output.
//! One input [`Scene`] in (which may hold more than one object, eg, both operands of a CSG-subtract).

use sc_geom_transport_inprocess::{AlgorithmId, JobHandle, JobRecipe, ResolutionRecord};
use sc_mesh_core::IndexedMesh;
use sc_scene::Scene;

/// Everything one geometry-operation job needs, bundled for a worker
/// thread: an owned [`Scene`] (lightweight to clone. Its mesh resources
/// are `Arc`-backed; may hold more than one object) and the
/// algorithm's own settings. Unlike support generation, there is no
/// cross-algorithm common-settings type here yet. Nothing has
/// surfaced as genuinely shared across subtract / intersect /
/// fix-mesh, so each algorithm's `Settings` carries everything it
/// needs on its own. Add one here (mirroring
/// `support-gen-handler::SupportStructureSettings`) only once a
/// second concrete operation actually needs to share a field with the
/// first.
#[derive(Clone, Debug)]
pub struct GeomOpJobContext<S> {
    pub scene: Scene,
    pub algorithm_settings: S,
}

/// A finished geometry-operation job's payload: the resulting
/// mesh(es) (`Vec` is used for multiple outputs)
/// plus everything needed to explain and reproduce them.
#[derive(Clone, Debug, PartialEq)]
pub struct GeomOpResult<A> {
    pub meshes: Vec<IndexedMesh>,
    pub algorithm_specs: A,
    pub resolution_trail: Vec<ResolutionRecord>,
    pub recipe: JobRecipe,
}

/// Why a geometry-operation job could not be submitted. Distinct from
/// `sc_geom_transport_inprocess::JobError`, which covers failure
/// *during* a job already running. These are all rejected before a
/// worker thread is even spawned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GeomOpError {
    /// The scene has no geometry for the algorithm to operate on.
    EmptyScene,
    /// A settings field failed an algorithm's own validation.
    InvalidSettings(String),
    /// A recipe named an algorithm other than the one asked to
    /// reproduce it.
    AlgorithmMismatch {
        recipe_algorithm: AlgorithmId,
        requested_algorithm: AlgorithmId,
    },
    /// A recipe's `resolved_settings` were missing or malformed for a
    /// key the algorithm needed to reconstruct its settings.
    MalformedRecipe(String),
}

impl std::fmt::Display for GeomOpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyScene => write!(f, "the scene has no geometry to operate on"),
            Self::InvalidSettings(reason) => write!(f, "invalid settings: {reason}"),
            Self::AlgorithmMismatch {
                recipe_algorithm,
                requested_algorithm,
            } => write!(
                f,
                "recipe was produced by algorithm '{recipe_algorithm}', not '{requested_algorithm}'"
            ),
            Self::MalformedRecipe(reason) => write!(f, "malformed recipe: {reason}"),
        }
    }
}

impl std::error::Error for GeomOpError {}

/// A geometry-operation strategy: CSG subtract/intersect, mesh
/// fixing, and future variants each implement this once.
/// [`run_geom_op`] / [`reproduce_geom_op`] are generic over it, so
/// driver code never matches on which algorithm it is holding.
pub trait GeomOpAlgorithm: Send + Sync + 'static {
    /// Knobs specific to this algorithm/operation.
    type Settings: Clone + Send + 'static;
    /// What this algorithm additionally reports about a finished job,
    /// beyond the mesh(es) and the common [`GeomOpResult`] fields.
    type InferredSpecs: Clone + Send + 'static + std::fmt::Debug + PartialEq;
    /// A lighter-weight snapshot this algorithm can publish
    /// repeatedly while a job runs. `()` for an algorithm with
    /// nothing meaningful to preview.
    type Partial: Clone + Send + 'static;

    fn id(&self) -> AlgorithmId;
    fn version(&self) -> &str;

    /// Submits a job. Returns immediately with a handle; the actual
    /// work runs on its own thread. Fails synchronously (no thread
    /// spawned) when the inputs are unusable. See also: [`GeomOpError`].
    fn start(
        &self,
        ctx: GeomOpJobContext<Self::Settings>,
    ) -> Result<JobHandle<GeomOpResult<Self::InferredSpecs>, Self::Partial>, GeomOpError>;

    /// Reconstructs the settings a [`JobRecipe`] this algorithm
    /// produced was resolved to, so [`reproduce_geom_op`] can replay
    /// the job without re-running any auto-detection.
    fn settings_from_recipe(&self, recipe: &JobRecipe) -> Result<Self::Settings, GeomOpError>;
}

/// Submits a geometry-operation job for `scene`, running
/// `algorithm_settings` through `algorithm`.
pub fn run_geom_op<A: GeomOpAlgorithm>(
    algorithm: &A,
    scene: &Scene,
    algorithm_settings: &A::Settings,
) -> Result<JobHandle<GeomOpResult<A::InferredSpecs>, A::Partial>, GeomOpError> {
    let ctx = GeomOpJobContext {
        scene: scene.clone(),
        algorithm_settings: algorithm_settings.clone(),
    };
    algorithm.start(ctx)
}

/// Deterministically replays a finished job from its [`JobRecipe`].
/// `scene` is supplied fresh by the caller like any other job.
/// This crate does not check the match of payload mesh with the recipe it was originally produced from.
pub fn reproduce_geom_op<A: GeomOpAlgorithm>(
    algorithm: &A,
    recipe: &JobRecipe,
    scene: &Scene,
) -> Result<JobHandle<GeomOpResult<A::InferredSpecs>, A::Partial>, GeomOpError> {
    let algorithm_settings = algorithm.settings_from_recipe(recipe)?;
    run_geom_op(algorithm, scene, &algorithm_settings)
}
