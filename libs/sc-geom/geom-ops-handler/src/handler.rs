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

/// Everything one geometry-algorithm job needs, bundled for a worker
/// thread: an owned [`Scene`] (cheap to clone -- its mesh resources are
/// `Arc`-backed; may hold more than one object), the family_specs shared by
/// every algorithm in this family, and this one algorithm's own secific_specs.
#[derive(Clone, Debug)]
pub struct GeomAlgorithmJobContext<C, S> {
    pub scene: Scene,
    pub common_family_alg_specs: C,
    pub specific_alg_specs: S,
}

/// A finished geometry-algorithm job's payload: the resulting mesh(es)
/// plus everything needed to explain and reproduce them.
#[derive(Clone, Debug, PartialEq)]
pub struct GeomAlgorithmResult<A> {
    pub meshes: Vec<IndexedMesh>,
    pub algorithm_specs: A,
    pub resolution_trail: Vec<ResolutionRecord>,
    pub recipe: JobRecipe,
}

/// Why a geometry-algorithm job could not be submitted (or a recipe could
/// not be replayed). Distinct from `sc_geom_transport_inprocess::JobError`,
/// which covers failure *during* a job already running.
///
/// [`Self::AlgorithmMismatch`] is the one variant no algorithm ever
/// constructs itself -- [`reproduce_geom_algorithm`] checks it
/// generically, before ever calling into the algorithm.
/// [`Self::InvalidSpecs`] and [`Self::MalformedRecipe`] are free-form
/// escape hatches an algorithm reaches for when nothing more specific
/// fits; anything worth its own typed variant, shared by every algorithm
/// in one family, belongs in that family's own error type instead, via
/// [`Self::FamilySpecific`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GeomOpError {
    /// The scene has no geometry for the algorithm to operate on.
    EmptyScene,
    /// A settings field failed an algorithm's own validation.
    InvalidSettings(String),
    /// A recipe named an algorithm other than the one asked to
    /// reproduce it.
pub enum GeomAlgorithmError<E> {
    InvalidSpecs(String),
    AlgorithmMismatch {
        recipe_algorithm: AlgorithmId,
        requested_algorithm: AlgorithmId,
    },
    /// A recipe's `resolved_settings` were missing or malformed for a
    /// key the algorithm needed to reconstruct its settings.
    MalformedRecipe(String),
    FamilySpecific(E),
}

impl<E: std::fmt::Display> std::fmt::Display for GeomAlgorithmError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSpecs(reason) => write!(f, "invalid input specs: {reason}"),
            Self::AlgorithmMismatch {
                recipe_algorithm,
                requested_algorithm,
            } => write!(
                f,
                "recipe was produced by algorithm '{recipe_algorithm}', not '{requested_algorithm}'"
            ),
            Self::MalformedRecipe(reason) => write!(f, "malformed recipe: {reason}"),
            Self::FamilySpecific(e) => std::fmt::Display::fmt(e, f),
        }
    }
}

impl<E: std::fmt::Debug + std::fmt::Display> std::error::Error for GeomAlgorithmError<E> {}

/// What every operation family's common/family specs/settings type must supply:
/// How to validate itself, and the ordinary bounds needed to
/// move it into a worker thread.
/// (shared by every algorithm in that family, e.g. `SupportStructureSpecs`.
/// or CSG subtract/intersection, or mesh fixing).
/// A family with nothing to share yet can implement this for `()`.
/// [`run_geom_op`] / [`reproduce_geom_op`] are generic over it, so
pub trait AlgorithmFamilyCommonSpecs<E>: Clone + Send + 'static {
    /// Checks this family-level specs alone,
    /// without the scene or any one specific-algorithm-level specs.
    /// An algorithm's `start()` still checks
    /// whatever it can only validate with those in hand.
    fn validate(&self) -> Result<(), E>;

    /*
    /// Knobs/specs/params specific to this algorithm/operation.
    type Settings: Clone + Send + 'static;
    */

}

/// A running (or submission-rejected) job for one [`GeomAlgorithm`], and
/// the specs pair [`GeomAlgorithm::specs_pair_from_recipe`] resolves a
/// [`JobRecipe`] to. Named aliases purely to keep the trait/function
/// signatures below readable -- both are otherwise exactly the shapes
/// this module already exposes.
pub type GeomAlgorithmOutcome<A> = Result<
    JobHandle<
        GeomAlgorithmResult<<A as GeomAlgorithm>::InferredSpecs>,
        <A as GeomAlgorithm>::Partial,
    >,
    GeomAlgorithmError<<A as GeomAlgorithm>::Error>,
