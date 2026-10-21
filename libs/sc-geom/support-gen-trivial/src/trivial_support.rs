/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 EntoMind Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! [`TrivialSupport`]: the reference `GeomAlgorithm` implementation for
//! the support-generation family.
//!
//! Deliberately trivial and geometrically uninteresting:
//! Terminology: "Strut" (as in "strut of a truss") means each rod for hypothetical support structures in such heuristic.
//! "Strut": one "hexagonal strut" per anchor point, (a typical heuristic support structure)
//! running straight from the anchor to the solid's boundary along `gravity`.
//! It is a watertight hexagonal-prism strut from `top` to `bottom`, with the
//! hexagon's plane perpendicular to `axis` (assumed a unit vector).

//! It exists to exercise every piece of
//! the interface end to end (specs/settings resolution with a trail, a
//! worker-thread job with progress/diagnostics/partial previews, a
//! reproducible recipe) with real, deterministic geometry, so a real
//! algorithm slots into a socket that already works.
//!
//! Determinism: once `explicit_locations` is non-empty (either the
//! caller supplied it, or a prior run's `JobRecipe`
//! did via [`TrivialSupport::specs_pair_from_recipe`]), this algorithm's
//! output is a pure function of the resolved anchors, boundary,
//! gravity and radius: it never reads mesh geometry beyond the
//! scene's world AABB, and only then when `explicit_locations` was
//! empty. A `generic_geom_alg_family_dispatch::reproduce_geom_algorithm` replay therefore
//! reproduces byte-identical meshes regardless of what `scene` is
//! passed to it, as long as the recipe's anchors are honoured (see
//! `tests_trivial_support::reproduce_support_is_byte_identical`).

use std::f64::consts::PI;

use sc_geom_transport_inprocess::{
    AlgorithmId, Diagnostic, FieldSource, JobError, JobHandle, JobProgress, JobRecipe,
    ResolutionRecord, Severity, Staged,
};
use sc_mesh_core::{Aabb, IndexedMesh, IndexedTriangle, MeshMetadata, Vector3D, Vertex};
use sc_mesh_spatial::{SceneSpatial, scene_aabb};
use sha2::{Digest, Sha256};
use support_gen_family_dispatch::{
    BoundaryShape, GeomAlgorithm, GeomAlgorithmError, GeomAlgorithmJobContext, GeomAlgorithmResult,
    PrintPlate, SupportAlgError, SupportStructureSpecs, normalize,
};

pub const TRIVIAL_SUPPORT_ALGORITHM_ID: &str = "trivial-support-v0";
pub const TRIVIAL_SUPPORT_VERSION: &str = "0.0.1";

const RESIN_DENSITY_G_PER_MM3: f64 = 0.0011; // ~1.1 g/cm^3, a typical UV resin.
const SIDES: usize = 6;

const ARTIFICIAL_DELAY_TICKS: u32 = 20;

/// Artificial, demo-only wait (see `sc_geom::TrivialSupportSpecs::artificial_delay_ms`)
/// so the progress bar has something to visibly animate through.
const ARTIFICIAL_DELAY_MS: u64 = 2_000;

/// Below this strut length (the distance from anchor to boundary),
/// [`hex_prism`] refuses to build geometry: the anchor is already
/// effectively on the boundary, and any prism would be degenerate
/// (near-zero volume, nothing worth printing). An implementation
/// detail of this toy algorithm, not a public setting: A real
/// algorithm deciding "no support needed here" would likely use its
/// own printer/material-aware threshold instead of a bare numerical
/// guard like this one.
const MIN_STRUT_LENGTH_MM: f64 = 1e-2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrivialSupportSpecs {
    pub radius_mm: f64,
    /*
    /// Artificial, demo-only delay: sleeps this many milliseconds
    /// (spread across [`ARTIFICIAL_DELAY_TICKS`] progress ticks)
    /// after the geometry is built, purely so a caller polling the
    /// job's [`JobProgress`]/[`JobHandle`] has something
    /// to watch, e.g. `apps/slicer-gui`'s menu:
    /// "Generate Support (trivial for test)" menu action sets this so the progress bar is
    /// visibly animating rather than resolving instantly. A real
    /// algorithm's job naturally takes time; this field simulates
    /// that for interface testing, it does not model actual cost.
    /// `0` (the default) disables it. Not part of the recipe: it
    /// doesn't affect the geometry, so a replay never needs it.
    /// pub artificial_delay_ms: u64,
     */
}

