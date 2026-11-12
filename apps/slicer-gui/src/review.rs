/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The pre-flight review of a slice (#90), run on the slice worker right
//! after the engine finishes and before the per-leaf stacks are merged.
//!
//! `sc_review` reads the model leaves' contours. Supports made here enter as
//! data, what the graph knows of their tips (#432): their geometry is never
//! read, so the review's cost grows with the model and not with them. A
//! support leaf no session drew has no such data, and is reviewed as a model.
//!
//! Cavities the user marked to fill (#132) arrive as one more stack, cut on
//! the slice's schedule ([`sc_review::fill_stack`]): it is reviewed as model
//! solid here, and merged with the plate's stacks for the preview and the
//! printer file, so all three see the same cured pixels.
//!
//! The review gives back what it took off the layers by itself, stray pixels
//! and debris (#432): [`erased_stack`], cut from the model leaves and the
//! fills, cancels them inside those pixels when merged with them, so the
//! preview and the printer file see them gone on this same slice. The model
//! meshes are not touched.
//!
//! A review never fails the slice. An input the review refuses, or a panic
//! inside a check, becomes [`ReviewOutcome::Failed`], and the slice lands as
//! usual.

use std::ops::Range;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use sc_geom::df_supports::{SupportContact, recommended_lift_mm};
use sc_print_glue::{PlateFrame, SlicedPlate};
use sc_review::{
    CavityFill, Finding, Growth, Island, IslandStart, ReviewInput, ReviewLeaf, ReviewOptions,
    SweepWatch, TipCone, TipPlan, TipPlanOptions, erase_stack, plan_all_tips, review_with,
};
use sc_scene::NodeId;
use sc_sl1cer::contour::{ContourLayer, ContourStack};
use sc_sl1cer::profile::{LayerSchedule, SliceProfile};
use sc_sl1cer::{CancelToken, LeafSlices};

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
        /// The pixels the review takes off the layers (#432).
        erased: CavityFill,
        /// Every island, as a placement of supports needs it (#437).
        islands: Vec<Island>,
        /// Every region that grew at once and wants more tips (#437).
        growths: Vec<Growth>,
        /// The tips those islands and regions need, `None` when no set can
        /// take them.
        tips: Option<Box<TipPlan>>,
        /// How the slice read the scene's points: a tip placed on this list
        /// is judged where the next slice will see it (#433).
        frame: PlateFrame,
        /// How long the review took, the sweep included.
        took: Duration,
    },
    /// Cancel stopped it: the run is cancelled with it.
    Cancelled,
    /// The review could not run, for this reason; the slice is unaffected.
    Failed(String),
}

/// A support leaf a session drew and where its supports touch the model: all
/// the review reads of them.
#[derive(Clone, Debug)]
pub(crate) struct SupportSet {
    pub(crate) leaf: NodeId,
    pub(crate) contacts: Vec<SupportContact>,
    /// The contact diameter of a tip added to it.
    pub(crate) tip_diameter_mm: f64,
}

/// The leaves the review reads: every sliced leaf but those `sets` know as
/// data, in the engine's order.
pub(crate) fn review_leaves<'l>(
    leaves: &'l [LeafSlices],
    sets: &[SupportSet],
) -> Vec<ReviewLeaf<'l>> {
    leaves
        .iter()
        .filter(|slices| !sets.iter().any(|set| set.leaf == slices.leaf()))
        .map(ReviewLeaf::from_slices)
        .collect()
}

/// The stretches of `contact`, each point where `at` says it lands.
fn contact_cones(
    contact: &SupportContact,
    at: impl Fn([f64; 3]) -> [f64; 3] + Copy,
) -> impl Iterator<Item = TipCone> {
    TipCone::stretches(
        at(contact.tip),
        contact.ball.map(at),
        contact.tip_radius_mm,
        contact.neck.map(at),
        at(contact.socket),
        contact.socket_radius_mm,
    )
}

/// The tips of `sets`, each point where `frame` says its leaf's point lands.
fn support_tips(sets: &[SupportSet], frame: &PlateFrame) -> Vec<TipCone> {
    sets.iter()
        .flat_map(|set| {
            let at = move |point| frame.point_mm(set.leaf, point);
            set.contacts
                .iter()
                .flat_map(move |contact| contact_cones(contact, at))
        })
        .collect()
}

/// What a tip must hold: the first pixels of an island, as the review that
/// listed it judged them (#433).
#[derive(Clone, Debug)]
pub(crate) struct TipHold {
    pub(crate) start: Arc<IslandStart>,
    pub(crate) frame: PlateFrame,
}

