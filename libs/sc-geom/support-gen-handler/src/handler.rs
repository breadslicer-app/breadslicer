/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 EntoMind Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Support-structure generation: the first geometry operation this
//! crate hosts, and the one that establishes the shape every later
//! operation (shell, offset, topology optimisation, ...) will follow.
//!
//! [`SupportAlgorithm`] is the "seam" (binder) for the plugin-like agorithms.
//! It can be a general geometric operation,
//! or a support-generation algorithm: one of distinct methods, eg,
//! * `support-gen-trivial`'s `TrivialSupport`,
//! * `support-gen-chalyplex` (in-development),
//! * a community heuristic,
//! * it can be an out-of-process adapter speaking to an external executable, or a Python process.
//! * A shell/offset algorithm
//!
//! It implements it once, and driver code calls [`gen_support`] / [`reproduce_support`]
//! without needing to know which. [`SupportStructureSettings`] carries
//! the inputs every support algorithm shares (gravity, the region it
//! must stay within, explicit or auto-detected anchor locations);
//! anything specific to one algorithm's numerics lives in that
//! algorithm's own `Settings` associated type instead of growing this
//! struct without bound.

use sc_geom_transport_inprocess::{AlgorithmId, JobHandle, JobRecipe, ResolutionRecord};
use sc_mesh_core::{IndexedMesh, Vector3D};
use sc_scene::Scene;

use crate::boundary::BoundaryShape;
use crate::print_plate::PrintPlate;

/// Common, cross-algorithm support-generation inputs. Not exhaustive:
/// this v0 covers what the toy "trivial support" algorithm needs;
/// Later algorithms
/// are expected to add fields here only when genuinely shared across
/// more than one algorithm, and to keep the rest on their own
/// `Settings` type.
#[derive(Clone, Debug, PartialEq)]
pub struct SupportStructureSettings {
    /// Direction objects fall, as a general vector: not assumed to be `-Z`.
    pub gravity: Vector3D<f64>,

    /// The region generated support geometry must stay within for
    /// this job. May be a subset of the printer's full build volume
    /// (see [`PrintPlate::boundary`]).
    /// The caller's choice, not the 3d-printer machine's limit.
    pub boundary_shape: BoundaryShape,

    /// Support anchor points the caller already chose.
    /// Empty means  "detect for me": implies existence of an algorithm that supports auto-detection
    /// records (and returns) what it picked. Returns it via a [`JobRecipe`],
    /// so that a "reproduce"/replay operation ([`reproduce_support`]) never re-detects.
    pub explicit_locations: Vec<Vector3D<f64>>,
    pub allow_expand_touch: bool,
    pub allow_expand_trunk: bool,
}

/// Everything one support-generation job needs, bundled for a "worker thread":
/// 1. an owned [`Scene`] (is cheap to clone, because its mesh resources are backed by `Arc`, hence, are a reference),
/// 2. the 3D printer/machine context (tightens the constraints),
/// 3. the setting common among all support-generator algorithms, and
/// 4. the algorithm's own settings.
#[derive(Clone, Debug)]
pub struct SupportJobContext<S> {
    pub scene: Scene,
    pub print_plate: PrintPlate,
    pub common: SupportStructureSettings,
    pub algorithm_settings: S,
}

/// A finished (or in-progress, via [`JobHandle::latest_partial`])
/// geometric processing (support-generation) job's result payload:
/// 1. the generated mesh(es): the additional geometry for the app to add to the scene,
///    never a modified copy of the input.
/// 2. everything needed to explain and reproduce them again.
#[derive(Clone, Debug, PartialEq)]
pub struct SupportStructureResult<A> {
    pub meshes: Vec<IndexedMesh>,
    pub algorithm_specs: A,
    pub resolution_trail: Vec<ResolutionRecord>,
    pub recipe: JobRecipe,
}

/// Why a support-generation job could not be submitted.
/// Distinct from `sc_geom_transport_inprocess::JobError`: which covers failure *during* a job that is already running.
/// Instead, these are all rejected before a worker thread is even spawned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SupportError {
    /// No explicit anchor locations were given and the scene has no
    /// geometry to auto-detect one from.
    EmptySceneAndNoExplicitLocations,
    /// The algorithm does not support this job's boundary shape yet
    /// (e.g. [`BoundaryShape::Mesh`] against an algorithm that only
    /// implements box queries).
    UnsupportedBoundaryShape,
    /// `gravity` was the zero vector, which has no direction.
    ZeroGravity,
    /// A settings field failed an algorithm's own validation (e.g. a
    /// non-positive radius).
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

impl std::fmt::Display for SupportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptySceneAndNoExplicitLocations => write!(
                f,
                "no explicit support locations were given and the scene has no geometry to auto-detect one from"
            ),
            Self::UnsupportedBoundaryShape => {
                write!(
                    f,
                    "this algorithm does not support the job's boundary shape"
                )
            }
            Self::ZeroGravity => write!(f, "gravity must be a non-zero vector"),
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

impl std::error::Error for SupportError {}