impl Default for TrivialSupportSpecs {
    fn default() -> Self {
        Self { radius_mm: 1.0 }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TrivialSupportInferredSpecs {
    pub anchors_used: Vec<Vector3D<f64>>,
    pub estimated_material_g: Staged<f64>,
}

/// Partial snapshot published while a support-generation job runs:
/// the supporting pieces/structures/struts/rods/cylinders built so far, and the running material estimate.
#[derive(Clone, Debug, PartialEq)]
pub struct SupportPartial {
    pub meshes_so_far: Vec<IndexedMesh>,
    pub estimated_material_g: Staged<f64>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TrivialSupport;

impl GeomAlgorithm for TrivialSupport {
    type GeomAlgCommonSpecs = SupportStructureSpecs;
    type GeomAlgSpecificSpecs = TrivialSupportSpecs;
    type InferredSpecs = TrivialSupportInferredSpecs;
    type Partial = SupportPartial;
    type Error = SupportAlgError;

    fn id(&self) -> AlgorithmId {
        AlgorithmId::new(TRIVIAL_SUPPORT_ALGORITHM_ID)
    }

    fn version(&self) -> &str {
        TRIVIAL_SUPPORT_VERSION
    }

    fn start(
        &self,
        ctx: GeomAlgorithmJobContext<Self::GeomAlgCommonSpecs, Self::GeomAlgSpecificSpecs>,
    ) -> Result<
        JobHandle<GeomAlgorithmResult<Self::InferredSpecs>, Self::Partial>,
        GeomAlgorithmError<Self::Error>,
    > {
        let GeomAlgorithmJobContext {
            scene,
            common_family_alg_specs,
            specific_alg_specs,
        } = ctx;

        if !(specific_alg_specs.radius_mm.is_finite() && specific_alg_specs.radius_mm > 0.0) {
            return Err(GeomAlgorithmError::InvalidSpecs(
                "radius_mm must be a finite, positive number".to_string(),
            ));
        }
        if matches!(
            common_family_alg_specs.boundary_shape,
            BoundaryShape::Mesh(_)
        ) {
            return Err(GeomAlgorithmError::FamilySpecific(
                SupportAlgError::UnsupportedBoundaryShape,
            ));
        }
        let gravity = normalize(common_family_alg_specs.gravity).ok_or(
            GeomAlgorithmError::FamilySpecific(SupportAlgError::ZeroGravity),
        )?;

        let scene_aabb_now = {
            let spatial = SceneSpatial::from_graph(scene.clone());
            scene_aabb(&spatial)
        };
        let input_mesh_hash = hash_aabb(scene_aabb_now);

        let mut trail = Vec::new();
        let anchors = if !common_family_alg_specs.explicit_locations.is_empty() {
            trail.push(ResolutionRecord::new(
                "support.anchors",
                FieldSource::Requested,
                format_points(&common_family_alg_specs.explicit_locations),
            ));
            common_family_alg_specs.explicit_locations.clone()
        } else {
            let aabb = scene_aabb_now.ok_or(GeomAlgorithmError::FamilySpecific(
                SupportAlgError::EmptySceneAndNoExplicitLocations,
            ))?;
            let anchor = lowest_point_along(aabb, gravity);
            trail.push(ResolutionRecord::new(
                "support.anchors",
                FieldSource::Derived,
                format_points(std::slice::from_ref(&anchor)),
            ));
            vec![anchor]
        };
        trail.push(ResolutionRecord::new(
            "support.radius_mm",
            FieldSource::Requested,
            specific_alg_specs.radius_mm.to_string(),
        ));

        let recipe = build_recipe(
            self.id(),
            self.version().to_string(),
            &common_family_alg_specs,
            &specific_alg_specs,
            &anchors,
            input_mesh_hash,
        )?;

        let boundary = common_family_alg_specs.boundary_shape.clone();
        let radius = specific_alg_specs.radius_mm;
        // let artificial_delay_ms = specific_alg_specs.artificial_delay_ms;

        Ok(JobHandle::spawn(move |worker| {
            let total = anchors.len() as u32;
            worker.report_progress(JobProgress {
                phase: "casting vertical struts".to_string(),
                completed: 0,
                total,
            });

            let mut meshes = Vec::with_capacity(anchors.len());
            let mut used_anchors = Vec::with_capacity(anchors.len());
            let mut material_g = 0.0_f64;

            for (i, anchor) in anchors.iter().enumerate() {
                if worker.is_cancelled() {
                    return Err(JobError::Cancelled);
                }

                let Some(hit) = boundary.cast_ray(*anchor, gravity) else {
                    worker.push_diagnostic(
                        Diagnostic::new(
                            Severity::Warning,
                            "support.boundary_unreachable",
                            format!(
                                "anchor {i} does not reach the solid boundary along gravity; skipped"
                            ),
                        )
                        .with_context("anchor", format_point(*anchor)),
                    );
                    continue;
                };
                // It's at downstream in processing implementation (hex_prism)) decides whether
                // this anchor actually needs a support structure; we
                // just react to `None` rather than pre-judging it here
                // (e.g. from how close the boundary already is).
                let Some(mesh) = hex_prism(*anchor, hit.point, radius, gravity) else {
                    worker.push_diagnostic(
                        Diagnostic::new(
                            Severity::Info,
                            "support.no_support_needed",
                            format!(
                                "anchor {i} is already effectively on the boundary; no support structure needed"
                            ),
                        )
                        .with_context("anchor", format_point(*anchor)),
                    );
                    continue;
                };
                material_g += estimate_prism_material_g(hit.distance, radius);
                meshes.push(mesh);
                used_anchors.push(*anchor);

                worker.publish_partial(SupportPartial {
                    meshes_so_far: meshes.clone(),
                    estimated_material_g: Staged::Provisional(material_g),
                });
                worker.report_progress(JobProgress {
                    phase: "casting vertical ".to_string(),
                    completed: (i + 1) as u32,
                    total,
                });
                // Not a real cost model: a short sleep so a caller
                // polling progress/partial actually observes more
                // than one tick, proving the wiring end to end.
                std::thread::sleep(std::time::Duration::from_millis(2));
            }

            // Artificial, demo-only wait. See `TrivialSupportSpecs::artificial_delay_ms`.
            // The geometry above is already final; this exists purely
            // so a caller watching progress has something to watch.
            let artificial_delay_ms: u64 =
                ARTIFICIAL_DELAY_MS.div_ceil(2) + rand::random_range(0..(ARTIFICIAL_DELAY_MS / 2));
            if artificial_delay_ms > 0 {
                let per_tick_ms = artificial_delay_ms / u64::from(ARTIFICIAL_DELAY_TICKS);
                for tick in 0..ARTIFICIAL_DELAY_TICKS {
                    if worker.is_cancelled() {
                        return Err(JobError::Cancelled);
                    }
                    worker.report_progress(JobProgress {
                        phase: "generating support (trivial for test)".to_string(),
                        completed: tick,
                        total: ARTIFICIAL_DELAY_TICKS,
                    });
                    std::thread::sleep(std::time::Duration::from_millis(per_tick_ms));
                }
            }

            worker.report_progress(JobProgress {
                phase: "finalising".to_string(),
                completed: total,
                total,
            });
            worker.publish_partial(SupportPartial {
                meshes_so_far: meshes.clone(),
                estimated_material_g: Staged::Final(material_g),
            });

            Ok(GeomAlgorithmResult {
                meshes,
                algorithm_specs: TrivialSupportInferredSpecs {
                    anchors_used: used_anchors,
                    estimated_material_g: Staged::Final(material_g),
                },
                resolution_trail: trail,
                recipe,
            })
        }))
    }

    fn specs_pair_from_recipe(
        &self,
        recipe: &JobRecipe,
    ) -> Result<(SupportStructureSpecs, Self::GeomAlgSpecificSpecs), GeomAlgorithmError<Self::Error>>
    {
        let gravity = parse_point(get(recipe, "support.gravity")?)?;
        let boundary_min = parse_point(get(recipe, "support.boundary.min")?)?;
        let boundary_max = parse_point(get(recipe, "support.boundary.max")?)?;
        let anchors = parse_points(get(recipe, "support.anchors")?)?;
        let radius_mm: f64 = get(recipe, "support.radius_mm")?.parse().map_err(|_| {
            GeomAlgorithmError::MalformedRecipe("support.radius_mm is not a number".to_string())
        })?;
        let allow_expand_touch = recipe
            .get("support.allow_expand_touch")
            .is_some_and(|v| v == "true");
        let allow_expand_trunk = recipe
            .get("support.allow_expand_trunk")
            .is_some_and(|v| v == "true");

        let boundary_shape = BoundaryShape::BuildVolumeBox {
            min: boundary_min,
            max: boundary_max,
        };
        let common_family_alg_specs = SupportStructureSpecs {
            gravity,
            boundary_shape: boundary_shape.clone(),
            print_plate: PrintPlate {
                boundary: boundary_shape,
                plate_normal: Vector3D::new([0.0, 0.0, 1.0]),
            },
            explicit_locations: anchors,
            allow_expand_touch,
            allow_expand_trunk,
        };
        Ok((
            common_family_alg_specs,
            TrivialSupportSpecs {
                radius_mm,
                // `artificial_delay_ms` is an example of what is not part of the recipe:
                // Because it does not affect reproducibility of the result:
                // A replay never needs to re-wait
                // artificial_delay_ms: 0,
            },
        ))
    }
}

fn get<'a>(
    recipe: &'a JobRecipe,
    key: &'static str,
) -> Result<&'a str, GeomAlgorithmError<SupportAlgError>> {
    recipe
        .get(key)
        .ok_or_else(|| GeomAlgorithmError::MalformedRecipe(format!("missing key '{key}'")))
}

fn build_recipe(
    algorithm_id: AlgorithmId,
    algorithm_version: String,
    common_family_alg_specs: &SupportStructureSpecs,
    specific_alg_specs: &TrivialSupportSpecs,
    anchors: &[Vector3D<f64>],
    input_mesh_hash: String,
) -> Result<JobRecipe, GeomAlgorithmError<SupportAlgError>> {
    let (boundary_min, boundary_max) = match &common_family_alg_specs.boundary_shape {
        BoundaryShape::BuildVolumeBox { min, max } => (*min, *max),
        BoundaryShape::Mesh(_) => {
            return Err(GeomAlgorithmError::FamilySpecific(
                SupportAlgError::UnsupportedBoundaryShape,
            ));
        }
    };
    Ok(JobRecipe {
        algorithm_id,
        algorithm_version,
        resolved_settings: vec![
            (
                "support.gravity".to_string(),
                format_point(common_family_alg_specs.gravity),
            ),
            (
                "support.boundary.min".to_string(),
                format_point(boundary_min),
            ),
            (
                "support.boundary.max".to_string(),
                format_point(boundary_max),
            ),
            ("support.anchors".to_string(), format_points(anchors)),
            (
                "support.radius_mm".to_string(),
                specific_alg_specs.radius_mm.to_string(),
            ),
            (
                "support.allow_expand_touch".to_string(),
                common_family_alg_specs.allow_expand_touch.to_string(),
            ),
            (
                "support.allow_expand_trunk".to_string(),
                common_family_alg_specs.allow_expand_trunk.to_string(),
            ),
        ],
        input_mesh_hash,
        seeds: Vec::new(),
    })
}

fn format_point(p: Vector3D<f64>) -> String {
    let c = p.coords();
    format!("{},{},{}", c[0], c[1], c[2])
}

fn format_points(points: &[Vector3D<f64>]) -> String {
    points
        .iter()
        .map(|p| format_point(*p))
        .collect::<Vec<_>>()
        .join(";")
}

fn parse_point(s: &str) -> Result<Vector3D<f64>, GeomAlgorithmError<SupportAlgError>> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 3 {
        return Err(GeomAlgorithmError::MalformedRecipe(format!(
            "expected 3 comma-separated components, got '{s}'"
        )));
    }
    let mut coords = [0.0_f64; 3];
    for (i, part) in parts.iter().enumerate() {
        coords[i] = part.parse().map_err(|_| {
            GeomAlgorithmError::MalformedRecipe(format!("'{part}' is not a number"))
        })?;
    }
    Ok(Vector3D::new(coords))
}