>;
pub type ResolvedAlgorithmSpecs<A> = Result<
    (
        <A as GeomAlgorithm>::GeomAlgCommonSpecs,
        <A as GeomAlgorithm>::GeomAlgSpecificSpecs,
    ),
    GeomAlgorithmError<<A as GeomAlgorithm>::Error>,
>;

/// A geometry-algorithm strategy: one concrete implementation of one
/// operation family (e.g. `TrivialSupport` for the "support generation" family).
/// [`run_geom_algorithm`]/[`reproduce_geom_algorithm`] are generic over
/// it, so driver code never matches on which algorithm it is holding.
pub trait GeomAlgorithm: Send + Sync + 'static {
    /// GeomAlgSpecificSpecs shared by every algorithm in this one algorithm family
    /// For example, shared between all "support generation" algorithms:
    /// the common intention, groups them into one family).
    type GeomAlgCommonSpecs: AlgorithmFamilyCommonSpecs<Self::Error>;
    /// Knobs specific to this algorithm alone.
    type GeomAlgSpecificSpecs: Clone + Send + 'static;
    /// What this algorithm additionally reports about a finished job,
    /// beyond the mesh(es) and the common_family_alg_specs [`GeomAlgorithmResult`] fields.
    type InferredSpecs: Clone + Send + 'static + std::fmt::Debug + PartialEq;
    /// A lighter-weight snapshot this algorithm can publish repeatedly
    /// while a job runs. `()` for an algorithm with nothing meaningful
    /// to preview.
    type Partial: Clone + Send + 'static;
    /// This algorithm-family's own error type, wrapped generically via
    /// [`GeomAlgorithmError::FamilySpecific`].
    type Error: std::fmt::Debug + std::fmt::Display + std::error::Error + Clone + PartialEq + Eq;

    fn id(&self) -> AlgorithmId;
    fn version(&self) -> &str;

    /// Submits a job. Returns immediately with a handle; the actual work
    /// runs on its own thread. Fails synchronously (no thread spawned)
    /// when the inputs are unusable.
    fn start(
        &self,
        ctx: GeomAlgorithmJobContext<Self::GeomAlgCommonSpecs, Self::GeomAlgSpecificSpecs>,
    ) -> GeomAlgorithmOutcome<Self>;

    /// Reconstructs the specs/settings a [`JobRecipe`] this algorithm produced
    /// was resolved to, so [`reproduce_geom_algorithm`] can replay the job
    /// without re-running any auto-detection.
    /// The recipe's `algorithm_id` has already been checked by the time this is called.
    /// An implementation does not need to re-check it.
    fn specs_pair_from_recipe(&self, recipe: &JobRecipe) -> ResolvedAlgorithmSpecs<Self>;
}

/// Submits a geometry-algorithm job for `scene`, combining the specsspecs
/// shared by every algorithm in `algorithm`'s family (`common_family_alg_specs`/`family_specs`)
/// with this particular/specific algorithm/implementation's own knobs (`specific_alg_specs`).
pub fn run_geom_algorithm<A: GeomAlgorithm>(
    algorithm: &A,
    scene: &Scene,
    common_family_alg_specs: &A::GeomAlgCommonSpecs,
    specific_alg_specs: &A::GeomAlgSpecificSpecs,
) -> GeomAlgorithmOutcome<A> {
    common_family_alg_specs
        .validate()
        .map_err(GeomAlgorithmError::FamilySpecific)?;
    let ctx = GeomAlgorithmJobContext {
        scene: scene.clone(),
        common_family_alg_specs: common_family_alg_specs.clone(),
        specific_alg_specs: specific_alg_specs.clone(),
    };
    algorithm.start(ctx)
}

/// Deterministically replays a finished job from its [`JobRecipe`]:
/// the inputs/specs/settings `recipe` resolved to are reused verbatim,
/// so an algorithm that auto-detected something on the first run never re-detects on replay.
/// `scene` is supplied fresh by the caller like any other job.
pub fn reproduce_geom_algorithm<A: GeomAlgorithm>(
    algorithm: &A,
    recipe: &JobRecipe,
    scene: &Scene,
) -> GeomAlgorithmOutcome<A> {
    if algorithm.id() != recipe.algorithm_id {
        return Err(GeomAlgorithmError::AlgorithmMismatch {
            recipe_algorithm: recipe.algorithm_id.clone(),
            requested_algorithm: algorithm.id(),
        });
    }
    let (common_family_alg_specs, specific_alg_specs) = algorithm.specs_pair_from_recipe(recipe)?;
    run_geom_algorithm(algorithm, scene, &common_family_alg_specs, &specific_alg_specs)
}
