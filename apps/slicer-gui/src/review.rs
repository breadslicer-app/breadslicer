/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The pre-flight review of a slice (#90), run on the slice worker right
//! after the engine finishes and before the per-leaf stacks are merged:
//! `sc_review` reads every leaf's contours with its role, model or support.
//!
//! Cavities the user marked to fill (#132) arrive as one more stack, cut on
//! the slice's schedule ([`sc_review::fill_stack`]): it is reviewed as model
//! solid here, and merged with the plate's stacks for the preview and the
//! printer file, so all three see the same cured pixels.
//!
//! Specks marked to erase by Fix All (#156) arrive the same way, as one more
//! stack ([`erased_stack`]): cut from the model leaves and the fills, it
//! cancels them inside the specks' pixels when merged with them, so the
//! review, the preview and the printer file all see the specks gone. The
//! model meshes are not touched.
//!
//! A review never fails the slice. An input the review refuses, or a panic
//! inside a check, becomes [`ReviewOutcome::Failed`], and the slice lands as
//! usual.

use std::panic::AssertUnwindSafe;
use std::time::Duration;

use sc_geom::df_supports::{is_support_mesh_name, recommended_lift_mm};
use sc_review::{
    CavityFill, Finding, ReviewInput, ReviewLeaf, ReviewOptions, Role, erase_stack, review_with,
};
use sc_sl1cer::LeafSlices;
use sc_sl1cer::contour::ContourStack;
use sc_sl1cer::profile::{LayerSchedule, SliceProfile};

use crate::mesh_loader::panic_message;

/// What the review of one slice gave.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ReviewOutcome {
    /// The checks ran.
    Reviewed {
        /// Every finding, in listing order (layer, then check).
        findings: Vec<Finding>,
        /// The pixels of the cavities offered a fill, by the index their
        /// fix names.
        fills: Vec<CavityFill>,
        /// The pixels that erase each floating speck, by the index its fix
        /// names (#156).
        erasures: Vec<CavityFill>,
        /// The pixels that thicken each thin model feature, by the index
        /// its fix names (#156).
        thickenings: Vec<CavityFill>,
        /// The pixels that join each island to the model just below it, by
        /// the index its fix names (#235).
        joins: Vec<CavityFill>,
        /// How long the review took, layer regions included.
        took: Duration,
    },
    /// The review could not run, for this reason; the slice is unaffected.
    Failed(String),
}

/// The role of a sliced leaf: generated supports carry their own name, and
/// every other leaf is a model.
pub(crate) fn role_of(name: Option<&str>) -> Role {
    if name.is_some_and(is_support_mesh_name) {
        Role::Support
    } else {
        Role::Model
    }
}

/// Every sliced leaf with its role, in the engine's order.
pub(crate) fn review_leaves(leaves: &[LeafSlices]) -> Vec<ReviewLeaf<'_>> {
    leaves
        .iter()
        .map(|slices| ReviewLeaf::from_slices(slices, role_of(slices.leaf_name())))
        .collect()
}

/// The stack that erases `erasures` (#156) from the model leaves of
/// `leaves` and from `filled`, all cut on `schedule`; `None` when nothing is
/// erased. Supports are never erased: a new support may run through where
/// a speck was.
pub(crate) fn erased_stack<'e>(
    leaves: &[LeafSlices],
    filled: Option<&ContourStack>,
    erasures: impl IntoIterator<Item = &'e CavityFill>,
    schedule: &LayerSchedule,
) -> Option<ContourStack> {
    let models: Vec<&ContourStack> = leaves
        .iter()
        .filter(|slices| role_of(slices.leaf_name()) == Role::Model)
        .map(LeafSlices::stack)
        .chain(filled)
        .collect();
    erase_stack(erasures, &models, schedule)
}

