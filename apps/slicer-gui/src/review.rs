/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The pre-flight review of a slice (#90), run on the slice worker right
//! after the engine finishes and before the per-leaf stacks are merged:
//! `sc_review` reads every leaf's contours with its role, model or support.
//!
//! A review never fails the slice. An input the review refuses, or a panic
//! inside a check, becomes [`ReviewOutcome::Failed`], and the slice lands as
//! usual.

use std::panic::AssertUnwindSafe;
use std::time::Duration;

use sc_geom::df_supports::is_support_mesh_name;
use sc_review::{Finding, ReviewInput, ReviewLeaf, ReviewOptions, Role, review_with};
use sc_sl1cer::LeafSlices;
use sc_sl1cer::profile::SliceProfile;

use crate::mesh_loader::panic_message;

/// What the review of one slice gave.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ReviewOutcome {
    /// The checks ran.
    Reviewed {
        /// Every finding, in listing order (layer, then check).
        findings: Vec<Finding>,
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

/// Reviews the leaves of one slicing run, cut with `profile`: its layer
/// schedule, its screen and its maximum height; the supports stand on a raft
/// whose top is `raft_top_mm` above the plate, or on none (#131).
pub(crate) fn review_slices(
    leaves: &[LeafSlices],
    profile: &SliceProfile,
    raft_top_mm: Option<f64>,
) -> ReviewOutcome {
    guarded(|| {
        let input = ReviewInput::from_profile(review_leaves(leaves), profile)
            .map_err(|err| err.to_string())?
            .with_raft_top_mm(raft_top_mm);
        let report = review_with(&input, &ReviewOptions::default());
        Ok(ReviewOutcome::Reviewed {
            findings: report.findings,
            took: report.timings.total,
        })
    })
}

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
    use sc_sl1cer::profile::{
        AaLevel, BuildVolume, LayerSchedule, Mirror, PackingMode, PixelResolution,
    };
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
        match review_slices(&leaves, &profile(4), None) {
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
        let ReviewOutcome::Reviewed { findings, .. } = review_slices(&leaves, &profile(12), None)
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
        let outcome = review_slices(&leaves, &profile(8), None);
        let ReviewOutcome::Failed(why) = outcome else {
            panic!("a 4-layer slice reviewed on 8 layers: {outcome:?}");
        };
        assert!(why.contains("8"), "{why}");
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