/// A support-generation strategy. Implemented once per algorithm
/// (this crate's "trivial support", and every future real or community algorithm);
/// [`gen_support`] and [`reproduce_support`] are generic over it, so
/// driver code never matches on which algorithm it is holding.
pub trait SupportAlgorithm: Send + Sync + 'static {
    /// Knobs specific to this algorithm, composed alongside
    /// [`SupportStructureSettings`] for a job.
    type Settings: Clone + Send + 'static;
    /// What this algorithm additionally reports about a finished job,
    /// beyond the mesh(es) and the common [`SupportStructureResult`]
    /// fields.
    type InferredSpecs: Clone + Send + 'static + std::fmt::Debug + PartialEq;
    /// A lighter-weight snapshot this algorithm can publish
    /// repeatedly while a job runs (see `sc_geom_transport_inprocess::JobWorker::publish_partial`).
    /// `()` for an algorithm with nothing meaningful to preview.
    type Partial: Clone + Send + 'static;

    fn id(&self) -> AlgorithmId;
    fn version(&self) -> &str;

    /// Submits a geometry processing job. Returns immediately with a handle; the actual
    /// work runs on its own thread.
    /// Fails synchronously (no thread spawned) when the inputs are unusable.
    /// See also: [`SupportError`].
    fn start(
        &self,
        ctx: SupportJobContext<Self::Settings>,
    ) -> Result<JobHandle<SupportStructureResult<Self::InferredSpecs>, Self::Partial>, SupportError>;

    /// Reconstructs the settings a [`JobRecipe`] this algorithm
    /// produced was resolved to, so [`reproduce_support`] can replay
    /// the geometry-processing job without re-running any auto-detection. Fails with
    /// [`SupportError::AlgorithmMismatch`] when `recipe` names a
    /// different algorithm, or [`SupportError::MalformedRecipe`] when
    /// an expected key is missing or unparsable.
    fn settings_from_recipe(
        &self,
        recipe: &JobRecipe,
    ) -> Result<(SupportStructureSettings, Self::Settings), SupportError>;
}

/// Submits a support-generation job for `scene` under `print_plate`,
/// combining the settings shared by every algorithm (`common`) with
/// one algorithm's own knobs (`algorithm_settings`).
pub fn gen_support<A: SupportAlgorithm>(
    algorithm: &A,
    scene: &Scene,
    print_plate: &PrintPlate,
    common: &SupportStructureSettings,
    algorithm_settings: &A::Settings,
) -> Result<JobHandle<SupportStructureResult<A::InferredSpecs>, A::Partial>, SupportError> {
    let ctx = SupportJobContext {
        scene: scene.clone(),
        print_plate: print_plate.clone(),
        common: common.clone(),
        algorithm_settings: algorithm_settings.clone(),
    };
    algorithm.start(ctx)
}

/// Deterministically replays a finished support-generation job from its [`JobRecipe`]:
/// the settings `recipe` resolved to are reused verbatim, so an
/// algorithm that auto-detected an anchor point on the first run
/// never re-detects on replay. `scene` and `print_plate` are supplied
/// fresh by the caller like any other job.
/// This crate does not verify whether they still match what the recipe was originally produced
/// from (see  README's "Not Included" section).
pub fn reproduce_support<A: SupportAlgorithm>(
    algorithm: &A,
    recipe: &JobRecipe,
    scene: &Scene,
    print_plate: &PrintPlate,
) -> Result<JobHandle<SupportStructureResult<A::InferredSpecs>, A::Partial>, SupportError> {
    let (common, algorithm_settings) = algorithm.settings_from_recipe(recipe)?;
    gen_support(algorithm, scene, print_plate, &common, &algorithm_settings)
}

/// A heuristic that says: per-vertex, "how much does this need support" score. In `[0.0, 1.0]`.
/// It is aligned one-to-one with `mesh.vertices()`:
/// This helps the app (driver/caller) to colour-code as a heat map.
/// Since it is lightweight, it is synchronous (unlike [`gen_support`]).
/// It is a version-0.0.0 heuristic, don't consider it a proper "structural analysis".
/// Heuristic: It scores each vertex by how much its neighbouring faces' normals point along `gravity` vector.
/// Direction: A face whose normal points the way gravity pulls is a downward-facing (overhanging) surface.
pub fn suggest_support_regions(mesh: &IndexedMesh, gravity: Vector3D<f64>) -> Vec<f32> {
    let Some(gravity_dir) = normalize(gravity) else {
        return vec![0.0; mesh.vertices().len()];
    };

    let mut accum = vec![Vector3D::<f64>::new([0.0, 0.0, 0.0]); mesh.vertices().len()];
    for face in mesh.faces() {
        let n = face.normal.to_f64();
        for v in face.vertices {
            let idx: usize = v.into();
            let a = accum[idx].coords();
            let c = n.coords();
            accum[idx] = Vector3D::new([a[0] + c[0], a[1] + c[1], a[2] + c[2]]);
        }
    }

    accum
        .into_iter()
        .map(|sum| match normalize(sum) {
            Some(avg) => avg.dot_product(&gravity_dir).max(0.0) as f32,
            None => 0.0,
        })
        .collect()
}

/// Unit vector along `v`, or `None` when `v` is (numerically) zero.
/// `pub`, not `pub(crate)`: algorithm crates implementing
/// [`SupportAlgorithm`] (outside this crate) legitimately need it too.
pub fn normalize(v: Vector3D<f64>) -> Option<Vector3D<f64>> {
    let n = v.norm();
    if n < 1e-12 {
        None
    } else {
        let c = v.coords();
        Some(Vector3D::new([c[0] / n, c[1] / n, c[2] / n]))
    }
}