/// Reviews the leaves of one slicing run, cut with `profile`: its layer
/// schedule, its screen and its maximum height. `filled` is the stack of the
/// cavities filled on this slice, and `erased` the one that erases the
/// specks ([`erased_stack`]), both reviewed as model. The supports stand on
/// a raft whose top is `raft_top_mm` above the plate, or on none (#131).
pub(crate) fn review_slices(
    leaves: &[LeafSlices],
    filled: Option<&ContourStack>,
    erased: Option<&ContourStack>,
    profile: &SliceProfile,
    raft_top_mm: Option<f64>,
) -> ReviewOutcome {
    guarded(|| {
        let mut leaves = review_leaves(leaves);
        if let Some(stack) = filled {
            leaves.push(ReviewLeaf::new(Some(FILLED_LEAF), Role::Model, stack));
        }
        if let Some(stack) = erased {
            leaves.push(ReviewLeaf::new(Some(ERASED_LEAF), Role::Model, stack));
        }
        let input = ReviewInput::from_profile(leaves, profile)
            .map_err(|err| err.to_string())?
            .with_raft_top_mm(raft_top_mm)
            .with_support_lift_mm(Some(recommended_lift_mm(None)));
        let report = review_with(&input, &ReviewOptions::default());
        Ok(ReviewOutcome::Reviewed {
            findings: report.findings,
            fills: report.fills,
            erasures: report.erasures,
            thickenings: report.thickenings,
            joins: report.joins,
            took: report.timings.total,
        })
    })
}

/// [`review_slices`] with the review's own timings (#427).
#[cfg(test)]
pub(crate) fn review_slices_measured(
    leaves: &[LeafSlices],
    filled: Option<&ContourStack>,
    erased: Option<&ContourStack>,
    profile: &SliceProfile,
    raft_top_mm: Option<f64>,
) -> (ReviewOutcome, sc_review::ReviewTimings) {
    let leaves = measured_leaves(leaves, filled, erased);
    let input = measured_input(leaves, profile, raft_top_mm);
    let report = review_with(&input, &ReviewOptions::default());
    let timings = report.timings;
    (
        ReviewOutcome::Reviewed {
            findings: report.findings,
            fills: report.fills,
            erasures: report.erasures,
            thickenings: report.thickenings,
            joins: report.joins,
            took: report.timings.total,
        },
        timings,
    )
}

