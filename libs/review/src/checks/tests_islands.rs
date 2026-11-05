/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Parts and islands (#84, #432).
//!
//! The default fake printer has 0.1 mm pixels (0.01 mm² each) and 0.05 mm
//! layers. A link of 5 pixels is 0.05 mm², and a tip reaches 1 pixel from a
//! part's pixels.

use super::*;
use crate::finding::RoleScope;
use crate::regions::SweepOptions;
use crate::runner::ReviewReport;
use crate::test_support::{Fixture, FixtureBuilder, Shape};

fn report(fixture: &Fixture) -> ReviewReport {
    fixture.report()
}

fn findings_of(fixture: &Fixture, check: CheckKind) -> Vec<Finding> {
    report(fixture)
        .findings
        .into_iter()
        .filter(|finding| finding.check == check)
        .collect()
}

/// The layers of every island finding.
fn islands(fixture: &Fixture) -> Vec<(usize, usize)> {
    findings_of(fixture, CheckKind::Island)
        .iter()
        .map(|finding| (finding.first_layer, finding.last_layer))
        .collect()
}

/// A body on the plate, to stand things on.
fn body(layers: usize) -> FixtureBuilder {
    FixtureBuilder::new(layers).model(0..layers, Shape::rect(20.0, 20.0, 60.0, 50.0))
}

/// A block that starts on layer `start` and stays to the top of `layers`.
fn with_block(builder: FixtureBuilder, start: usize, layers: usize) -> FixtureBuilder {
    builder.model(start..layers, Shape::rect(100.0, 40.0, 130.0, 70.0))
}

#[test]
fn what_stands_on_the_plate_is_clean() {
    let fixture = body(8).build();
    let report = report(&fixture);
    assert!(report.findings.is_empty(), "{:?}", report.findings);
    assert_eq!(report.parts.len(), 1);
    assert_eq!(report.parts[0].held, Held::Plate);
    assert!(report.erased.is_empty());
}

#[test]
fn a_block_on_the_second_layer_rests_on_the_plate() {
    let fixture = with_block(body(8), 1, 8).build();
    assert!(islands(&fixture).is_empty());
}

#[test]
fn a_block_in_mid_air_is_an_island_with_a_support_at_its_first_pixels() {
    let fixture = with_block(body(10), 4, 10).build();
    let findings = findings_of(&fixture, CheckKind::Island);
    assert_eq!(findings.len(), 1, "{findings:?}");
    let island = &findings[0];
    assert_eq!((island.first_layer, island.last_layer), (4, 9));
    assert_eq!(island.severity, Severity::Error);
    assert_eq!(island.scope, RoleScope::Model);
    assert_eq!(island.title, "Unsupported island");
    let Some(SuggestedFix::AddSupport { at }) = island.fix else {
        panic!("{:?}", island.fix);
    };
    assert!((at.z_mm - 0.2).abs() < 1e-9, "{at:?}");
    assert!(island.footprint.is_some());
    assert!((island.largest_mm2.expect("measured") - 9.0).abs() < 1e-9);
}

#[test]
fn size_does_not_matter_for_an_island() {
    // Four pixels that last: an island all the same.
    let fixture = body(10)
        .model(4..10, Shape::rect(100.0, 40.0, 102.0, 42.0))
        .build();
    assert_eq!(islands(&fixture), [(4, 9)]);
}

#[test]
fn a_tip_under_its_own_pixels_holds_a_part() {
    let fixture = with_block(body(10), 4, 10).tip(115.0, 55.0, 4, 3.0).build();
    assert!(islands(&fixture).is_empty());
    let report = report(&fixture);
    assert_eq!(report.parts[1].held, Held::Tip);
    assert!(report.parts[1].tip_contact_px > 0);
}

/// #433: what an island says of a tip, once the review is over, is what the
/// next review says of a plate with that tip on it, wherever the tip stands.
#[test]
fn an_island_judges_a_tip_as_the_next_review_does() {
    let bare = with_block(body(10), 4, 10).build();
    let report = report(&bare);
    let island = &report.islands[0];
    // Under it, beside its edge, far from it, stopping short of the layer
    // below it, and thin at its very edge.
    let tips = [
        (115.0, 55.0, 4, 3.0),
        (129.0, 55.0, 4, 3.0),
        (136.0, 55.0, 4, 6.0),
        (150.0, 55.0, 4, 3.0),
        (115.0, 55.0, 2, 3.0),
        (131.0, 55.0, 4, 1.0),
    ];
    let mut verdicts = Vec::new();
    for (x, y, layer, radius) in tips {
        let tipped = with_block(body(10), 4, 10).tip(x, y, layer, radius).build();
        let reviewed = islands(&tipped).is_empty();
        assert_eq!(
            island.start.held_by(tipped.input().tips()),
            reviewed,
            "a tip at {x}, {y} up to layer {layer}"
        );
        verdicts.push(reviewed);
    }
    assert!(verdicts.contains(&true) && verdicts.contains(&false));
}

#[test]
fn a_tip_that_stops_short_of_the_layer_below_holds_nothing() {
    let fixture = with_block(body(10), 4, 10).tip(115.0, 55.0, 2, 3.0).build();
    assert_eq!(islands(&fixture), [(4, 9)]);
}

#[test]
fn a_tip_beside_a_part_holds_nothing() {
    // The block's edge is at x = 130; the tip stands 2 mm away.
    let fixture = with_block(body(10), 4, 10).tip(150.0, 55.0, 4, 3.0).build();
    assert_eq!(islands(&fixture), [(4, 9)]);
}

#[test]
fn any_pixel_of_a_tip_holds_a_part_unless_a_share_is_asked_for() {
    // A 6-pixel tip beside the block's edge at x = 130: a few of its pixels
    // lie within a pixel of the block, out of a section of over 100.
    let fixture = with_block(body(10), 4, 10).tip(136.0, 55.0, 4, 6.0).build();
    let contact = report(&fixture).parts[1].tip_contact_px;
    assert!(contact > 0 && contact < 50, "{contact}");
    assert!(islands(&fixture).is_empty());

    let input = fixture.input();
    let regions = PlateRegions::compute(&input, &SweepOptions::default());
    let half = IslandOptions {
        min_tip_share: 0.5,
        ..IslandOptions::default()
    };
    let found = run(
        &ReviewContext::new(&input, &regions),
        &half,
        &SweepOptions::default(),
    );
    assert_eq!(found.islands.len(), 1);
    assert_eq!(found.islands[0].first_layer, 4);
}

#[test]
fn a_tip_that_reaches_a_part_with_most_of_itself_holds_it() {
    let fixture = with_block(body(10), 4, 10).tip(129.0, 55.0, 4, 3.0).build();
    assert!(islands(&fixture).is_empty());
}

#[test]
fn a_part_smaller_than_the_tip_stands_on_it() {
    // Two pixels on a tip of 113: all of the part is over it.
    let fixture = body(10)
        .model(4..10, Shape::rect(115.0, 55.0, 117.0, 57.0))
        .tip(116.0, 56.0, 4, 6.0)
        .build();
    assert!(islands(&fixture).is_empty());
}

#[test]
fn a_tip_under_the_model_below_is_no_contact() {
    // The layer below has model where the tip stands: the tip holds that,
    // and the fine neck it makes does not carry what comes above.
    let neck = Shape::rect(115.0, 55.0, 117.0, 57.0);
    let fixture = with_block(body(10).model(3..4, neck), 4, 10)
        .tip(116.0, 56.0, 3, 1.0)
        .tip(116.0, 56.0, 4, 1.0)
        .build();
    let report = report(&fixture);
    let start = report
        .parts
        .iter()
        .find(|part| part.first_layer == 4)
        .expect("the block is a part of its own");
    assert_eq!(start.held, Held::Nothing);
}

#[test]
fn a_part_on_a_fine_neck_starts_on_its_own() {
    // A 3 x 1 pixel neck (0.03 mm²) under a block: no link, whatever it is
    // joined to below.
    let fixture = FixtureBuilder::new(10)
        .model(0..4, Shape::rect(40.0, 40.0, 103.0, 70.0))
        .model(4..10, Shape::rect(100.0, 69.0, 140.0, 100.0))
        .build();
    let report = report(&fixture);
    let start = report.parts.iter().find(|part| part.first_layer == 4);
    assert!(start.is_some(), "{:?}", report.parts);
    assert_eq!(islands(&fixture), [(4, 9)]);
}

#[test]
fn a_tip_under_the_neighbour_does_not_hold_a_part_on_a_neck() {
    let fixture = FixtureBuilder::new(10)
        .model(0..4, Shape::rect(40.0, 40.0, 103.0, 70.0))
        .model(4..10, Shape::rect(100.0, 69.0, 140.0, 100.0))
        .tip(60.0, 55.0, 4, 3.0)
        .build();
    assert_eq!(islands(&fixture), [(4, 9)]);
}

#[test]
fn a_link_of_the_minimum_area_holds_a_part() {
    // A 5 x 1 pixel link: 0.05 mm², exactly the constant.
    let fixture = FixtureBuilder::new(10)
        .model(0..4, Shape::rect(40.0, 40.0, 105.0, 70.0))
        .model(4..10, Shape::rect(100.0, 69.0, 140.0, 100.0))
        .build();
    assert!(islands(&fixture).is_empty());
}

#[test]
fn a_thin_pin_on_a_surface_rests_on_it_whole() {
    // Under the minimum link, but all of it stands on the model below.
    let fixture = FixtureBuilder::new(10)
        .model(0..4, Shape::rect(40.0, 40.0, 90.0, 70.0))
        .model(4..10, Shape::rect(60.0, 50.0, 62.0, 52.0))
        .build();
    assert!(islands(&fixture).is_empty());
    assert_eq!(report(&fixture).parts.len(), 1);
}

#[test]
fn a_part_lasts_until_it_joins_the_body() {
    // A block from layer 4, joined to the body by a bridge on layer 8.
    let fixture = with_block(body(12), 4, 12)
        .model(8..12, Shape::rect(55.0, 40.0, 105.0, 45.0))
        .build();
    assert_eq!(islands(&fixture), [(4, 7)]);
    let report = report(&fixture);
    let island = &report.islands[0];
    assert_eq!(island.first_layer, 4);
    assert_eq!(island.joins_on, Some(8));
    assert_eq!(island.growth_mm2.len(), 4);
    assert!((island.first_area_mm2 - 9.0).abs() < 1e-9);
    assert_eq!(island.first_pixels_mm.len(), 900);
}

#[test]
fn the_larger_part_goes_on_where_two_meet() {
    // Two blocks in mid-air that merge on layer 6: the small one ends there.
    let fixture = body(12)
        .model(4..12, Shape::rect(100.0, 40.0, 130.0, 70.0))
        .model(4..6, Shape::rect(135.0, 40.0, 140.0, 45.0))
        .model(6..12, Shape::rect(125.0, 40.0, 140.0, 45.0))
        .build();
    let report = report(&fixture);
    assert_eq!(report.islands.len(), 1, "{:?}", report.parts);
    assert_eq!(report.islands[0].joins_on, None);
    let small = report
        .parts
        .iter()
        .find(|part| part.first_layer == 4 && part.largest_px() < 100)
        .expect("the small block");
    assert_eq!(small.layers(), 2);
    assert!(small.joined.is_some());
}

#[test]
fn a_held_part_goes_on_over_a_larger_island() {
    let fixture = body(12)
        .model(4..12, Shape::rect(100.0, 40.0, 130.0, 70.0))
        .model(4..7, Shape::rect(135.0, 40.0, 140.0, 45.0))
        .model(7..12, Shape::rect(125.0, 40.0, 140.0, 45.0))
        .tip(137.0, 42.0, 4, 2.0)
        .build();
    let report = report(&fixture);
    assert_eq!(islands(&fixture), [(4, 6)], "{:?}", report.parts);
    assert_eq!(report.islands[0].joins_on, Some(7));
}

#[test]
fn what_lasts_under_three_layers_is_erased() {
    let fixture = with_block(body(10), 4, 6).build();
    let report = report(&fixture);
    assert!(
        !report.findings.iter().any(|f| f.check == CheckKind::Island),
        "{:?}",
        report.findings
    );
    let note = report
        .findings
        .iter()
        .find(|f| f.check == CheckKind::CleanUp)
        .expect("a note");
    assert_eq!(note.title, "Debris removed");
    assert_eq!(note.severity, Severity::Info);
    assert!(note.detail.contains("1 part "), "{}", note.detail);
    assert!(note.detail.contains("layers 5–6"), "{}", note.detail);
    assert_eq!(report.erased.slabs.len(), 2);
}

#[test]
fn what_leaves_the_layers_leaves_the_raster_and_nothing_else() {
    use crate::fill::erase_stack;
    use crate::input::ReviewLeaf;
    use crate::runs::{area_px, runs_from_raster};
    use sc_sl1cer::raster::rasterize_layers_binary;

    // On layer 4: a spur off the body, and a block of debris on the layer it
    // leaves: their rectangles must not overlap, or a ring would be
    // cancelled twice and light what it should clear.
    let fixture = with_block(
        body(10).model(4..5, Shape::rect(60.0, 30.0, 90.0, 31.0)),
        4,
        6,
    )
    .build();
    let report = report(&fixture);
    assert!(!report.erased.is_empty());
    let input = fixture.input();
    let stacks: Vec<_> = input.leaves().iter().map(|leaf| leaf.stack).collect();
    let cut = erase_stack([&report.erased], &stacks, fixture.schedule()).expect("a stack");
    let merged = fixture.input_with(ReviewLeaf::new(Some("Erased"), &cut));
    for layer in 0..merged.layer_count() {
        let contours: Vec<_> = merged
            .leaves()
            .iter()
            .map(|leaf| &leaf.stack.layers()[layer])
            .collect();
        let runs = runs_from_raster(&rasterize_layers_binary(&contours, merged.pixel_map()));
        // Only the body is left, on every layer.
        assert_eq!(area_px(&runs), 40 * 30, "layer {layer}");
    }
}

#[test]
fn three_layers_are_an_island() {
    let fixture = with_block(body(10), 4, 7).build();
    assert_eq!(islands(&fixture), [(4, 6)]);
    assert!(report(&fixture).erased.is_empty());
}

#[test]
fn a_short_part_a_tip_touches_stays() {
    // Too little to hold it, but erasing it would leave the tip short of
    // what comes above: it stays.
    let fixture = with_block(body(10), 4, 6).tip(136.0, 55.0, 4, 6.0).build();
    let report = report(&fixture);
    assert!(report.erased.is_empty(), "{:?}", report.erased);
    assert!(report.findings.is_empty(), "{:?}", report.findings);
}

#[test]
fn a_held_short_part_is_no_debris() {
    let fixture = with_block(body(10), 4, 6).tip(115.0, 55.0, 4, 3.0).build();
    assert!(report(&fixture).erased.is_empty());
}

#[test]
fn stray_pixels_leave_the_layers_with_a_note() {
    // A one-pixel spur off the body's right edge, for three layers.
    let fixture = body(6)
        .model(1..4, Shape::rect(60.0, 30.0, 70.0, 31.0))
        .build();
    let report = report(&fixture);
    let note = &report.findings[0];
    assert_eq!(note.check, CheckKind::CleanUp);
    assert_eq!(note.title, "Stray pixels removed");
    assert_eq!((note.first_layer, note.last_layer), (1, 3));
    assert!(
        note.detail
            .starts_with("30 stray pixels removed on layers 2–4"),
        "{}",
        note.detail
    );
    assert_eq!(report.erased.slabs.len(), 3);
    assert!((report.erased.volume_mm3() - 30.0 * 0.01 * 0.05).abs() < 1e-9);
}

#[test]
fn a_part_held_only_by_a_one_pixel_neck_in_its_layer_is_cut_there() {
    // A block that joins the body on its own layer through a line of one
    // pixel: the line is stray, so the block starts on its own.
    let fixture = body(10)
        .model(4..10, Shape::rect(60.0, 30.0, 100.0, 31.0))
        .model(4..10, Shape::rect(100.0, 15.0, 130.0, 45.0))
        .build();
    assert_eq!(islands(&fixture), [(4, 9)]);
}

#[test]
fn a_sudden_growth_is_listed_from_the_first_pixels() {
    // 64 pixels, then 900: x14, on a link that holds it.
    let fixture = body(10)
        .model(4..6, Shape::rect(111.0, 51.0, 119.0, 59.0))
        .model(6..10, Shape::rect(100.0, 40.0, 130.0, 70.0))
        .tip(115.0, 55.0, 4, 3.0)
        .build();
    let growth = findings_of(&fixture, CheckKind::SurfaceGrowth);
    assert_eq!(growth.len(), 1, "{growth:?}");
    assert_eq!((growth[0].first_layer, growth[0].last_layer), (6, 6));
    assert_eq!(growth[0].severity, Severity::Warning);
    assert!(growth[0].detail.contains("×14"), "{}", growth[0].detail);
    assert!(growth[0].fix.is_none());
}

#[test]
fn growth_under_the_factor_is_not_listed() {
    let fixture = FixtureBuilder::new(10)
        .model(0..6, Shape::rect(100.0, 40.0, 110.0, 50.0))
        .model(6..10, Shape::rect(100.0, 40.0, 130.0, 70.0))
        .build();
    assert!(findings_of(&fixture, CheckKind::SurfaceGrowth).is_empty());
}

#[test]
fn a_start_over_a_layer_only_supports_stand_on_is_judged() {
    // The first model pixels on layer 4, supports below: no gap.
    let fixture = FixtureBuilder::new(10)
        .model(4..10, Shape::rect(100.0, 40.0, 130.0, 70.0))
        .support_layers(0..10)
        .build();
    assert_eq!(islands(&fixture), [(4, 9)]);
    let held = FixtureBuilder::new(10)
        .model(4..10, Shape::rect(100.0, 40.0, 130.0, 70.0))
        .support_layers(0..10)
        .tip(115.0, 55.0, 4, 3.0)
        .build();
    assert!(islands(&held).is_empty());
}

#[test]
fn a_mirrored_screen_keeps_the_tips_under_their_parts() {
    use sc_print_primitives::Mirror;
    for (x, y) in [(true, false), (false, true), (true, true)] {
        let fixture = with_block(body(10).mirror(Mirror { x, y }), 4, 10)
            .tip(115.0, 55.0, 4, 3.0)
            .build();
        assert!(islands(&fixture).is_empty(), "{x} {y}");
    }
}

#[test]
fn the_result_does_not_depend_on_the_threads() {
    let fixture = with_block(body(40), 4, 40)
        .model(10..30, Shape::rect(140.0, 40.0, 160.0, 60.0))
        .tip(150.0, 50.0, 10, 3.0)
        .build();
    let input = fixture.input();
    let reference = PlateRegions::compute(&input, &SweepOptions::default());
    for threads in [1, 3, 7] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .expect("a pool");
        let swept = pool.install(|| PlateRegions::compute(&input, &SweepOptions::default()));
        assert_eq!(swept.layers(), reference.layers());
    }
}