impl TipHold {
    /// Whether a tip of the set on `leaf` touching the model at `contacts`
    /// holds the island: what the next review will say of it.
    pub(crate) fn by(&self, leaf: NodeId, contacts: &[SupportContact]) -> bool {
        let at = |point| self.frame.point_mm(leaf, point);
        let cones: Vec<TipCone> = contacts
            .iter()
            .flat_map(|contact| contact_cones(contact, at))
            .collect();
        self.start.held_by(&cones)
    }
}

/// The layers the support leaves of `sets` have contours on. It looks only
/// at whether a layer has any.
fn support_layers(leaves: &[LeafSlices], sets: &[SupportSet]) -> Range<usize> {
    leaves
        .iter()
        .filter(|slices| sets.iter().any(|set| set.leaf == slices.leaf()))
        .filter_map(|slices| {
            let lit = |layer: &ContourLayer| !layer.polygons().is_empty();
            let layers = slices.stack().layers();
            Some((layers.iter().position(lit)?, layers.iter().rposition(lit)?))
        })
        .reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))
        .map_or(0..0, |(first, last)| first..last + 1)
}

/// The stack that takes `erased` off the leaves `sets` does not know as data
/// and off `filled`, all cut on `schedule`; `None` when nothing is erased.
/// Supports made here are never erased: what is erased is the model's.
pub(crate) fn erased_stack(
    leaves: &[LeafSlices],
    sets: &[SupportSet],
    filled: Option<&ContourStack>,
    erased: &CavityFill,
    schedule: &LayerSchedule,
) -> Option<ContourStack> {
    let models: Vec<&ContourStack> = review_leaves(leaves, sets)
        .iter()
        .map(|leaf| leaf.stack)
        .chain(filled)
        .collect();
    erase_stack([erased], &models, schedule)
}

/// Reviews `plate`: its models, with `filled`, the stack of the cavities
/// filled on this slice, as model; and what is known of its supports as
/// data, standing on a raft whose top is `raft_top_mm` above the plate, or
/// on none (#131). `on_layer` hears every layer swept, and `cancel` stops it.
pub(crate) fn review_slices(
    plate: &SlicedPlate,
    filled: Option<&ContourStack>,
    sets: &[SupportSet],
    raft_top_mm: Option<f64>,
    bare_tip_mm: Option<f64>,
    origin: [f64; 2],
    cancel: &CancelToken,
    on_layer: &(dyn Fn(usize) + Sync),
) -> ReviewOutcome {
    let frame = plate.frame().with_origin(origin);
    let supports = KnownSupports {
        sets,
        tips: support_tips(sets, &frame),
        box_mm: plate.support_box_mm(),
        frame,
        bare_tip_mm,
    };
    let watch = SweepWatch { cancel, on_layer };
    review_stacks(
        plate.leaves(),
        plate.profile(),
        filled,
        &supports,
        raft_top_mm,
        watch,
    )
}

/// What the review reads of the supports made here.
struct KnownSupports<'s> {
    sets: &'s [SupportSet],
    tips: Vec<TipCone>,
    box_mm: Option<[f64; 5]>,
    frame: PlateFrame,
    /// The contact of the tips a plate with no set would get (#444): its
    /// first tip starts the set.
    bare_tip_mm: Option<f64>,
}

/// Reviews `leaves`, cut with `profile`: its layer schedule, its screen and
/// its maximum height.
fn review_stacks(
    leaves: &[LeafSlices],
    profile: &SliceProfile,
    filled: Option<&ContourStack>,
    supports: &KnownSupports<'_>,
    raft_top_mm: Option<f64>,
    watch: SweepWatch<'_>,
) -> ReviewOutcome {
    guarded(|| {
        let mut reviewed = review_leaves(leaves, supports.sets);
        if let Some(stack) = filled {
            reviewed.push(ReviewLeaf::new(Some(FILLED_LEAF), stack));
        }
        let input = ReviewInput::from_profile(reviewed, profile)
            .map_err(|err| err.to_string())?
            .with_raft_top_mm(raft_top_mm)
            .with_support_lift_mm(Some(recommended_lift_mm(None)))
            .with_tips(supports.tips.clone())
            .with_support_extent(support_layers(leaves, supports.sets), supports.box_mm);
        Ok(
            match review_with(&input, &ReviewOptions::default(), watch) {
                None => ReviewOutcome::Cancelled,
                Some(mut report) => ReviewOutcome::Reviewed {
                    tips: tip_plan(&report.islands, &report.growths, supports).map(|plan| {
                        plan.note_too_small(&mut report.findings, &report.islands);
                        Box::new(plan)
                    }),
                    growths: report.growths,
                    frame: supports.frame.clone(),
                    findings: report.findings,
                    fills: report.fills,
                    erased: report.erased,
                    islands: report.islands,
                    took: report.timings.total,
                },
            },
        )
    })
}