/// Each procedure of a review by itself (#427), on the same input, each
/// through `step`: what it costs with the machine to itself.
#[cfg(test)]
pub(crate) fn review_procedures_alone(
    leaves: &[LeafSlices],
    filled: Option<&ContourStack>,
    erased: Option<&ContourStack>,
    profile: &SliceProfile,
    raft_top_mm: Option<f64>,
    step: &mut dyn FnMut(&'static str, &mut dyn FnMut()),
) {
    use sc_review::checks::{islands, output, raft, thin, traps};
    use sc_review::{PlateRegions, ReviewContext};
    let mut input = None;
    step("input", &mut || {
        let leaves = measured_leaves(leaves, filled, erased);
        input = Some(measured_input(leaves, profile, raft_top_mm));
    });
    let input = input.expect("built");
    let options = ReviewOptions::default();
    let mut regions = None;
    step("regions", &mut || {
        regions = Some(PlateRegions::compute(&input));
    });
    let regions = regions.expect("computed");
    let ctx = ReviewContext::new(&input, &regions);
    step("islands", &mut || {
        drop(islands::run_with_joins(&ctx, &options.islands));
    });
    step("traps", &mut || {
        drop(traps::run_with_fills(&ctx, &options.traps));
    });
    step("thin", &mut || {
        drop(thin::run_with_thickenings(&ctx, &options.thin));
    });
    step("output", &mut || {
        drop(output::run_with_erasures(&ctx, &options.output));
    });
    step("raft", &mut || drop(raft::clashes(&ctx, &options.raft)));
}

/// A way to run a review, to see what it needs (#427).
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReviewVariant {
    /// As the app runs it.
    Full,
    /// With the thin check on the models only.
    ThinModelOnly,
    /// Without the supports at all.
    ModelOnly,
    /// With the supports' raft and nothing else of them.
    ModelAndRaft,
    /// With the fills as traced outlines instead of rectangles.
    TracedFill,
    /// Without the fills.
    NoFill,
}

/// The review of `variant`, with the rings of a traced fill and how many of
/// its sampled layers rasterise differently from the rectangles.
#[cfg(test)]
pub(crate) fn review_variant(
    variant: ReviewVariant,
    leaves: &[LeafSlices],
    filled: Option<&ContourStack>,
    erased: Option<&ContourStack>,
    profile: &SliceProfile,
    raft_top_mm: Option<f64>,
) -> (sc_review::ReviewReport, usize, usize) {
    use sc_sl1cer::contour::ContourLayer;
    let pixel_map =
        *measured_input(measured_leaves(leaves, None, None), profile, raft_top_mm).pixel_map();
    let (traced, mismatches, points) = match (variant, filled) {
        (ReviewVariant::TracedFill, Some(stack)) => {
            let (traced, mismatches, points) = traced_stack(stack, &pixel_map);
            (Some(traced), mismatches, points)
        }
        _ => (None, 0, 0),
    };
    let filled = match variant {
        ReviewVariant::NoFill => None,
        ReviewVariant::TracedFill => traced.as_ref().or(filled),
        _ => filled,
    };
    // The supports below the raft's top, and no others.
    let raft_only: Vec<ContourStack> = leaves
        .iter()
        .filter(|leaf| role_of(leaf.leaf_name()) == Role::Support)
        .map(|leaf| {
            let top = raft_top_mm.unwrap_or(0.0);
            ContourStack::new_unchecked(
                leaf.stack()
                    .layers()
                    .iter()
                    .map(|layer| {
                        if layer.z_mm() <= top {
                            layer.clone()
                        } else {
                            ContourLayer::new(layer.z_mm(), Vec::new())
                        }
                    })
                    .collect(),
            )
        })
        .collect();
    let mut list = measured_leaves(leaves, filled, erased);
    match variant {
        ReviewVariant::ModelOnly => list.retain(|leaf| leaf.role == Role::Model),
        ReviewVariant::ModelAndRaft => {
            list.retain(|leaf| leaf.role == Role::Model);
            list.extend(
                raft_only
                    .iter()
                    .map(|stack| ReviewLeaf::new(Some("Raft"), Role::Support, stack)),
            );
        }
        _ => {}
    }
    let input = measured_input(list, profile, raft_top_mm);
    let mut options = ReviewOptions::default();
    if variant == ReviewVariant::ThinModelOnly {
        options.thin.support_width_px = 0.0;
    }
    (review_with(&input, &options), mismatches, points)
}

/// `stack` with each layer's pixels as outlines: a ring per boundary, a hole
/// wound against its outer ring, instead of a polygon per rectangle. Returns
/// it, how many layers of a sample rasterise differently, and its points.
#[cfg(test)]
fn traced_stack(
    stack: &ContourStack,
    pixel_map: &sc_sl1cer::raster::pixel_map::PixelMap,
) -> (ContourStack, usize, usize) {
    use sc_review::runs::runs_from_raster;
    use sc_sl1cer::contour::ContourLayer;
    use sc_sl1cer::raster::scanline::rasterize_layer_binary;
    let (mut mismatches, mut points) = (0, 0);
    let layers: Vec<ContourLayer> = stack
        .layers()
        .iter()
        .enumerate()
        .map(|(index, layer)| {
            if layer.polygons().is_empty() {
                return ContourLayer::new(layer.z_mm(), Vec::new());
            }
            let runs = runs_from_raster(&rasterize_layer_binary(layer, pixel_map));
            let polygons = traced_rings(&runs, pixel_map);
            points += polygons.iter().map(|p| p.outer().len()).sum::<usize>();
            let traced = ContourLayer::new(layer.z_mm(), polygons);
            if index % 50 == 0
                && runs_from_raster(&rasterize_layer_binary(&traced, pixel_map)) != runs
            {
                mismatches += 1;
            }
            traced
        })
        .collect();
    (ContourStack::new_unchecked(layers), mismatches, points)
}

/// The boundary of `runs`, sorted by row, as closed rings of pixel corners.
#[cfg(test)]
fn traced_rings(
    runs: &[sc_review::Run],
    pixel_map: &sc_sl1cer::raster::pixel_map::PixelMap,
) -> Vec<sc_sl1cer::contour::Polygon> {
    use std::collections::HashMap;

    use sc_review::input::pixel_to_plate;
    use sc_sl1cer::contour::{ClosedRing, Polygon};
    use sc_sl1cer::geom2d::Point2D;

    /// The parts of `spans` that `cover` leaves out.
    fn uncovered(spans: &[sc_review::Run], cover: &[sc_review::Run]) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        for span in spans {
            let mut from = span.x0;
            for other in cover {
                if other.x1 <= from {
                    continue;
                }
                if other.x0 >= span.x1 {
                    break;
                }
                if other.x0 > from {
                    out.push((from, other.x0));
                }
                from = from.max(other.x1);
            }
            if from < span.x1 {
                out.push((from, span.x1));
            }
        }
        out
    }

    let rows: Vec<&[sc_review::Run]> = runs.chunk_by(|a, b| a.row == b.row).collect();
    let mut next: HashMap<(u32, u32), Vec<(u32, u32)>> = HashMap::new();
    let mut edge = |from: (u32, u32), to: (u32, u32)| next.entry(from).or_default().push(to);
    for (i, row) in rows.iter().enumerate() {
        let y = row[0].row;
        let above: &[sc_review::Run] = match i.checked_sub(1).map(|k| rows[k]) {
            Some(r) if r[0].row + 1 == y => r,
            _ => &[],
        };
        let below: &[sc_review::Run] = match rows.get(i + 1) {
            Some(r) if r[0].row == y + 1 => r,
            _ => &[],
        };
        for run in *row {
            edge((run.x0, y + 1), (run.x0, y));
            edge((run.x1, y), (run.x1, y + 1));
        }
        for (a, b) in uncovered(row, above) {
            edge((a, y), (b, y));
        }
        for (a, b) in uncovered(row, below) {
            edge((b, y + 1), (a, y + 1));
        }
    }
    let mut polygons = Vec::new();
    while let Some(&start) = next.keys().next() {
        let mut ring = vec![start];
        let mut at = start;
        loop {
            let outgoing = next.get_mut(&at).expect("a boundary closes");
            let to = outgoing.pop().expect("a boundary closes");
            if outgoing.is_empty() {
                next.remove(&at);
            }
            at = to;
            if at == start {
                break;
            }
            ring.push(at);
        }
        // Only the corners.
        let count = ring.len();
        let corners: Vec<Point2D> = (0..count)
            .filter(|&i| {
                let (a, b, c) = (
                    ring[(i + count - 1) % count],
                    ring[i],
                    ring[(i + 1) % count],
                );
                !((a.0 == b.0 && b.0 == c.0) || (a.1 == b.1 && b.1 == c.1))
            })
            .map(|i| {
                let [x, y] =
                    pixel_to_plate(pixel_map, [f64::from(ring[i].0), f64::from(ring[i].1)]);
                Point2D::new(x, y)
            })
            .collect();
        polygons.push(Polygon::new_unchecked(ClosedRing::new(corners), Vec::new()));
    }
    polygons
}

