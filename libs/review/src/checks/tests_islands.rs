/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Island tests (#84).
//!
//! The fake printer has 0.1 mm pixels (0.01 mm² each) and 0.05 mm layers, so
//! a shape of `n` pixels is `n / 100` mm² and layer `k` is cut at
//! `0.05 * k + 0.025` mm. Layers 0 and 1 rest on the plate, so islands start
//! at layer 2.

use sc_print_primitives::Mirror;

use super::*;
use crate::finding::PlatePoint;
use crate::test_support::{Fixture, FixtureBuilder, HEIGHT_PX, LAYER_HEIGHT_MM, Shape, WIDTH_PX};

/// The island findings of `fixture` with the default options.
fn islands(fixture: &Fixture) -> Vec<Finding> {
    fixture.check(|ctx| run(ctx, &IslandOptions::default()))
}

/// The island findings of `fixture` with `options`.
fn islands_with(fixture: &Fixture, options: &IslandOptions) -> Vec<Finding> {
    fixture.check(|ctx| run(ctx, options))
}

/// The camera focus at the centre of pixel `(x, y)` on `layer`.
fn at_pixel(fixture: &Fixture, layer: usize, x: u32, y: u32) -> PlatePoint {
    fixture
        .input()
        .focus(layer, [f64::from(x) + 0.5, f64::from(y) + 0.5])
}

/// A square of `side` pixels whose corner is pixel `(x, y)`.
fn square(x: u32, y: u32, side: u32) -> Shape {
    Shape::rect(
        f64::from(x),
        f64::from(y),
        f64::from(x + side),
        f64::from(y + side),
    )
}

/// The single finding of `findings`.
fn only(findings: &[Finding]) -> &Finding {
    assert_eq!(findings.len(), 1, "{findings:#?}");
    &findings[0]
}

/// The pixel `[x, y]` whose centre the finding's focus is, on its first
/// layer.
fn focus_pixel(fixture: &Fixture, finding: &Finding) -> [u32; 2] {
    for y in 0..HEIGHT_PX {
        for x in 0..WIDTH_PX {
            if at_pixel(fixture, finding.first_layer, x, y) == finding.focus {
                return [x, y];
            }
        }
    }
    panic!("the focus {:?} is not a pixel centre", finding.focus);
}

/// Whether pixel `[x, y]` of `layer` is solid.
fn is_solid(fixture: &Fixture, layer: usize, [x, y]: [u32; 2]) -> bool {
    fixture
        .layer_pixels(layer)
        .solid
        .iter()
        .any(|run| run.row == y && run.x0 <= x && x < run.x1)
}

/// Asserts that the finding's focus is a solid pixel of its first layer with
/// nothing solid straight below it, and that a model island's suggested
/// support is there, at the bottom of the layer.
fn assert_focus_rests_on_nothing(fixture: &Fixture, finding: &Finding) {
    let pixel = focus_pixel(fixture, finding);
    assert!(is_solid(fixture, finding.first_layer, pixel), "{pixel:?}");
    assert!(
        !is_solid(fixture, finding.first_layer - 1, pixel),
        "the focus {pixel:?} rests on cured resin"
    );
    if finding.scope == RoleScope::Model {
        let at = fixture
            .input()
            .underside(finding.first_layer, finding.focus);
        assert_eq!(finding.fix, Some(SuggestedFix::AddSupport { at }));
    }
}

// Clean plates: nothing to report.

#[test]
fn a_block_standing_on_the_plate_has_no_island() {
    // 40 x 30 pixels (4 x 3 mm) on every layer of an 8-layer plate.
    let fixture = FixtureBuilder::new(8)
        .model(.., Shape::rect(40.0, 30.0, 80.0, 60.0))
        .build();
    assert!(islands(&fixture).is_empty(), "{:?}", islands(&fixture));
}

#[test]
fn an_empty_plate_has_no_island() {
    assert!(islands(&FixtureBuilder::new(4).build()).is_empty());
}

#[test]
fn parts_on_the_first_two_layers_rest_on_the_plate() {
    // Parts on layer 0 only, whatever their shape.
    let fixture = FixtureBuilder::new(3)
        .model(0..1, Shape::rect(10.0, 10.0, 60.0, 40.0))
        .model(0..1, square(100, 20, 1))
        .support(0..1, Shape::circle(150.0, 80.0, 6.0))
        .build();
    assert!(islands(&fixture).is_empty());
    // A base that touches the plate along a 20-pixel sliver and widens to
    // 600 pixels on layer 1, and a part on layer 1 alone: the first layers'
    // exposure cures through the one empty layer below them, and no support
    // would fit there.
    let fixture = FixtureBuilder::new(6)
        .model(0..1, Shape::rect(50.0, 50.0, 60.0, 52.0))
        .model(1.., Shape::rect(40.0, 40.0, 70.0, 60.0))
        .model(1..2, square(150, 60, 5))
        .build();
    assert!(islands(&fixture).is_empty(), "{:#?}", islands(&fixture));
}

#[test]
fn a_pyramid_narrowing_upwards_has_no_island() {
    let mut builder = FixtureBuilder::new(10);
    for layer in 0..10 {
        let inset = 2.0 * layer as f64;
        builder = builder.model(
            layer..=layer,
            Shape::rect(40.0 + inset, 20.0 + inset, 100.0 - inset, 80.0 - inset),
        );
    }
    assert!(islands(&builder.build()).is_empty());
}

#[test]
fn an_overhang_growing_gently_has_no_island() {
    // The block widens by 2 pixels per side per layer: every layer rests on
    // most of the one below.
    let mut builder = FixtureBuilder::new(10);
    for layer in 0..10 {
        let grow = 2.0 * layer as f64;
        builder = builder.model(
            layer..=layer,
            Shape::rect(80.0 - grow, 50.0 - grow, 100.0 + grow, 70.0 + grow),
        );
    }
    assert!(islands(&builder.build()).is_empty());
}

#[test]
fn a_model_held_by_a_support_tip_has_no_island() {
    // A 10 x 10 model block starting at layer 4, on a 6 x 6 support pillar
    // from the plate whose tip enters the block by one layer.
    let fixture = FixtureBuilder::new(8)
        .model(4.., square(60, 40, 10))
        .support(0..5, square(62, 42, 6))
        .build();
    assert!(islands(&fixture).is_empty(), "{:#?}", islands(&fixture));
}

#[test]
fn a_model_on_a_support_tip_that_ends_below_it_has_no_island() {
    // The tip stops at layer 3; the block starts at layer 4 right on top of
    // it. The tip and the block never share a layer, yet the block rests on
    // the tip.
    let fixture = FixtureBuilder::new(8)
        .model(4.., square(60, 40, 10))
        .support(0..4, square(62, 42, 6))
        .build();
    assert!(islands(&fixture).is_empty(), "{:#?}", islands(&fixture));
}

#[test]
fn a_plate_on_four_support_tips_has_no_island() {
    // A 20 x 20 model plate held by four 4 x 4 tips that enter it: they cover
    // 64 of its 400 pixels, 16 %, under the model margin but well over the
    // tip margin. Generated supports hold their islands this way.
    let fixture = FixtureBuilder::new(8)
        .model(4.., square(60, 40, 20))
        .support(0..5, square(62, 42, 4))
        .support(0..5, square(74, 42, 4))
        .support(0..5, square(62, 54, 4))
        .support(0..5, square(74, 54, 4))
        .build();
    assert!(islands(&fixture).is_empty(), "{:#?}", islands(&fixture));
}

#[test]
fn a_model_on_a_raft_has_no_island() {
    let fixture = FixtureBuilder::new(8)
        .support(0..2, Shape::rect(20.0, 20.0, 120.0, 90.0))
        .model(2.., Shape::rect(40.0, 30.0, 100.0, 80.0))
        .build();
    assert!(islands(&fixture).is_empty());
}

#[test]
fn a_region_resting_exactly_on_the_margin_is_held() {
    // A 20 x 20 slab on a 10 x 10 post: exactly 25 % of it rests on the
    // post, which is not under the margin.
    let fixture = FixtureBuilder::new(6)
        .model(0..3, square(70, 50, 10))
        .model(3.., square(65, 45, 20))
        .build();
    assert!(islands(&fixture).is_empty(), "{:#?}", islands(&fixture));
}

#[test]
fn a_part_joined_through_three_pixels_is_held() {
    // A 20 x 20 slab from layer 4 joined to a block on the plate by a neck 3
    // pixels wide: one piece, resting on the block.
    let fixture = FixtureBuilder::new(8)
        .model(.., Shape::rect(20.0, 20.0, 60.0, 60.0))
        .model(4.., Shape::rect(60.0, 40.0, 63.0, 43.0))
        .model(4.., Shape::rect(63.0, 30.0, 83.0, 50.0))
        .build();
    assert!(islands(&fixture).is_empty(), "{:#?}", islands(&fixture));
}

// Islands that must be reported.

#[test]
fn a_part_starting_in_mid_air_is_an_unsupported_island() {
    // A block on the plate, and a 20 x 10 model bar that starts at layer 4
    // with nothing below it and goes on to the top.
    let fixture = FixtureBuilder::new(8)
        .model(.., Shape::rect(10.0, 10.0, 40.0, 40.0))
        .model(4.., Shape::rect(100.0, 60.0, 120.0, 70.0))
        .build();
    let finding = only(&islands(&fixture)).clone();
    assert_eq!(finding.check, CheckKind::Island);
    assert_eq!(finding.severity, Severity::Error);
    assert_eq!(finding.scope, RoleScope::Model);
    assert_eq!((finding.first_layer, finding.last_layer), (4, 7));
    assert_eq!(finding.title, "Unsupported island");
    assert_eq!(
        finding.detail,
        "2.00 mm² of model starts with nothing cured below it (0 % supported)."
    );
    // The middle of the bar, 5 pixels from its long edges: of the pixels that
    // far in, four are nearest its centroid (110, 65), and the first in scan
    // order wins. On the bar's first layer, cut 0.225 mm above the plate; a
    // support tip goes right below it, on the bar's underside 0.2 mm up.
    let expected = at_pixel(&fixture, 4, 109, 64);
    assert_eq!(finding.focus, expected);
    assert!((expected.z_mm - LAYER_HEIGHT_MM.mul_add(4.0, 0.5 * LAYER_HEIGHT_MM)).abs() < 1e-9);
    let at = PlatePoint::new(expected.x_mm, expected.y_mm, 4.0 * LAYER_HEIGHT_MM);
    let Some(SuggestedFix::AddSupport { at: fix }) = finding.fix else {
        panic!("{:?}", finding.fix)
    };
    assert!((fix.z_mm - at.z_mm).abs() < 1e-9, "{fix:?}");
    assert_eq!((fix.x_mm, fix.y_mm), (at.x_mm, at.y_mm));
}

#[test]
fn a_slab_on_a_thin_post_is_a_weakly_supported_island() {
    // A 20 x 20 slab on a 4 x 4 model post: 16 of its 400 pixels rest on
    // cured resin, 4 %, under a fifth of the 25 % margin: an error.
    let fixture = FixtureBuilder::new(8)
        .model(0..4, square(78, 48, 4))
        .model(4.., square(70, 40, 20))
        .build();
    let finding = only(&islands(&fixture)).clone();
    assert_eq!(finding.severity, Severity::Error);
    assert_eq!(finding.scope, RoleScope::Model);
    assert_eq!((finding.first_layer, finding.last_layer), (4, 7));
    assert_eq!(finding.title, "Weakly supported island");
    assert_eq!(
        finding.detail,
        "4.00 mm² of model rests on only 0.160 mm² of cured resin (4 % supported, under the 25 % margin)."
    );
    // The support goes on the slab's unheld frame, never on the post.
    assert_focus_rests_on_nothing(&fixture, &finding);
}

#[test]
fn a_slab_on_a_wider_post_is_a_warning() {
    // The same slab on an 8 x 6 post: 48 of 400 pixels, 12 %: held, but by
    // less than the margin.
    let fixture = FixtureBuilder::new(8)
        .model(0..4, Shape::rect(76.0, 47.0, 84.0, 53.0))
        .model(4.., square(70, 40, 20))
        .build();
    let finding = only(&islands(&fixture)).clone();
    assert_eq!(finding.severity, Severity::Warning);
    assert_eq!(
        finding.detail,
        "4.00 mm² of model rests on only 0.480 mm² of cured resin (12 % supported, under the 25 % margin)."
    );
    assert_focus_rests_on_nothing(&fixture, &finding);
}

#[test]
fn a_model_on_too_thin_a_support_tip_is_weakly_supported() {
    // The same slab held by a 3 x 2 support tip that enters it: the tip
    // counts against the tip margin, but 6 pixels of 400 (1.5 %) are under
    // it. The island concerns the model, although its piece has support
    // pixels, and the new support goes beside the tip, not on it.
    let fixture = FixtureBuilder::new(8)
        .model(4.., square(70, 40, 20))
        .support(0..5, Shape::rect(79.0, 49.0, 82.0, 51.0))
        .build();
    let finding = only(&islands(&fixture)).clone();
    assert_eq!(finding.severity, Severity::Warning);
    assert_eq!(finding.scope, RoleScope::Model);
    assert_eq!(finding.title, "Weakly supported island");
    assert_eq!(
        finding.detail,
        "4.00 mm² of model rests on only 0.0600 mm² of support tips (1 % supported, under the 5 % margin for tips)."
    );
    assert_focus_rests_on_nothing(&fixture, &finding);
}

#[test]
fn a_model_on_a_sliver_of_a_tip_is_an_error() {
    // A 2 x 1 tip under the 400-pixel slab: 0.5 %, under a fifth of the tip
    // margin.
    let fixture = FixtureBuilder::new(8)
        .model(4.., square(70, 40, 20))
        .support(0..5, Shape::rect(79.0, 49.0, 81.0, 50.0))
        .build();
    let finding = only(&islands(&fixture)).clone();
    assert_eq!(finding.severity, Severity::Error);
    assert!(
        finding
            .detail
            .ends_with("(<1 % supported, under the 5 % margin for tips)."),
        "{}",
        finding.detail
    );
}

#[test]
fn a_model_on_model_and_tips_mixes_the_margins() {
    // 40 pixels of a 400-pixel slab rest on a model post (10 %, under the
    // 25 % margin) and 4 on a tip (1 %, under the 5 % margin): together they
    // give 10 / 25 + 1 / 5 = 60 % of the hold asked for, so the slab is
    // under the margin of that mix, 44 / 400 / 0.6 = 18.33 %.
    let fixture = FixtureBuilder::new(8)
        .model(0..4, Shape::rect(72.0, 42.0, 80.0, 47.0))
        .support(0..5, square(84, 52, 2))
        .model(4.., square(70, 40, 20))
        .build();
    let finding = only(&islands(&fixture)).clone();
    assert_eq!(finding.severity, Severity::Warning);
    assert_eq!(
        finding.detail,
        "4.00 mm² of model rests on only 0.440 mm² of cured resin and support tips (11.00 % supported, under the 18.33 % margin for that mix)."
    );
}

#[test]
fn removing_the_support_turns_the_island_into_an_error() {
    let supported = FixtureBuilder::new(8)
        .model(4.., square(60, 40, 10))
        .support(0..5, square(62, 42, 6))
        .build();
    let bare = FixtureBuilder::new(8)
        .model(4.., square(60, 40, 10))
        .build();
    assert!(islands(&supported).is_empty());
    assert_eq!(only(&islands(&bare)).severity, Severity::Error);
}

#[test]
fn a_floating_support_is_a_support_island() {
    // A support pillar that starts at layer 3 with nothing below it.
    let fixture = FixtureBuilder::new(8)
        .model(.., Shape::rect(10.0, 10.0, 40.0, 40.0))
        .support(3.., square(150, 80, 5))
        .build();
    let finding = only(&islands(&fixture)).clone();
    assert_eq!(finding.severity, Severity::Error);
    assert_eq!(finding.scope, RoleScope::Support);
    assert_eq!((finding.first_layer, finding.last_layer), (3, 7));
    assert_eq!(finding.title, "Floating support");
    assert_eq!(
        finding.detail,
        "0.250 mm² of support starts with nothing cured below it (0 % supported)."
    );
    assert_eq!(finding.focus, at_pixel(&fixture, 3, 152, 82));
    // A support tip goes on the model, not under another support.
    assert_eq!(finding.fix, None);
}

#[test]
fn a_support_on_too_little_is_weakly_held() {
    // A support platform that widens from a 2 x 2 stem to 12 x 12 at once:
    // support on support is structure, judged against the 25 % margin.
    let fixture = FixtureBuilder::new(6)
        .support(0..3, square(150, 60, 2))
        .support(3.., square(145, 55, 12))
        .build();
    let finding = only(&islands(&fixture)).clone();
    assert_eq!(finding.severity, Severity::Error);
    assert_eq!(finding.scope, RoleScope::Support);
    assert_eq!(finding.title, "Weakly held support");
    assert_eq!(
        finding.detail,
        "1.44 mm² of support rests on only 0.0400 mm² of cured resin (2 % supported, under the 25 % margin)."
    );
    assert_eq!(finding.fix, None);
}

#[test]
fn a_floating_support_that_runs_into_a_model_is_a_support_island() {
    // A 5 x 5 support starting in mid-air at layer 3 enters a model slab
    // that starts at layer 4. The island starts with support only: it is a
    // floating support, suggests no support under it, and says how large the
    // part hanging from it grows. The slab rests on the support as on a tip
    // (25 of 400 pixels, 6 %), so it is not an island of its own.
    let fixture = FixtureBuilder::new(8)
        .support(3..5, square(100, 60, 5))
        .model(4.., square(92, 52, 20))
        .build();
    let finding = only(&islands(&fixture)).clone();
    assert_eq!(finding.scope, RoleScope::Support);
    assert_eq!(finding.title, "Floating support");
    assert_eq!((finding.first_layer, finding.last_layer), (3, 7));
    assert_eq!(
        finding.detail,
        "0.250 mm² of support starts with nothing cured below it (0 % supported), and the part it starts grows to 4.00 mm² with nothing else holding it."
    );
    assert_eq!(finding.fix, None);
}

#[test]
fn model_and_support_islands_are_reported_apart() {
    let fixture = FixtureBuilder::new(6)
        .model(3.., square(40, 40, 10))
        .support(3.., square(140, 40, 10))
        .build();
    let findings = islands(&fixture);
    assert_eq!(findings.len(), 2, "{findings:#?}");
    let mut scopes: Vec<RoleScope> = findings.iter().map(|f| f.scope).collect();
    scopes.sort_unstable();
    assert_eq!(scopes, vec![RoleScope::Model, RoleScope::Support]);
    assert!(findings.iter().all(|f| f.severity == Severity::Error));
}

// Weak joints.

#[test]
fn a_part_hanging_from_a_corner_pixel_is_an_island() {
    // A 20 x 20 slab from layer 4 that meets a block standing on the plate
    // only where the block's corner pixel (59, 59) touches the slab's
    // (60, 60): no joint at all.
    let fixture = FixtureBuilder::new(8)
        .model(.., Shape::rect(20.0, 20.0, 60.0, 60.0))
        .model(4.., Shape::rect(60.0, 60.0, 80.0, 80.0))
        .build();
    let finding = only(&islands(&fixture)).clone();
    assert_eq!(finding.severity, Severity::Error);
    assert_eq!((finding.first_layer, finding.last_layer), (4, 7));
    assert_eq!(
        finding.detail,
        "4.00 mm² of model starts with nothing cured below it (0 % supported)."
    );
    assert_eq!(finding.focus, at_pixel(&fixture, 4, 69, 69));
    assert_focus_rests_on_nothing(&fixture, &finding);
}

#[test]
fn a_part_hanging_from_a_one_pixel_neck_is_an_island() {
    // The slab hangs from the block by a 3 x 1 pixel neck.
    let fixture = FixtureBuilder::new(8)
        .model(.., Shape::rect(20.0, 20.0, 60.0, 60.0))
        .model(4.., Shape::rect(60.0, 40.0, 63.0, 41.0))
        .model(4.., Shape::rect(63.0, 30.0, 83.0, 50.0))
        .build();
    let finding = only(&islands(&fixture)).clone();
    assert_eq!(finding.severity, Severity::Error);
    assert_eq!(
        finding.detail,
        "4.00 mm² of model starts with nothing cured below it (0 % supported)."
    );
    assert_eq!(finding.focus, at_pixel(&fixture, 4, 72, 39));
}

#[test]
fn a_model_touching_a_support_only_at_a_corner_is_an_island() {
    // A 6 x 6 model blob from layer 4 whose corner touches a 20 x 20 support
    // pillar's corner.
    let fixture = FixtureBuilder::new(8)
        .support(.., square(40, 40, 20))
        .model(4.., square(60, 60, 6))
        .build();
    let finding = only(&islands(&fixture)).clone();
    assert_eq!(finding.scope, RoleScope::Model);
    assert_eq!(finding.severity, Severity::Error);
    assert_eq!(finding.focus, at_pixel(&fixture, 4, 62, 62));
}

// One finding per start, over the part that hangs from it.

#[test]
fn a_growing_tip_is_one_island_over_its_layers() {
    // An upside-down pyramid starting at layer 3: 2 x 2, then 6 x 6 (4 of
    // its 36 pixels rest on the tip), then 10 x 10 up to the top. Nothing
    // sound ever holds it: one island over layers 3 to 7.
    let fixture = FixtureBuilder::new(8)
        .model(3..4, square(99, 59, 2))
        .model(4..5, square(97, 57, 6))
        .model(5.., square(95, 55, 10))
        .build();
    let finding = only(&islands(&fixture)).clone();
    assert_eq!((finding.first_layer, finding.last_layer), (3, 7));
    assert_eq!(finding.severity, Severity::Error);
    assert_eq!(
        finding.detail,
        "0.0400 mm² of model starts with nothing cured below it (0 % supported), and the part it starts grows to 1.00 mm² with nothing else holding it. Its first layer is smaller than a support tip, so the support aims one layer higher, where it is large enough."
    );
    assert_eq!(finding.focus, at_pixel(&fixture, 3, 99, 59));
    // #405: the 2 x 2 tip (0.04 mm²) is smaller than a tip's contact
    // (0.07 mm²): the support goes one layer up, on the 6 x 6 layer (0.36
    // mm²), on the ring that grew around the tip, at the bottom of layer 4.
    let Some(SuggestedFix::AddSupport { at }) = finding.fix else {
        panic!("{:?}", finding.fix)
    };
    let span = fixture.input().layers()[4];
    assert!((at.z_mm - span.bottom_mm).abs() < 1e-9, "{at:?}");
    let ring = |[x, y]: [u32; 2]| (97..103).contains(&x) && (57..63).contains(&y);
    let tip = |[x, y]: [u32; 2]| (99..101).contains(&x) && (59..61).contains(&y);
    let raster = |at: PlatePoint| {
        (0..WIDTH_PX)
            .flat_map(|x| (0..HEIGHT_PX).map(move |y| [x, y]))
            .find(|&[x, y]| {
                let p = fixture
                    .input()
                    .pixel_to_plate([f64::from(x) + 0.5, f64::from(y) + 0.5]);
                (p[0] - at.x_mm).abs() < 1e-9 && (p[1] - at.y_mm).abs() < 1e-9
            })
    };
    let pixel = raster(at).expect("a pixel centre");
    assert!(ring(pixel) && !tip(pixel), "{pixel:?}");
    // Without the policy, it aims at the tip itself.
    let plain = only(&islands_with(
        &fixture,
        &IslandOptions::default().without_policy(),
    ))
    .clone();
    assert_eq!(
        plain.fix,
        Some(SuggestedFix::AddSupport {
            at: fixture.input().underside(3, plain.focus)
        })
    );
}

/// #405: a first layer as large as a tip's contact keeps its support.
#[test]
fn a_first_layer_as_large_as_a_tip_keeps_its_support() {
    // A 3 x 3 tip (0.09 mm², larger than the 0.07 mm² contact) that grows.
    let fixture = FixtureBuilder::new(8)
        .model(3..4, square(99, 59, 3))
        .model(4.., square(96, 56, 9))
        .build();
    let finding = only(&islands(&fixture)).clone();
    assert_eq!(
        finding.fix,
        Some(SuggestedFix::AddSupport {
            at: fixture.input().underside(3, finding.focus)
        })
    );
    assert!(!finding.detail.contains("aims"), "{}", finding.detail);
}

#[test]
fn an_island_spans_the_part_hanging_from_it() {
    // A 2 x 2 tip at layer 3 grows on itself (6, 10 and 14 pixels wide) until
    // a wall standing on the plate takes it in at layer 7: the island spans
    // layers 3 to 6 and grows to 1.96 mm².
    let fixture = FixtureBuilder::new(9)
        .model(.., Shape::rect(20.0, 20.0, 40.0, 100.0))
        .model(3..4, square(99, 59, 2))
        .model(4..5, square(97, 57, 6))
        .model(5..6, square(95, 55, 10))
        .model(6..7, square(93, 53, 14))
        .model(7.., Shape::rect(20.0, 53.0, 107.0, 67.0))
        .build();
    let finding = only(&islands(&fixture)).clone();
    assert_eq!((finding.first_layer, finding.last_layer), (3, 6));
    assert_eq!(
        finding.detail,
        "0.0400 mm² of model starts with nothing cured below it (0 % supported), and the part it starts grows to 1.96 mm² before the rest of the print holds it. Its first layer is smaller than a support tip, so the support aims one layer higher, where it is large enough."
    );
}

#[test]
fn a_shallow_ramp_in_mid_air_is_one_island() {
    // A 10 x 10 block that slides 8 pixels sideways per layer: from layer 2
    // every layer rests on 20 of its 100 pixels, so all of them are islands,
    // joined into one that starts at layer 2.
    let mut builder = FixtureBuilder::new(8).model(0..1, square(20, 50, 10));
    for layer in 1..8 {
        builder = builder.model(layer..=layer, square(20 + 8 * layer as u32, 50, 10));
    }
    let finding = only(&islands(&builder.build())).clone();
    assert_eq!((finding.first_layer, finding.last_layer), (2, 7));
    assert_eq!(finding.severity, Severity::Warning);
    assert_eq!(
        finding.detail,
        "1.00 mm² of model rests on only 0.200 mm² of cured resin (20 % supported, under the 25 % margin)."
    );
}

#[test]
fn a_strand_sliding_by_its_width_is_one_island() {
    // A 5 x 5 strand in mid-air that moves 5 pixels per layer: no layer rests
    // on the one below, but each touches it along an edge, so it is one
    // island from layer 2 to 9, not eight.
    let mut builder = FixtureBuilder::new(10);
    for layer in 2..10 {
        builder = builder.model(layer..=layer, square(20 + 5 * layer as u32, 50, 5));
    }
    let fixture = builder.build();
    let finding = only(&islands(&fixture)).clone();
    assert_eq!((finding.first_layer, finding.last_layer), (2, 9));
    assert_eq!(finding.severity, Severity::Error);
    assert_eq!(finding.focus, at_pixel(&fixture, 2, 32, 52));
}

#[test]
fn a_part_growing_on_a_weak_island_is_reported_where_it_starts() {
    // A 10 x 10 slab on a 3 x 3 post (9 %), then a 40 x 40 slab on it: one
    // island from layer 3, whose numbers are the start's, and whose detail
    // shows the 16 mm² that hang from it.
    let fixture = FixtureBuilder::new(8)
        .model(0..3, square(54, 54, 3))
        .model(3..4, square(50, 50, 10))
        .model(4.., square(35, 35, 40))
        .build();
    let finding = only(&islands(&fixture)).clone();
    assert_eq!((finding.first_layer, finding.last_layer), (3, 7));
    assert_eq!(finding.severity, Severity::Warning);
    assert_eq!(
        finding.detail,
        "1.00 mm² of model rests on only 0.0900 mm² of cured resin (9 % supported, under the 25 % margin), and the part it starts grows to 16.0 mm² with nothing else holding it."
    );
    assert_focus_rests_on_nothing(&fixture, &finding);
}

#[test]
fn starts_that_merge_are_reported_each_with_its_support() {
    // On layer 3, a 10 x 10 slab on a 3 x 3 post (9 %) and a 4 x 4 square
    // with nothing below; from layer 4 one slab covers both. Each start is a
    // finding with its own support, on its own unheld pixels.
    let fixture = FixtureBuilder::new(8)
        .model(0..3, square(54, 54, 3))
        .model(3..4, square(50, 50, 10))
        .model(3..4, square(70, 52, 4))
        .model(4.., Shape::rect(45.0, 45.0, 80.0, 65.0))
        .build();
    let findings = islands(&fixture);
    assert_eq!(findings.len(), 2, "{findings:#?}");
    let weak = findings
        .iter()
        .find(|f| f.severity == Severity::Warning)
        .expect("the slab on the post");
    let bare = findings
        .iter()
        .find(|f| f.severity == Severity::Error)
        .expect("the square on nothing");
    assert!(
        weak.detail
            .starts_with("1.00 mm² of model rests on only 0.0900 mm²"),
        "{}",
        weak.detail
    );
    assert!(
        bare.detail
            .starts_with("0.160 mm² of model starts with nothing"),
        "{}",
        bare.detail
    );
    assert_eq!(bare.focus, at_pixel(&fixture, 3, 71, 53));
    for finding in &findings {
        assert_eq!((finding.first_layer, finding.last_layer), (3, 7));
        assert_focus_rests_on_nothing(&fixture, finding);
    }
}

#[test]
fn two_tips_that_merge_are_two_islands() {
    // Two 2 x 2 tips at layer 2, joined at layer 3 by a bar that rests on
    // them alone: each tip needs its own support.
    let fixture = FixtureBuilder::new(6)
        .model(2..3, square(40, 40, 2))
        .model(2..3, square(60, 40, 2))
        .model(3.., Shape::rect(35.0, 38.0, 67.0, 46.0))
        .build();
    let findings = islands(&fixture);
    assert_eq!(findings.len(), 2, "{findings:#?}");
    let foci: Vec<PlatePoint> = findings.iter().map(|f| f.focus).collect();
    assert!(foci.contains(&at_pixel(&fixture, 2, 40, 40)), "{foci:?}");
    assert!(foci.contains(&at_pixel(&fixture, 2, 60, 40)), "{foci:?}");
}

#[test]
fn two_tips_side_by_side_are_two_islands() {
    let fixture = FixtureBuilder::new(6)
        .model(2.., square(40, 40, 6))
        .model(2.., square(80, 40, 6))
        .build();
    let findings = islands(&fixture);
    assert_eq!(findings.len(), 2, "{findings:#?}");
    assert_ne!(findings[0].focus, findings[1].focus);
}

#[test]
fn islands_stacked_over_a_gap_are_two_islands() {
    // A one-layer island at layer 2, nothing at layers 3 and 4, another at
    // layer 5 over the same pixels: a 0.10 mm gap no exposure cures through,
    // so they are two islands.
    let fixture = FixtureBuilder::new(7)
        .model(2..3, square(60, 60, 8))
        .model(5.., square(60, 60, 8))
        .build();
    let findings = islands(&fixture);
    let layers: Vec<(usize, usize)> = findings
        .iter()
        .map(|f| (f.first_layer, f.last_layer))
        .collect();
    assert_eq!(layers, vec![(2, 2), (5, 6)]);
}

/// #403: the exposure of a layer cures through one layer of liquid at 0.05
/// mm, so what has solid two layers below rests on it.
#[test]
fn solid_one_layer_of_liquid_below_holds_a_part() {
    // A block on the plate to layer 2, nothing at layer 3, and the same
    // block again from layer 4: no island.
    let fixture = FixtureBuilder::new(8)
        .model(0..3, square(60, 60, 8))
        .model(4.., square(60, 60, 8))
        .build();
    assert!(islands(&fixture).is_empty(), "{:?}", islands(&fixture));
    // Counting only the layer right below, it is an island.
    let plain = IslandOptions {
        cure_depth_mm: 0.0,
        ..IslandOptions::default()
    };
    assert_eq!(islands_with(&fixture, &plain).len(), 1);
    // Two islands over one layer of liquid are one island.
    let stacked = FixtureBuilder::new(6)
        .model(2..3, square(60, 60, 8))
        .model(4.., square(60, 60, 8))
        .build();
    let layers: Vec<(usize, usize)> = islands(&stacked)
        .iter()
        .map(|f| (f.first_layer, f.last_layer))
        .collect();
    assert_eq!(layers, vec![(2, 5)]);
    // At 0.1 mm layers the exposure does not reach through a layer.
    let thick = FixtureBuilder::new(8)
        .layer_height_mm(0.1)
        .max_z_mm(10.0)
        .model(0..3, square(60, 60, 8))
        .model(4.., square(60, 60, 8))
        .build();
    assert_eq!(islands(&thick).len(), 1);
}

/// #403: how many layers of liquid the exposure cures through, by layer
/// height: one at 0.05 mm, two at 0.03 mm, none at 0.1 mm.
#[test]
fn the_cure_depth_reaches_through_layers_by_their_height() {
    let gap = |layer_mm: f64| {
        let fixture = FixtureBuilder::new(12)
            .layer_height_mm(layer_mm)
            .model(.., square(10, 10, 4))
            .build();
        pieces::cured_gap_layers(&fixture.input(), 8, island_policy::CURE_DEPTH_MM)
    };
    assert_eq!(gap(0.05), 1);
    assert_eq!(gap(0.03), 2);
    assert_eq!(gap(0.1), 0);
    assert_eq!(gap(0.01), island_policy::MAX_CURED_GAP_LAYERS);
}

/// A wall on the plate, and a 2 x 2 speck beside it at layer 3 that a ledge
/// from the wall covers from layer `ledge`: an island of `ledge - 3` layers.
fn speck_under_a_ledge(ledge: usize, speck_side: u32) -> Fixture {
    FixtureBuilder::new(10)
        .model(.., Shape::rect(20.0, 20.0, 40.0, 100.0))
        .model(3..ledge, square(50, 50, speck_side))
        .model(ledge.., Shape::rect(20.0, 48.0, 56.0, 56.0))
        .build()
}

/// #402: a speck the next layers take in is a minimal island: a note, with
/// no support.
#[test]
fn a_speck_the_next_layer_takes_in_is_a_minimal_island() {
    // One layer (0.05 mm), 0.04 mm², 0.002 mm³.
    let fixture = speck_under_a_ledge(4, 2);
    let finding = only(&islands(&fixture)).clone();
    assert_eq!(finding.title, "Minimal island");
    assert_eq!(finding.severity, Severity::Info);
    assert_eq!(finding.fix, None);
    assert_eq!((finding.first_layer, finding.last_layer), (3, 3));
    assert_eq!(
        finding.detail,
        "0.0400 mm² of model starts with nothing cured below it, but the next layer holds it (0.0500 mm), 0.00200 mm³ in all and never larger than 0.0400 mm². The next layers lift it with them; a support this small would do more harm than good."
    );
    // Two layers (0.10 mm) still are.
    let two = speck_under_a_ledge(5, 2);
    assert_eq!(only(&islands(&two)).severity, Severity::Info);
    // Without the policy, it is an unsupported island.
    let plain = only(&islands_with(
        &fixture,
        &IslandOptions::default().without_policy(),
    ))
    .clone();
    assert_eq!(plain.title, "Unsupported island");
    assert!(matches!(plain.fix, Some(SuggestedFix::AddSupport { .. })));
}

/// #402: an island that grows higher, wider or loose is not minimal.
#[test]
fn a_higher_wider_or_loose_island_is_not_minimal() {
    // Three layers (0.15 mm).
    let three = speck_under_a_ledge(6, 2);
    let finding = only(&islands(&three)).clone();
    assert_eq!(finding.title, "Unsupported island");
    assert!(matches!(finding.fix, Some(SuggestedFix::AddSupport { .. })));
    // A 3 x 3 section (0.09 mm²), larger than a tip's contact.
    let wide = speck_under_a_ledge(4, 3);
    assert_eq!(only(&islands(&wide)).title, "Unsupported island");
    // A one-layer speck nothing ever takes in.
    let loose = FixtureBuilder::new(6)
        .model(.., Shape::rect(20.0, 20.0, 40.0, 100.0))
        .model(3..4, square(50, 50, 2))
        .build();
    assert_eq!(only(&islands(&loose)).title, "Unsupported island");
}

/// #402: a support sliver the next layer takes in is a minimal floating
/// support.
#[test]
fn a_support_sliver_the_next_layer_takes_in_is_minimal() {
    let fixture = FixtureBuilder::new(8)
        .support(.., Shape::rect(20.0, 20.0, 40.0, 100.0))
        .support(3..4, square(50, 50, 2))
        .support(4.., Shape::rect(20.0, 48.0, 56.0, 56.0))
        .build();
    let finding = only(&islands(&fixture)).clone();
    assert_eq!(finding.title, "Minimal floating support");
    assert_eq!(finding.severity, Severity::Info);
    assert_eq!(finding.scope, RoleScope::Support);
}

/// #404: a piece no larger than a tip's contact stays with the part it
/// touches across a weak joint.
#[test]
fn a_tiny_piece_on_a_weak_joint_stays_with_what_it_touches() {
    // 0.05 mm pixels (0.0025 mm²): a block on the plate and, from layer 3,
    // a 5 x 5 square (25 pixels, 0.0625 mm²) touching its corner through
    // one pixel, with nothing below it.
    let fixture = FixtureBuilder::new(8)
        .screen(160, 120, 0.05)
        .model(.., Shape::rect(20.0, 20.0, 60.0, 60.0))
        .model(3.., Shape::rect(60.0, 60.0, 65.0, 65.0))
        .build();
    assert!(islands(&fixture).is_empty(), "{:?}", islands(&fixture));
    // Cutting every weak joint, it is an island of its own.
    let cut = IslandOptions {
        weak_joint_keep_mm2: 0.0,
        ..IslandOptions::default()
    };
    assert_eq!(islands_with(&fixture, &cut).len(), 1);
    // A 6 x 6 square (0.09 mm²) is cut off as before.
    let larger = FixtureBuilder::new(8)
        .screen(160, 120, 0.05)
        .model(.., Shape::rect(20.0, 20.0, 60.0, 60.0))
        .model(3.., Shape::rect(60.0, 60.0, 66.0, 66.0))
        .build();
    assert_eq!(islands(&larger).len(), 1);
}

/// #235: an island over model across a gap thinner than a support offers to
/// join it, and the join cures that gap.
#[test]
fn an_island_just_above_the_model_offers_to_join_it() {
    // A block on the plate to layer 2, nothing at layers 3 and 4 (0.10 mm),
    // and an 8 x 8 block from layer 5 over it.
    let fixture = FixtureBuilder::new(9)
        .model(0..3, square(60, 60, 8))
        .model(5.., square(60, 60, 8))
        .build();
    let input = fixture.input();
    let regions = fixture.regions();
    let ctx = ReviewContext::new(&input, &regions);
    let (findings, joins) = run_with_joins(&ctx, &IslandOptions::default());
    let finding = only(&findings).clone();
    assert_eq!(finding.title, "Unsupported island");
    let Some(SuggestedFix::JoinBelow { at, join }) = finding.fix else {
        panic!("{:?}", finding.fix)
    };
    assert_eq!(at, fixture.input().underside(5, finding.focus));
    assert_eq!(join, 0);
    assert_eq!(joins.len(), 1);
    assert!(
        finding.detail.ends_with(
            "Model lies 0.100 mm below it, too close for a support: curing that gap joins it to the model."
        ),
        "{}",
        finding.detail
    );
    // The gap's two layers, under the whole block: 0.64 mm² x 0.10 mm.
    let fill = &joins[0];
    assert_eq!(fill.slabs.len(), 2);
    assert!((fill.slabs[0].bottom_mm - 0.15).abs() < 1e-9);
    assert!((fill.slabs[1].top_mm - 0.25).abs() < 1e-9);
    assert!(
        (fill.volume_mm3() - 0.064).abs() < 1e-9,
        "{}",
        fill.volume_mm3()
    );
    // Over a support, never: it would glue the part to its support.
    let on_support = FixtureBuilder::new(9)
        .support(0..3, square(60, 60, 8))
        .model(5.., square(60, 60, 8))
        .build();
    let finding = only(&islands(&on_support)).clone();
    assert!(matches!(finding.fix, Some(SuggestedFix::AddSupport { .. })));
    // Across a thicker gap (0.15 mm), a support.
    let far = FixtureBuilder::new(9)
        .model(0..3, square(60, 60, 8))
        .model(6.., square(60, 60, 8))
        .build();
    let finding = only(&islands(&far)).clone();
    assert!(matches!(finding.fix, Some(SuggestedFix::AddSupport { .. })));
}

#[test]
fn a_weak_part_on_an_island_and_a_tip_is_an_island_of_its_own() {
    // A 4 x 4 blob in mid-air at layer 3, and from layer 4 a 30 x 30 plate
    // that rests on the blob and on a 4 x 4 tip from the plate. The plate is
    // weak even on everything below it (16 + 16 of 900 pixels), and it rests
    // on something sound, so it is a start of its own beside the blob.
    let fixture = FixtureBuilder::new(8)
        .model(3..4, square(60, 60, 4))
        .support(0..5, square(70, 50, 4))
        .model(4.., square(50, 40, 30))
        .build();
    let findings = islands(&fixture);
    assert_eq!(findings.len(), 2, "{findings:#?}");
    assert_eq!(findings[0].first_layer, 3);
    assert_eq!(findings[0].severity, Severity::Error);
    assert_eq!(findings[1].first_layer, 4);
    assert_eq!(findings[1].severity, Severity::Warning);
    assert_eq!(
        findings[1].detail,
        "9.00 mm² of model rests on only 0.320 mm² of cured resin and support tips (3.55 % supported, under the 8.33 % margin for that mix)."
    );
    for finding in &findings {
        assert_focus_rests_on_nothing(&fixture, finding);
    }
}

#[test]
fn a_part_touching_the_layer_below_only_diagonally_is_an_island() {
    // Pixels link across layers only straight up: a block shifted by its
    // whole width rests on nothing, even though its corner meets the one
    // below, which is sound.
    let fixture = FixtureBuilder::new(4)
        .model(0..2, square(50, 50, 5))
        .model(2.., square(55, 55, 5))
        .build();
    let finding = only(&islands(&fixture)).clone();
    assert_eq!(finding.severity, Severity::Error);
    assert_eq!(finding.first_layer, 2);
}

#[test]
fn an_island_on_the_last_layer_is_reported() {
    let fixture = FixtureBuilder::new(5)
        .model(.., square(20, 20, 10))
        .model(4..5, square(120, 60, 5))
        .build();
    let finding = only(&islands(&fixture)).clone();
    assert_eq!((finding.first_layer, finding.last_layer), (4, 4));
}

// Severity.

#[test]
fn a_large_island_on_one_pixel_is_an_error_like_a_speck() {
    // A 40 x 40 slab whose corner pixel rests on a pillar (1 of 1600 pixels)
    // and a 2 x 2 speck with nothing below: both have next to nothing
    // holding them.
    let fixture = FixtureBuilder::new(8)
        .model(0..4, square(20, 20, 10))
        .model(4.., square(29, 29, 40))
        .model(4..5, square(150, 100, 2))
        .build();
    let findings = islands(&fixture);
    assert_eq!(findings.len(), 2, "{findings:#?}");
    assert!(
        findings.iter().all(|f| f.severity == Severity::Error),
        "{findings:#?}"
    );
}

// Focus.

#[test]
fn a_ring_island_is_focused_on_the_ring() {
    // The centroid of a ring lies in its hole; the focus must be on the
    // ring itself.
    let fixture = FixtureBuilder::new(5)
        .model(2.., Shape::ring(100.0, 60.0, 20.0, 14.0))
        .build();
    let finding = only(&islands(&fixture)).clone();
    assert_focus_rests_on_nothing(&fixture, &finding);
    // Midway across the ring's width, about 17 pixels from its centre.
    let [x, y] = focus_pixel(&fixture, &finding);
    let r = (f64::from(x) + 0.5 - 100.0).hypot(f64::from(y) + 0.5 - 60.0);
    assert!((15.5..=18.5).contains(&r), "{r}");
}

#[test]
fn the_focus_follows_the_mirror() {
    let build = |mirror: Mirror| {
        FixtureBuilder::new(4)
            .mirror(mirror)
            .model(2.., square(30, 20, 6))
            .build()
    };
    let plain = build(Mirror::default());
    let mirrored = build(Mirror { x: true, y: false });
    let a = only(&islands(&plain)).focus;
    let b = only(&islands(&mirrored)).focus;
    assert_eq!(a, at_pixel(&plain, 2, 32, 22));
    assert_eq!(b, at_pixel(&mirrored, 2, 32, 22));
    assert!((a.x_mm + b.x_mm).abs() < 1e-9, "{a:?} {b:?}");
    assert_eq!(a.y_mm, b.y_mm);
}

// Noise and options.

#[test]
fn specks_under_the_minimum_area_are_ignored() {
    // One pixel and a 3-pixel bar in mid-air, each on a single layer.
    let fixture = FixtureBuilder::new(4)
        .model(2..3, square(50, 50, 1))
        .model(2..3, Shape::rect(80.0, 50.0, 83.0, 51.0))
        .build();
    assert!(islands(&fixture).is_empty(), "{:#?}", islands(&fixture));
}

#[test]
fn an_island_of_exactly_the_minimum_area_is_reported() {
    let fixture = FixtureBuilder::new(4)
        .model(2..3, square(50, 50, 2))
        .build();
    assert_eq!(only(&islands(&fixture)).severity, Severity::Error);
}

#[test]
fn a_tip_that_starts_with_one_pixel_is_reported_from_that_pixel() {
    // The single pixel where the tip starts is under the minimum, but the
    // island it starts grows past it, so the finding starts there.
    let fixture = FixtureBuilder::new(6)
        .model(2..3, square(60, 60, 1))
        .model(3..4, square(59, 59, 3))
        .model(4.., square(58, 58, 5))
        .build();
    let finding = only(&islands(&fixture)).clone();
    assert_eq!((finding.first_layer, finding.last_layer), (2, 5));
    assert_eq!(finding.focus, at_pixel(&fixture, 2, 60, 60));
}

#[test]
fn stray_pixels_that_merge_into_one_tip_are_one_island() {
    // Two single pixels at layer 2 under a 5 x 5 tip at layer 3: both starts
    // are specks, so the island is reported once.
    let fixture = FixtureBuilder::new(6)
        .model(2..3, square(60, 60, 1))
        .model(2..3, square(62, 61, 1))
        .model(3.., square(59, 59, 5))
        .build();
    let finding = only(&islands(&fixture)).clone();
    assert_eq!(finding.first_layer, 2);
}

#[test]
fn the_options_move_the_margins_and_the_minimum() {
    // 36 of 100 pixels rest on a support pillar: held at the 5 % tip margin,
    // an island at 50 %.
    let fixture = FixtureBuilder::new(8)
        .model(4.., square(60, 40, 10))
        .support(0..5, square(62, 42, 6))
        .build();
    let strict_tips = IslandOptions {
        min_tip_supported_fraction: 0.5,
        ..IslandOptions::default()
    };
    assert_eq!(
        only(&islands_with(&fixture, &strict_tips)).detail,
        "1.00 mm² of model rests on only 0.360 mm² of support tips (36 % supported, under the 50 % margin for tips)."
    );
    // The same on a model post: held at 25 %, an island at 50 %.
    let post = FixtureBuilder::new(8)
        .model(0..4, square(62, 42, 6))
        .model(4.., square(60, 40, 10))
        .build();
    assert!(islands(&post).is_empty());
    let strict = IslandOptions {
        min_supported_fraction: 0.5,
        ..IslandOptions::default()
    };
    assert_eq!(
        only(&islands_with(&post, &strict)).detail,
        "1.00 mm² of model rests on only 0.360 mm² of cured resin (36 % supported, under the 50 % margin)."
    );
    // A margin of 0 turns the check off; a minimum over the island's size
    // hides it.
    let floating = FixtureBuilder::new(4).model(2.., square(50, 50, 5)).build();
    let off = IslandOptions {
        min_supported_fraction: 0.0,
        ..IslandOptions::default()
    };
    assert!(islands_with(&floating, &off).is_empty());
    let large_only = IslandOptions {
        min_area_px: 26,
        ..IslandOptions::default()
    };
    assert!(islands_with(&floating, &large_only).is_empty());
    // A tip margin of 0 makes any tip enough.
    let thin_tip = FixtureBuilder::new(8)
        .model(4.., square(70, 40, 20))
        .support(0..5, square(79, 49, 1))
        .build();
    let any_tip = IslandOptions {
        min_tip_supported_fraction: 0.0,
        ..IslandOptions::default()
    };
    assert_eq!(islands(&thin_tip).len(), 1);
    assert!(islands_with(&thin_tip, &any_tip).is_empty());
}

#[test]
fn margins_out_of_range_are_clamped_and_nan_falls_back() {
    // A margin over 1 asks for more than the whole area: it is clamped to
    // 1, so a block standing on the plate is still held.
    let block = FixtureBuilder::new(6).model(.., square(40, 40, 10)).build();
    let over = IslandOptions {
        min_supported_fraction: 2.0,
        min_tip_supported_fraction: 7.0,
        ..IslandOptions::default()
    };
    assert!(islands_with(&block, &over).is_empty());
    let sanitized = over.sanitized();
    assert_eq!(sanitized.min_supported_fraction, 1.0);
    assert_eq!(sanitized.min_tip_supported_fraction, 1.0);
    // Not a number: the default margin, not a disabled check.
    let floating = FixtureBuilder::new(4).model(2.., square(50, 50, 5)).build();
    let nan = IslandOptions {
        min_supported_fraction: f64::NAN,
        min_tip_supported_fraction: f64::NAN,
        cure_depth_mm: f64::NAN,
        weak_joint_keep_mm2: f64::NAN,
        minimal_height_mm: f64::NAN,
        minimal_volume_mm3: f64::NAN,
        minimal_section_mm2: f64::NAN,
        join_gap_mm: f64::NAN,
        support_climb_mm: f64::NAN,
        tip_contact_mm2: f64::NAN,
        ..IslandOptions::default()
    };
    assert_eq!(nan.sanitized(), IslandOptions::default());
    assert_eq!(islands_with(&floating, &nan).len(), 1);
    let negative = IslandOptions {
        min_supported_fraction: -1.0,
        ..IslandOptions::default()
    };
    assert_eq!(negative.sanitized().min_supported_fraction, 0.0);
}

#[test]
fn a_margin_that_is_not_a_whole_percent_never_reads_as_the_fraction() {
    // A 10 x 10 slab on 33 pixels, under a margin of a third: 33 % against
    // 33.33 %, shown with the decimals that tell them apart.
    let fixture = FixtureBuilder::new(8)
        .model(0..4, Shape::rect(60.0, 40.0, 70.0, 43.0))
        .model(0..4, Shape::rect(60.0, 43.0, 63.0, 44.0))
        .model(4.., square(60, 40, 10))
        .build();
    let third = IslandOptions {
        min_supported_fraction: 1.0 / 3.0,
        ..IslandOptions::default()
    };
    assert_eq!(
        only(&islands_with(&fixture, &third)).detail,
        "1.00 mm² of model rests on only 0.330 mm² of cured resin (33.00 % supported, under the 33.33 % margin)."
    );
}

#[test]
fn the_defaults_are_the_documented_constants() {
    let options = IslandOptions::default();
    assert_eq!(options.min_supported_fraction, MIN_SUPPORTED_FRACTION);
    assert_eq!(
        options.min_tip_supported_fraction,
        MIN_TIP_SUPPORTED_FRACTION
    );
    assert_eq!(options.min_area_px, MIN_ISLAND_AREA_PX);
    assert_eq!(MIN_SUPPORTED_FRACTION, 0.25);
    assert_eq!(MIN_TIP_SUPPORTED_FRACTION, 0.05);
    assert_eq!(options.sanitized(), options);
}

#[test]
fn the_whole_review_lists_the_islands() {
    let fixture = FixtureBuilder::new(6)
        .model(.., Shape::rect(10.0, 10.0, 40.0, 40.0))
        .model(3.., square(100, 50, 10))
        .build();
    let islands: Vec<Finding> = fixture
        .review()
        .into_iter()
        .filter(|f| f.check == CheckKind::Island)
        .collect();
    assert_eq!(only(&islands).first_layer, 3);
}

#[test]
fn numbers_are_printed_with_three_significant_digits() {
    assert_eq!(significant(0.004_56), "0.00456");
    assert_eq!(significant(0.123_4), "0.123");
    assert_eq!(significant(1.0), "1.00");
    assert_eq!(significant(12.34), "12.3");
    assert_eq!(significant(1234.4), "1234");
    assert_eq!(significant(0.0), "0");
    // Rounding that carries into the next power of ten keeps three digits.
    assert_eq!(significant(9.9996), "10.0");
    assert_eq!(significant(9.999_97), "10.0");
    assert_eq!(significant(0.999_55), "1.00");
    assert_eq!(significant(99.96), "100");
    let pair = |a: &str, b: &str| (a.to_string(), b.to_string());
    assert_eq!(percents(0, 10, 0.25), pair("0 %", "25 %"));
    assert_eq!(percents(1, 1000, 0.25), pair("<1 %", "25 %"));
    assert_eq!(percents(249, 1000, 0.25), pair("24 %", "25 %"));
    assert_eq!(percents(29, 100, 0.5), pair("29 %", "50 %"));
    assert_eq!(percents(33, 100, 1.0 / 3.0), pair("33.00 %", "33.33 %"));
    assert_eq!(percents(3, 100, 0.035), pair("3.0 %", "3.5 %"));
}
