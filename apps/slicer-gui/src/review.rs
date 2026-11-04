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

/// Each check of a review by itself on the same regions, which the review
/// runs together (#427): how long they take with the machine to themselves.
#[cfg(test)]
pub(crate) fn review_checks_alone(
    leaves: &[LeafSlices],
    filled: Option<&ContourStack>,
    erased: Option<&ContourStack>,
    profile: &SliceProfile,
    raft_top_mm: Option<f64>,
) -> Vec<(&'static str, Duration)> {
    use sc_review::checks::{islands, output, raft, thin, traps};
    use sc_review::{PlateRegions, ReviewContext};
    let leaves = measured_leaves(leaves, filled, erased);
    let input = measured_input(leaves, profile, raft_top_mm);
    let options = ReviewOptions::default();
    let timed = |f: &mut dyn FnMut()| {
        let started = std::time::Instant::now();
        f();
        started.elapsed()
    };
    let mut regions = None;
    let mut times = vec![(
        "regions",
        timed(&mut || regions = Some(PlateRegions::compute(&input))),
    )];
    let regions = regions.expect("computed");
    let ctx = ReviewContext::new(&input, &regions);
    times.push((
        "islands",
        timed(&mut || drop(islands::run_with_joins(&ctx, &options.islands))),
    ));
    times.push((
        "traps",
        timed(&mut || drop(traps::run_with_fills(&ctx, &options.traps))),
    ));
    times.push((
        "thin",
        timed(&mut || drop(thin::run_with_thickenings(&ctx, &options.thin))),
    ));
    times.push((
        "output",
        timed(&mut || drop(output::run_with_erasures(&ctx, &options.output))),
    ));
    times.push((
        "raft",
        timed(&mut || drop(raft::clashes(&ctx, &options.raft))),
    ));
    times
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