#[cfg(test)]
fn measured_leaves<'a>(
    leaves: &'a [LeafSlices],
    filled: Option<&'a ContourStack>,
    erased: Option<&'a ContourStack>,
) -> Vec<ReviewLeaf<'a>> {
    let mut leaves = review_leaves(leaves);
    if let Some(stack) = filled {
        leaves.push(ReviewLeaf::new(Some(FILLED_LEAF), Role::Model, stack));
    }
    if let Some(stack) = erased {
        leaves.push(ReviewLeaf::new(Some(ERASED_LEAF), Role::Model, stack));
    }
    leaves
}

#[cfg(test)]
fn measured_input<'a>(
    leaves: Vec<ReviewLeaf<'a>>,
    profile: &SliceProfile,
    raft_top_mm: Option<f64>,
) -> ReviewInput<'a> {
    ReviewInput::from_profile(leaves, profile)
        .expect("the stacks fit the run")
        .with_raft_top_mm(raft_top_mm)
        .with_support_lift_mm(Some(recommended_lift_mm(None)))
}

/// The name of the filled cavities' leaf in the review.
const FILLED_LEAF: &str = "Filled cavities";

/// The name of the erased specks' leaf in the review.
const ERASED_LEAF: &str = "Erased specks";

/// Runs `review`, turning its error or its panic into
/// [`ReviewOutcome::Failed`].
fn guarded(review: impl FnOnce() -> Result<ReviewOutcome, String>) -> ReviewOutcome {
    match std::panic::catch_unwind(AssertUnwindSafe(review)) {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(message)) => ReviewOutcome::Failed(message),
        Err(payload) => ReviewOutcome::Failed(panic_message(&payload)),
    }
}