fn parse_points(s: &str) -> Result<Vec<Vector3D<f64>>, GeomAlgorithmError<SupportAlgError>> {
    if s.is_empty() {
        return Ok(Vec::new());
    }
    s.split(';').map(parse_point).collect()
}

fn hash_aabb(aabb: Option<Aabb>) -> String {
    let mut hasher = Sha256::new();
    match aabb {
        Some(aabb) => {
            for c in aabb.min().coords() {
                hasher.update(c.to_le_bytes());
            }
            for c in aabb.max().coords() {
                hasher.update(c.to_le_bytes());
            }
        }
        None => hasher.update(b"empty"),
    }
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The AABB corner furthest along `gravity_dir`: The "lowest" point
/// of the scene's bounding box in the direction things fall. A v0
/// stand-in for real overhang detection: cheap, deterministic, and
/// good enough to prove the auto-detect-then-record-into-the-recipe
/// wiring without pretending to be a structural analysis.
fn lowest_point_along(aabb: Aabb, gravity_dir: Vector3D<f64>) -> Vector3D<f64> {
    let min = aabb.min().coords();
    let max = aabb.max().coords();
    let g = gravity_dir.coords();

    let mut best = [0.0_f64; 3];
    let mut best_dot = f64::NEG_INFINITY;
    for &x in &[min[0], max[0]] {
        for &y in &[min[1], max[1]] {
            for &z in &[min[2], max[2]] {
                let candidate = [f64::from(x), f64::from(y), f64::from(z)];
                let dot = candidate[0] * g[0] + candidate[1] * g[1] + candidate[2] * g[2];
                if dot > best_dot {
                    best_dot = dot;
                    best = candidate;
                }
            }
        }
    }
    Vector3D::new(best)
}

fn estimate_prism_material_g(height_mm: f64, radius_mm: f64) -> f64 {
    // Regular-hexagon cross-section area with circumradius `radius_mm`.
    let area_mm2 = 1.5 * 3.0_f64.sqrt() * radius_mm * radius_mm;
    area_mm2 * height_mm * RESIN_DENSITY_G_PER_MM3
}

/// A watertight hexagonal-prism strut from `top` to `bottom`, with the
/// hexagon's plane perpendicular to `axis` (assumed a unit vector).
/// See note on "Strut" in support-gen-trivial/src/trivial_support.rs
///
/// Returns `None`, building nothing, when `top` and `bottom` are
/// closer than [`MIN_STRUT_LENGTH_MM`]: this is the *only* place
/// that decides whether a given anchor actually needs a support
/// structure at all: callers pass through whatever anchor/boundary
/// pair they resolved and react to the answer, they don't pre-judge
/// it (e.g. by looking at how small the boundary box is) themselves.
pub(crate) fn hex_prism(
    top: Vector3D<f64>,
    bottom: Vector3D<f64>,
    radius: f64,
    axis: Vector3D<f64>,
) -> Option<IndexedMesh> {
    if top.diff(&bottom).norm() < MIN_STRUT_LENGTH_MM {
        return None;
    }

    let helper = if axis.coords()[0].abs() < 0.9 {
        Vector3D::new([1.0, 0.0, 0.0])
    } else {
        Vector3D::new([0.0, 1.0, 0.0])
    };
    let u = normalize(axis.cross_product(&helper)).unwrap_or(Vector3D::new([1.0, 0.0, 0.0]));
    let v = axis.cross_product(&u);

    let ring = |center: Vector3D<f64>| -> Vec<Vertex> {
        (0..SIDES)
            .map(|k| {
                let theta = 2.0 * PI * (k as f64) / (SIDES as f64);
                let (cu, cv) = (theta.cos() * radius, theta.sin() * radius);
                let c = center.coords();
                let uu = u.coords();
                let vv = v.coords();
                Vertex::new([
                    (c[0] + uu[0] * cu + vv[0] * cv) as f32,
                    (c[1] + uu[1] * cu + vv[1] * cv) as f32,
                    (c[2] + uu[2] * cu + vv[2] * cv) as f32,
                ])
            })
            .collect()
    };

    let mut vertices = ring(top);
    vertices.extend(ring(bottom));

    let mut faces = Vec::with_capacity(SIDES * 2 + (SIDES - 2) * 2);
    for k in 0..SIDES {
        let k2 = (k + 1) % SIDES;
        push_face(&mut faces, k, SIDES + k, SIDES + k2);
        push_face(&mut faces, k, SIDES + k2, k2);
    }
    for k in 1..SIDES - 1 {
        push_face(&mut faces, 0, k, k + 1);
    }
    for k in 1..SIDES - 1 {
        push_face(&mut faces, SIDES, SIDES + k + 1, SIDES + k);
    }

    Some(IndexedMesh::new(
        Some(MeshMetadata {
            name: Some("trivial-support-v0".to_string()),
            header_comment: None,
            declared_facet_normals: None,
        }),
        vertices,
        faces,
    ))
}

fn push_face(faces: &mut Vec<IndexedTriangle>, a: usize, b: usize, c: usize) {
    faces.push(IndexedTriangle::from_raw_indices([
        a as u32, b as u32, c as u32,
    ]));
}