/// The tips `islands` and `growths` need, of the widest contact among the
/// sets, which is the one that has to fit, or of the tips a plate with no set
/// would get; `None` when nothing says what tips those are.
fn tip_plan(
    islands: &[Island],
    growths: &[Growth],
    supports: &KnownSupports<'_>,
) -> Option<TipPlan> {
    let diameter_mm = supports
        .sets
        .iter()
        .map(|set| set.tip_diameter_mm)
        .reduce(f64::max)
        .or(supports.bare_tip_mm)?;
    let smallest = sc_geom::df_supports::zone_presets::smallest_size().tip_diameter_mm;
    Some(plan_all_tips(
        islands,
        growths,
        &TipPlanOptions::for_tip(diameter_mm).with_smallest(smallest),
    ))
}

/// [`review_stacks`] for a test that slices with its own profile, nobody
/// watching, the supports where `sets` says.
#[cfg(test)]
pub(crate) fn review_for_test(
    leaves: &[LeafSlices],
    filled: Option<&ContourStack>,
    profile: &SliceProfile,
    sets: &[SupportSet],
) -> ReviewOutcome {
    review_bare_for_test(leaves, filled, profile, sets, None)
}

/// [`review_for_test`], planning for a plate with no set with tips of
/// `bare_tip_mm`.
#[cfg(test)]
pub(crate) fn review_bare_for_test(
    leaves: &[LeafSlices],
    filled: Option<&ContourStack>,
    profile: &SliceProfile,
    sets: &[SupportSet],
    bare_tip_mm: Option<f64>,
) -> ReviewOutcome {
    let cancel = CancelToken::new();
    let watch = SweepWatch {
        cancel: &cancel,
        on_layer: &|_| {},
    };
    let frame = PlateFrame::default();
    let supports = KnownSupports {
        sets,
        tips: support_tips(sets, &frame),
        box_mm: None,
        frame,
        bare_tip_mm,
    };
    review_stacks(leaves, profile, filled, &supports, None, watch)
}

/// The resin of the leaves of `sets`, in cubic millimetres: what their
/// sections add up to, layer by layer.
#[cfg(test)]
pub(crate) fn support_resin_for_test(plate: &SlicedPlate, sets: &[SupportSet]) -> f64 {
    let area = |ring: &[sc_sl1cer::geom2d::Point2D]| {
        let mut twice = 0.0;
        for (i, a) in ring.iter().enumerate() {
            let b = &ring[(i + 1) % ring.len()];
            twice += a.x * b.y - b.x * a.y;
        }
        (twice * 0.5).abs()
    };
    let tops: Vec<f64> = plate.schedule().iter_slab_tops().collect();
    plate
        .leaves()
        .iter()
        .filter(|slices| sets.iter().any(|set| set.leaf == slices.leaf()))
        .flat_map(|slices| slices.stack().layers().iter().enumerate())
        .map(|(index, layer)| {
            let height = tops[index] - index.checked_sub(1).map_or(0.0, |below| tops[below]);
            let section: f64 = layer
                .polygons()
                .iter()
                .map(|polygon| {
                    area(polygon.outer().vertices())
                        - polygon
                            .holes()
                            .iter()
                            .map(|hole| area(hole.vertices()))
                            .sum::<f64>()
                })
                .sum();
            section * height
        })
        .sum()
}

/// How many tips of `sets` there are, and how many do not end in the models
/// of `plate`: a support always ends in the model.
#[cfg(test)]
pub(crate) fn tips_in_model_for_test(plate: &SlicedPlate, sets: &[SupportSet]) -> (usize, usize) {
    let frame = plate.frame();
    let ends: Vec<[f64; 3]> = sets
        .iter()
        .flat_map(|set| {
            let frame = &frame;
            set.contacts
                .iter()
                .map(move |contact| frame.point_mm(set.leaf, contact.tip))
        })
        .collect();
    let input = ReviewInput::from_profile(review_leaves(plate.leaves(), sets), plate.profile())
        .expect("the slice's leaves");
    (ends.len(), sc_review::tips_outside_model(&input, &ends))
}