#[cfg(test)]
mod review_tests {
    use std::sync::Arc;

    use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
    use sc_mesh_fixtures::unit_cube_mesh;
    use sc_review::CheckKind;
    use sc_scene::{LeafPayload, Scene, SceneMetadata};
    use sc_sl1cer::contour::merge_stacks;
    use sc_sl1cer::profile::{
        AaLevel, BuildVolume, LayerSchedule, Mirror, PackingMode, PixelResolution,
    };
    use sc_sl1cer::raster::pixel_map::PixelMap;
    use sc_sl1cer::raster::scanline::rasterize_layer_binary;
    use sc_sl1cer::slice_scene;

    use super::*;

    /// A 20 x 20 mm plate of 0.1 mm pixels, sliced in `layers` layers of
    /// 0.25 mm.
    fn profile(layers: u32) -> SliceProfile {
        SliceProfile::new(
            BuildVolume::new(20.0, 20.0, 20.0).expect("a valid volume"),
            PixelResolution::new(200, 200).expect("a valid resolution"),
            LayerSchedule::uniform(0.25, 0.25, layers).expect("a valid schedule"),
            Mirror::default(),
            AaLevel::Off,
            PackingMode::None,
        )
    }

    fn named(name: &str) -> Option<SceneMetadata> {
        Some(SceneMetadata {
            name: Some(name.to_owned()),
            ..Default::default()
        })
    }

    /// Adds a 1 mm cube named `name` whose bottom centre is at `at`.
    fn add_cube(scene: &mut Scene, name: &str, at: [f64; 3]) {
        let leaf = scene
            .add_leaf(
                named(name),
                Transform3D::Affine(AffineTransform3D::from_translation([
                    at[0],
                    at[1],
                    at[2] + 0.5,
                ])),
                LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
            )
            .expect("inline-mesh leaf");
        scene.add_root(leaf).expect("fresh leaf becomes a root");
    }

    /// A model cube on the plate and a support-named cube beside it.
    fn model_and_support() -> Scene {
        let mut scene = Scene::new();
        add_cube(&mut scene, "Wolf", [0.0, 0.0, 0.0]);
        add_cube(&mut scene, "Supports", [3.0, 0.0, 0.0]);
        scene
    }

