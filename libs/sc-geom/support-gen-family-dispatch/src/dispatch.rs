/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 EntoMind Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! See notes on `libs/sc-geom/support-gen-family-dispatch/src/lib.rs`

use generic_geom_alg_family_dispatch::AlgorithmFamilyCommonSpecs;
use generic_geom_alg_family_dispatch::BoundaryShape;
use sc_mesh_core::{IndexedMesh, Vector3D};

use crate::print_plate::PrintPlate;

/// The inputs that are common across algorithms in algorithm-family "support-generation".
/// It defines, what is common across all support algorithms (hence, defines this family).
/// It is incomplete. It currently covers the iputs that are recealed by current implementations.
/// (currenlty, the toy "trivial support" algorithm , calyplex).
/// It is at the elvel of `AlgorithmFamilyCommonSpecs`, which itself is expected to grow a bit.
/// Future algorithms may descover fields to be added to `GeomAlgSpecificSpecs`,
/// some may be genuinely shared across more than one algorithm,
/// and then they will be added here to `SupportStructureSpecs`
/// as the specialisation of `AlgorithmFamilyCommonSpecs` for support generation.
/// In other words,
/// later algorithms are expected to add fields here to `SupportStructureSpecs`,
/// but only when genuinely shared across more than one algorithm.
/// and to keep the rest on their own `specific_alg_specs: &GeomAlgSpecificSpecs` type.
#[derive(Clone, Debug, PartialEq)]
pub struct SupportStructureSpecs {
    /// Direction objects fall, as a general vector: not assumed to be `-Z`.
    pub gravity: Vector3D<f64>,

    /// The region generated support geometry must stay within for this
    /// job. May be a subset of the printer's full build volume (see
    /// [`PrintPlate::boundary`]). The caller's choice, not the
    /// 3d-printer machine's limit.
    pub boundary_shape: BoundaryShape,

    /// The printer's own physical context. Not consulted by any
    /// algorithm yet -- carried here so a boundary- or
    /// plate-normal-aware algorithm can use it without a signature
    /// change.
    pub print_plate: PrintPlate,

    /// Support anchor points that the caller is giving.
    /// Empty means "detect for me": implies an algorithm that supports auto-detection
    /// records (and returns) what it picked, via a `JobRecipe`,
    /// so that a "reproduce"/replay operation never re-detects.
    pub explicit_locations: Vec<Vector3D<f64>>,
    pub allow_expand_touch: bool,
    pub allow_expand_trunk: bool,
}

/// Failures specific to the support-generation family, reached
/// generically via `generic_geom_alg_family_dispatch::GeomAlgorithmError::FamilySpecific`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SupportAlgError {
    /// No explicit anchor locations were given and the scene has no
    /// geometry to auto-detect one from.
    EmptySceneAndNoExplicitLocations,
    /// The algorithm does not support this job's boundary shape yet
    /// (e.g. [`BoundaryShape::Mesh`] against an algorithm that only
    /// implements box queries).
    UnsupportedBoundaryShape,
    /// `gravity` was the zero vector, which has no direction.
    ZeroGravity,
}

impl std::fmt::Display for SupportAlgError {
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
        }
    }
}

impl std::error::Error for SupportAlgError {}

impl AlgorithmFamilyCommonSpecs<SupportAlgError> for SupportStructureSpecs {
    fn validate(&self) -> Result<(), SupportAlgError> {
        if normalize(self.gravity).is_none() {
            return Err(SupportAlgError::ZeroGravity);
        }
        Ok(())
    }
}

/// A heuristic that says: per-vertex, "how much does this need support"
/// score. In `[0.0, 1.0]`. It is aligned one-to-one with
/// `mesh.vertices()`: this helps the app (driver/caller) to colour-code
/// as a heat map. Since it is lightweight, it is synchronous
/// (unlike a `generic_geom_alg_family_dispatch::run_geom_algorithm` job).
///
/// It is a version-0.0.0 heuristic (intended for sanity testing of E2E integration itself,
/// including highlighting overhand (support-needing) for GUI,
/// not geometry operations), shall not be mistaken with a proper "structural analysis".
///
/// Heuristic: it scores each vertex by how much its neighbouring faces'
/// normals point along `gravity`. Direction: a face whose normal points
/// the way gravity pulls is a downward-facing (overhanging) surface.
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
/// [`generic_geom_alg_family_dispatch::GeomAlgorithm`] for this family (outside this
/// crate) legitimately need it too.
pub fn normalize(v: Vector3D<f64>) -> Option<Vector3D<f64>> {
    let n = v.norm();
    if n < 1e-12 {
        None
    } else {
        let c = v.coords();
        Some(Vector3D::new([c[0] / n, c[1] / n, c[2] / n]))
    }
}