/// The name of the filled cavities' leaf in the review.
const FILLED_LEAF: &str = "Filled cavities";

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

    /// The review of `leaves` with no support known as data.
    fn review_plain(
        leaves: &[LeafSlices],
        filled: Option<&ContourStack>,
        profile: &SliceProfile,
    ) -> ReviewOutcome {
        review_for_test(leaves, filled, profile, &[])
    }

    #[test]
    fn supports_known_as_data_are_left_out_and_the_rest_are_models() {
        let leaves = slice_scene(&model_and_support(), &profile(4)).expect("the scene slices");
        let all = review_leaves(&leaves, &[]);
        assert_eq!(all.len(), 2);
        for (leaf, slices) in all.iter().zip(&leaves) {
            assert!(std::ptr::eq(leaf.stack, slices.stack()));
        }
        let sets = [SupportSet {
            leaf: leaves[1].leaf(),
            contacts: Vec::new(),
            tip_diameter_mm: 0.3,
        }];
        let models = review_leaves(&leaves, &sets);
        assert_eq!(models.len(), 1);
        assert!(std::ptr::eq(models[0].stack, leaves[0].stack()));
        assert_eq!(support_layers(&leaves, &sets), 0..4);
    }

    #[test]
    fn a_tripped_token_cancels_the_review() {
        let leaves = slice_scene(&model_and_support(), &profile(4)).expect("the scene slices");
        let cancel = CancelToken::new();
        cancel.cancel();
        let watch = SweepWatch {
            cancel: &cancel,
            on_layer: &|_| {},
        };
        let supports = KnownSupports {
            sets: &[],
            tips: Vec::new(),
            box_mm: None,
            frame: PlateFrame::default(),
            bare_tip_mm: None,
        };
        let outcome = review_stacks(&leaves, &profile(4), None, &supports, None, watch);
        assert_eq!(outcome, ReviewOutcome::Cancelled);
    }

    #[test]
    fn what_the_review_erases_leaves_the_plate_it_slices() {
        // A slab in mid-air one layer thick: debris.
        let mut scene = Scene::new();
        add_cube(&mut scene, "Wolf", [0.0, 0.0, 0.0]);
        let slab = scene
            .add_leaf(
                named("Slab"),
                Transform3D::Affine(AffineTransform3D::from_linear_and_translation(
                    [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 0.25]],
                    [4.0, 0.0, 1.725],
                )),
                LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
            )
            .expect("inline-mesh leaf");
        scene.add_root(slab).expect("fresh leaf becomes a root");
        let profile = profile(12);
        let leaves = slice_scene(&scene, &profile).expect("the scene slices");
        let plain =
            merge_stacks(leaves.iter().map(|l| l.stack().clone()).collect()).expect("one schedule");
        assert!(lit_pixels(&plain, 6, &profile) > 0);

        let ReviewOutcome::Reviewed { erased, .. } = review_plain(&leaves, None, &profile) else {
            panic!("the review failed");
        };
        let cut = erased_stack(&leaves, &[], None, &erased, profile.layer_schedule())
            .expect("the slab leaves");
        let stacks = leaves
            .iter()
            .map(|l| l.stack().clone())
            .chain(Some(cut))
            .collect();
        let merged = merge_stacks(stacks).expect("one schedule");
        assert_eq!(lit_pixels(&merged, 6, &profile), 0);
        assert_eq!(
            lit_pixels(&merged, 0, &profile),
            lit_pixels(&plain, 0, &profile)
        );
    }

    #[test]
    fn a_plate_standing_on_the_build_plate_reviews_clean() {
        let leaves = slice_scene(&model_and_support(), &profile(4)).expect("the scene slices");
        match review_plain(&leaves, None, &profile(4)) {
            ReviewOutcome::Reviewed { findings, .. } => {
                assert!(findings.is_empty(), "two cubes on the plate: {findings:?}")
            }
            other => panic!("the review failed: {other:?}"),
        }
    }

    #[test]
    fn a_cube_in_mid_air_is_found() {
        let mut scene = Scene::new();
        add_cube(&mut scene, "Wolf", [0.0, 0.0, 0.0]);
        add_cube(&mut scene, "Floating", [4.0, 0.0, 2.0]);
        let leaves = slice_scene(&scene, &profile(12)).expect("the scene slices");
        let ReviewOutcome::Reviewed { findings, .. } = review_plain(&leaves, None, &profile(12))
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
        let outcome = review_plain(&leaves, None, &profile(8));
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
        } = review_plain(&leaves, None, &profile)
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
            review_plain(&leaves, Some(&filled), &profile)
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