    #[test]
    fn support_names_are_supports_and_the_rest_models() {
        assert_eq!(role_of(Some("Supports")), Role::Support);
        assert_eq!(role_of(Some("Supports (DragonFruit)")), Role::Support);
        assert_eq!(role_of(Some("Wolf")), Role::Model);
        assert_eq!(role_of(Some("supports")), Role::Model);
        assert_eq!(role_of(None), Role::Model);
    }

    #[test]
    fn every_sliced_leaf_is_reviewed_with_its_role() {
        let leaves = slice_scene(&model_and_support(), &profile(4)).expect("the scene slices");
        let reviewed = review_leaves(&leaves);
        let roles: Vec<(Option<&str>, Role)> = reviewed.iter().map(|l| (l.name, l.role)).collect();
        assert_eq!(
            roles,
            [
                (Some("Wolf"), Role::Model),
                (Some("Supports"), Role::Support)
            ]
        );
        for (leaf, slices) in reviewed.iter().zip(&leaves) {
            assert!(std::ptr::eq(leaf.stack, slices.stack()));
        }
    }

    #[test]
    fn a_plate_standing_on_the_build_plate_reviews_clean() {
        let leaves = slice_scene(&model_and_support(), &profile(4)).expect("the scene slices");
        match review_slices(&leaves, None, None, &profile(4), None) {
            ReviewOutcome::Reviewed { findings, .. } => {
                assert!(findings.is_empty(), "two cubes on the plate: {findings:?}")
            }
            ReviewOutcome::Failed(why) => panic!("the review failed: {why}"),
        }
    }

    #[test]
    fn a_cube_in_mid_air_is_found() {
        let mut scene = Scene::new();
        add_cube(&mut scene, "Wolf", [0.0, 0.0, 0.0]);
        add_cube(&mut scene, "Floating", [4.0, 0.0, 2.0]);
        let leaves = slice_scene(&scene, &profile(12)).expect("the scene slices");
        let ReviewOutcome::Reviewed { findings, .. } =
            review_slices(&leaves, None, None, &profile(12), None)
        else {
            panic!("the review failed");
        };
        assert!(
            findings
                .iter()
                .any(|f| f.first_layer == 8 && f.check == CheckKind::Island),
            "{findings:?}"
        );
    }

    #[test]
    fn stacks_that_do_not_fit_the_profile_fail_the_review_only() {
        let leaves = slice_scene(&model_and_support(), &profile(4)).expect("the scene slices");
        let outcome = review_slices(&leaves, None, None, &profile(8), None);
        let ReviewOutcome::Failed(why) = outcome else {
            panic!("a 4-layer slice reviewed on 8 layers: {outcome:?}");
        };
        assert!(why.contains("8"), "{why}");
    }

