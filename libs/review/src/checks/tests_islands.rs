/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Island tests (#84).
//!
//! The fake printer has 0.1 mm pixels (0.01 mm² each) and 0.05 mm layers, so
//! a shape of `n` pixels is `n / 100` mm² and layer `k` is cut at
//! `0.05 * k + 0.025` mm.

use sc_print_primitives::Mirror;

use super::*;
use crate::finding::PlatePoint;
use crate::test_support::{Fixture, FixtureBuilder, LAYER_HEIGHT_MM, Shape};

/// The island findings of `fixture` with the default options.
fn islands(fixture: &Fixture) -> Vec<Finding> {
    fixture.check(|ctx| run(ctx, &IslandOptions::default()))
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

// Clean plates: nothing to report.

#[test]
fn a_block_standing_on_the_plate_has_no_island() {
    // 40 x 30 pixels (4 x 3 mm) on every layer of an 8-layer plate.
    let fixture = FixtureBuilder::new(8)
        .model(.., Shape::rect(40.0, 30.0, 80.0, 60.0))
        .build();
    let findings = fixture.check(|ctx| run(ctx, &IslandOptions::default()));
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn an_empty_plate_has_no_island() {
    assert!(islands(&FixtureBuilder::new(4).build()).is_empty());
}

#[test]
fn parts_on_the_first_layer_are_never_islands() {
    // Layer 0 rests on the plate, whatever its shape; the parts are only one
    // layer tall, so nothing above them can be an island either.
    let fixture = FixtureBuilder::new(3)
        .model(0..1, Shape::rect(10.0, 10.0, 60.0, 40.0))
        .model(0..1, square(100, 20, 1))
        .support(0..1, Shape::circle(150.0, 80.0, 6.0))
        .build();
    assert!(islands(&fixture).is_empty());
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
    // from the plate whose tip enters the block by one layer: 36 of its 100
    // pixels rest on the pillar, over the margin.
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

// Islands that must be reported.

#[test]
fn a_part_starting_in_mid_air_is_an_unsupported_island() {
    // A block on the plate, and a 20 x 10 model bar that starts at layer 4
    // with nothing below it.
    let fixture = FixtureBuilder::new(8)
        .model(.., Shape::rect(10.0, 10.0, 40.0, 40.0))
        .model(4.., Shape::rect(100.0, 60.0, 120.0, 70.0))
        .build();
    let finding = only(&islands(&fixture)).clone();
    assert_eq!(finding.check, CheckKind::Island);
    assert_eq!(finding.severity, Severity::Error);
    assert_eq!(finding.scope, RoleScope::Model);
    assert_eq!((finding.first_layer, finding.last_layer), (4, 4));
    assert_eq!(finding.title, "Unsupported island");
    assert_eq!(
        finding.detail,
        "2.00 mm² of model starts with nothing cured below it (0 % supported)."
    );
    // The bar's pixel nearest its centroid (110, 65) (four pixels tie; the
    // first in scan order wins), on the bar's first layer, cut 0.225 mm
    // above the plate: a support tip goes right there.
    let expected = at_pixel(&fixture, 4, 110, 64);
    assert_eq!(finding.focus, expected);
    assert!((expected.z_mm - LAYER_HEIGHT_MM.mul_add(4.0, 0.5 * LAYER_HEIGHT_MM)).abs() < 1e-9);
    assert_eq!(finding.fix, Some(SuggestedFix::AddSupport { at: expected }));
}

#[test]
fn a_slab_on_a_thin_post_is_a_weakly_supported_island() {
    // A 20 x 20 slab on a 4 x 4 post: 16 of its 400 pixels rest on cured
    // resin.
    let fixture = FixtureBuilder::new(8)
        .model(0..4, square(78, 48, 4))
        .model(4.., square(70, 40, 20))
        .build();
    let finding = only(&islands(&fixture)).clone();
    assert_eq!(finding.severity, Severity::Warning);
    assert_eq!(finding.scope, RoleScope::Model);
    assert_eq!((finding.first_layer, finding.last_layer), (4, 4));
    assert_eq!(finding.title, "Weakly supported island");
    assert_eq!(
        finding.detail,
        "4.00 mm² of model rests on only 0.160 mm² of cured resin (4 % supported, under the 25 % margin)."
    );
    assert_eq!(finding.focus, at_pixel(&fixture, 4, 80, 49));
    assert!(matches!(finding.fix, Some(SuggestedFix::AddSupport { .. })));
}

#[test]
fn a_model_on_too_thin_a_support_tip_is_weakly_supported() {
    // The same slab held by a 2 x 2 support tip that enters it: the support
    // counts as cured resin below, but 4 pixels of 400 are not enough. The
    // island concerns the model, although its region has support pixels.
    let fixture = FixtureBuilder::new(8)
        .model(4.., square(70, 40, 20))
        .support(0..5, square(79, 49, 2))
        .build();
    let finding = only(&islands(&fixture)).clone();
    assert_eq!(finding.severity, Severity::Warning);
    assert_eq!(finding.scope, RoleScope::Model);
    assert_eq!(finding.title, "Weakly supported island");
    assert!(
        finding
            .detail
            .contains("rests on only 0.0400 mm² of cured resin (1 % supported"),
        "{}",
        finding.detail
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
    assert_eq!((finding.first_layer, finding.last_layer), (3, 3));
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
    // A support platform that widens from a 2 x 2 stem to 12 x 12 at once.
    let fixture = FixtureBuilder::new(6)
        .support(0..3, square(150, 60, 2))
        .support(3.., square(145, 55, 12))
        .build();
    let finding = only(&islands(&fixture)).clone();
    assert_eq!(finding.severity, Severity::Warning);
    assert_eq!(finding.scope, RoleScope::Support);
    assert_eq!(finding.title, "Weakly held support");
    assert_eq!(
        finding.detail,
        "1.44 mm² of support rests on only 0.0400 mm² of cured resin (2 % supported, under the 25 % margin)."
    );
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

#[test]
fn a_growing_tip_is_one_island_over_its_layers() {
    // An upside-down pyramid starting at layer 3: 2 x 2, then 6 x 6 (4 of
    // its 36 pixels rest on the tip: still an island), then 10 x 10 (36 of
    // 100: held). One island over layers 3 and 4, not two.
    let fixture = FixtureBuilder::new(8)
        .model(3..4, square(99, 59, 2))
        .model(4..5, square(97, 57, 6))
        .model(5.., square(95, 55, 10))
        .build();
    let finding = only(&islands(&fixture)).clone();
    assert_eq!((finding.first_layer, finding.last_layer), (3, 4));
    // The weakest layer is the first, with nothing below it.
    assert_eq!(finding.severity, Severity::Error);
    assert_eq!(
        finding.detail,
        "0.0400 mm² of model starts with nothing cured below it (0 % supported)."
    );
    assert_eq!(finding.focus, at_pixel(&fixture, 3, 100, 59));
}

#[test]
fn a_shallow_ramp_in_mid_air_is_one_island() {
    // A 10 x 10 block that slides 8 pixels sideways per layer: every layer
    // rests on 20 of its 100 pixels, so all of them are island layers, joined
    // by their overlaps into one island.
    let mut builder = FixtureBuilder::new(8).model(0..1, square(20, 50, 10));
    for layer in 1..8 {
        builder = builder.model(layer..=layer, square(20 + 8 * layer as u32, 50, 10));
    }
    let finding = only(&islands(&builder.build())).clone();
    assert_eq!((finding.first_layer, finding.last_layer), (1, 7));
    assert_eq!(finding.severity, Severity::Warning);
    assert!(
        finding
            .detail
            .starts_with("1.00 mm² of model rests on only 0.200 mm²")
    );
    assert!(
        finding.detail.contains("(20 % supported"),
        "{}",
        finding.detail
    );
}

#[test]
fn the_weakest_layer_sets_the_numbers_and_the_severity() {
    // A 10 x 10 slab on a 3 x 3 post (9 %), then a 40 x 40 slab on it (6 %):
    // one island over layers 3 and 4 whose weakest layer is the second.
    let fixture = FixtureBuilder::new(8)
        .model(0..3, square(54, 54, 3))
        .model(3..4, square(50, 50, 10))
        .model(4.., square(35, 35, 40))
        .build();
    let finding = only(&islands(&fixture)).clone();
    assert_eq!((finding.first_layer, finding.last_layer), (3, 4));
    assert_eq!(finding.severity, Severity::Warning);
    assert_eq!(
        finding.detail,
        "On layer 5, 16.0 mm² of model rests on only 1.00 mm² of cured resin (6 % supported, under the 25 % margin)."
    );
    // The focus stays where the island starts.
    assert_eq!(finding.focus, at_pixel(&fixture, 3, 55, 54));
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
    // A one-layer island at layer 2, nothing at layer 3, another at layer 4
    // over the same pixels: they never touch, so they are two islands.
    let fixture = FixtureBuilder::new(6)
        .model(2..3, square(60, 60, 8))
        .model(4.., square(60, 60, 8))
        .build();
    let findings = islands(&fixture);
    let layers: Vec<(usize, usize)> = findings
        .iter()
        .map(|f| (f.first_layer, f.last_layer))
        .collect();
    assert_eq!(layers, vec![(2, 2), (4, 4)]);
}

#[test]
fn a_part_touching_the_layer_below_only_diagonally_is_an_island() {
    // Pixels link across layers only straight up: a block shifted by its
    // whole width rests on nothing, even though its corner meets the one
    // below.
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

#[test]
fn a_ring_island_is_focused_on_the_ring() {
    // The centroid of a ring lies in its hole; the focus must be on the
    // ring itself.
    let fixture = FixtureBuilder::new(5)
        .model(2.., Shape::ring(100.0, 60.0, 20.0, 14.0))
        .build();
    let finding = only(&islands(&fixture)).clone();
    let regions = fixture.regions();
    let ring = regions.layer(2).solid[0];
    assert_eq!(finding.focus, fixture.input().focus(2, ring.inside_point()));
    let [x, y] = ring.inside_px;
    let pixels = fixture.layer_pixels(2);
    assert!(
        pixels
            .solid
            .iter()
            .any(|run| run.row == y && run.x0 <= x && x < run.x1),
        "the focus pixel is solid"
    );
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
    assert_eq!(a, at_pixel(&plain, 2, 33, 22));
    assert_eq!(b, at_pixel(&mirrored, 2, 33, 22));
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
    // The minimum applies to the island's largest layer, so the single
    // pixel where the tip starts is kept, and the finding starts there.
    let fixture = FixtureBuilder::new(6)
        .model(2..3, square(60, 60, 1))
        .model(3..4, square(59, 59, 3))
        .model(4.., square(58, 58, 5))
        .build();
    let finding = only(&islands(&fixture)).clone();
    assert_eq!((finding.first_layer, finding.last_layer), (2, 3));
    assert_eq!(finding.focus, at_pixel(&fixture, 2, 60, 60));
}

#[test]
fn the_options_move_the_margin_and_the_minimum() {
    // 36 of 100 pixels rest on the pillar: held at 25 %, an island at 50 %.
    let fixture = FixtureBuilder::new(8)
        .model(4.., square(60, 40, 10))
        .support(0..5, square(62, 42, 6))
        .build();
    let strict = IslandOptions {
        min_supported_fraction: 0.5,
        ..IslandOptions::default()
    };
    let findings = fixture.check(|ctx| run(ctx, &strict));
    let finding = only(&findings);
    assert_eq!(finding.severity, Severity::Warning);
    assert!(
        finding
            .detail
            .ends_with("(36 % supported, under the 50 % margin)."),
        "{}",
        finding.detail
    );
    let lenient = IslandOptions {
        min_supported_fraction: 0.0,
        ..IslandOptions::default()
    };
    let floating = FixtureBuilder::new(4).model(2.., square(50, 50, 5)).build();
    assert!(floating.check(|ctx| run(ctx, &lenient)).is_empty());
    let large_only = IslandOptions {
        min_area_px: 26,
        ..IslandOptions::default()
    };
    assert!(floating.check(|ctx| run(ctx, &large_only)).is_empty());
}

#[test]
fn the_defaults_are_the_documented_constants() {
    let options = IslandOptions::default();
    assert_eq!(options.min_supported_fraction, MIN_SUPPORTED_FRACTION);
    assert_eq!(options.min_area_px, MIN_ISLAND_AREA_PX);
    assert_eq!(MIN_SUPPORTED_FRACTION, 0.25);
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
    assert_eq!(percent(0, 10), "0 %");
    assert_eq!(percent(1, 1000), "<1 %");
    assert_eq!(percent(249, 1000), "24 %");
    assert_eq!(percent(29, 100), "29 %");
}