    /// A closed `outer` mm cube standing on the plate, hollowed by a sealed
    /// `inner` mm cubic cavity at its centre: two shells of one mesh, the
    /// inner one turned inside out.
    fn hollow_cube(outer: f64, inner: f64) -> Scene {
        use sc_mesh_core::{IndexedMesh, IndexedTriangle, Vertex};
        // The faces of `unit_cube_mesh`, outward.
        const FACES: [[u32; 3]; 12] = [
            [0, 3, 2],
            [0, 2, 1],
            [4, 5, 6],
            [4, 6, 7],
            [3, 7, 6],
            [3, 6, 2],
            [0, 1, 5],
            [0, 5, 4],
            [1, 2, 6],
            [1, 6, 5],
            [0, 4, 7],
            [0, 7, 3],
        ];
        let cube = unit_cube_mesh();
        let mut vertices: Vec<Vertex> = Vec::new();
        let mut faces: Vec<IndexedTriangle> = Vec::new();
        for (scale, flip) in [(outer, false), (inner, true)] {
            let base = vertices.len() as u32;
            vertices.extend(
                cube.vertices
                    .iter()
                    .map(|v| Vertex::new(v.coords().map(|c| c * scale as f32))),
            );
            faces.extend(FACES.iter().map(|&[a, b, c]| {
                let [a, b, c] = [a + base, b + base, c + base];
                IndexedTriangle::from_raw_indices(if flip { [a, c, b] } else { [a, b, c] })
            }));
        }
        let mesh = IndexedMesh {
            meta: None,
            vertices,
            faces,
        };
        let mut scene = Scene::new();
        let leaf = scene
            .add_leaf(
                named("Hollow"),
                Transform3D::Affine(AffineTransform3D::from_translation([0.0, 0.0, outer / 2.0])),
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .expect("inline-mesh leaf");
        scene.add_root(leaf).expect("fresh leaf becomes a root");
        scene
    }

    /// The lit pixels of layer `layer` of `stack`, on the profile's screen.
    fn lit_pixels(stack: &ContourStack, layer: usize, profile: &SliceProfile) -> u64 {
        let volume = profile.build_volume();
        let map = PixelMap::new(
            volume.x(),
            volume.y(),
            *profile.pixel_resolution(),
            profile.mirror(),
        );
        rasterize_layer_binary(&stack.layers()[layer], &map)
            .runs()
            .iter()
            .filter(|run| run.value != 0)
            .map(|run| u64::from(run.length))
            .sum()
    }

    #[test]
    fn a_filled_cavity_is_cured_in_the_plate_and_gone_from_the_review() {
        // A 6 mm cube around a 2 mm cavity (8 mm³, a medium trap): layers
        // 8-15 of 24 hold it.
        let profile = profile(24);
        let leaves = slice_scene(&hollow_cube(6.0, 2.0), &profile).expect("the scene slices");
        let ReviewOutcome::Reviewed {
            findings, fills, ..
        } = review_slices(&leaves, None, None, &profile, None)
        else {
            panic!("the review failed");
        };
        let trap = findings
            .iter()
            .find(|f| f.check == CheckKind::ResinTrap)
            .unwrap_or_else(|| panic!("no trap in {findings:?}"));
        assert_eq!(trap.severity, sc_review::Severity::Warning);
        assert_eq!((trap.first_layer, trap.last_layer), (8, 15));
        let Some(sc_review::SuggestedFix::Fill { cavity, .. }) = trap.fix else {
            panic!("a medium trap is filled, got {:?}", trap.fix);
        };
        let filled = sc_review::fill_stack([&fills[cavity]], profile.layer_schedule())
            .expect("the cavity has pixels");

        // Reviewed with the fill, the trap is gone.
        let ReviewOutcome::Reviewed { findings, .. } =
            review_slices(&leaves, Some(&filled), None, &profile, None)
        else {
            panic!("the review failed");
        };
        assert!(
            findings.iter().all(|f| f.check != CheckKind::ResinTrap),
            "{findings:?}"
        );

        // Merged as the slice run merges it, the cavity's layers are solid:
        // the 60 x 60 pixel square, without its 20 x 20 pixel hole.
        let plain = merge_stacks(leaves.iter().map(|l| l.stack().clone()).collect()).unwrap();
        let merged = merge_stacks(
            leaves
                .iter()
                .map(|l| l.stack().clone())
                .chain([filled])
                .collect(),
        )
        .unwrap();
        assert_eq!(lit_pixels(&plain, 12, &profile), 3600 - 400);
        assert_eq!(lit_pixels(&merged, 12, &profile), 3600);
        assert_eq!(lit_pixels(&merged, 20, &profile), 3600);
        for layer in 0..24 {
            assert_eq!(
                lit_pixels(&merged, layer, &profile),
                3600,
                "layer {layer} is not solid"
            );
        }
    }

    #[test]
    fn a_panicking_check_fails_the_review_without_unwinding() {
        let outcome = guarded(|| panic!("a check fell over"));
        assert_eq!(
            outcome,
            ReviewOutcome::Failed("a check fell over".to_owned())
        );
    }
}
