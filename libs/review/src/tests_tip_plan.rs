/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Where the tips of the islands go (#437).
//!
//! The default fake printer has 0.1 mm pixels (0.01 mm² each) and 0.05 mm
//! layers.

use super::*;
use crate::test_support::{FixtureBuilder, Shape};

/// A body on the plate, to stand things on.
fn body(layers: usize) -> FixtureBuilder {
    FixtureBuilder::new(layers).model(0..layers, Shape::rect(20.0, 20.0, 60.0, 50.0))
}

/// #437: the tips of a region that grew go on the surface it gained, apart
/// from each other, not on what stood there.
#[test]
fn the_tips_of_a_growth_share_out_what_it_gained() {
    let report = body(10)
        .model(4..6, Shape::rect(111.0, 51.0, 119.0, 59.0))
        .model(6..10, Shape::rect(100.0, 40.0, 130.0, 70.0))
        .tip(115.0, 55.0, 4, 3.0)
        .build()
        .report();
    let plan = plan_all_tips(&[], &report.growths, &TipPlanOptions::for_tip(0.3));
    let tips: Vec<&PlannedTip> = plan.of_growth(0).collect();
    assert_eq!(tips.len(), GROWTH_TIPS);
    assert!(plan.no_room.is_empty());
    let (a, b) = (tips[0].at, tips[1].at);
    let apart = (a.x_mm - b.x_mm).hypot(a.y_mm - b.y_mm);
    assert!(apart >= 0.6, "{apart}");
    // On the rim the region gained, off the pillar of 111..119, 51..59 in
    // pixels of 0.1 mm.
    let pixels = &report.growths[0].pixels;
    for tip in &tips {
        assert!(pixels.layers[0].runs.iter().any(|run| {
            let row_mm = (f64::from(run.row) + 0.5).mul_add(pixels.step_mm[1], pixels.origin_mm[1]);
            (row_mm - tip.at.y_mm).abs() < pixels.step_mm[1].abs()
        }));
    }
}

/// A region where no contact ever fits gets no tip, and says so.
#[test]
fn a_growth_no_contact_fits_on_is_left_with_no_room() {
    let report = body(10)
        .model(4..6, Shape::rect(111.0, 51.0, 119.0, 59.0))
        .model(6..10, Shape::rect(100.0, 40.0, 130.0, 70.0))
        .tip(115.0, 55.0, 4, 3.0)
        .build()
        .report();
    let plan = plan_all_tips(&[], &report.growths, &TipPlanOptions::for_tip(5.0));
    assert!(plan.growth.is_empty());
    assert_eq!(plan.no_room, [0]);
}

fn islands(builder: FixtureBuilder) -> Vec<Island> {
    builder.build().report().islands
}

fn plan_for(builder: FixtureBuilder, diameter_mm: f64) -> TipPlan {
    plan_tips(&islands(builder), &TipPlanOptions::for_tip(diameter_mm))
}

#[test]
fn an_island_as_wide_as_a_tip_gets_one_under_its_first_pixels() {
    // 3 mm wide, on layer 4.
    let builder = body(10).model(4..10, Shape::rect(100.0, 40.0, 130.0, 70.0));
    let found = islands(builder);
    let plan = plan_tips(&found, &TipPlanOptions::for_tip(0.3));
    assert_eq!(plan.tips.len(), 1);
    let tip = &plan.tips[0];
    assert_eq!(tip.layer, 4);
    assert!((tip.at.z_mm - 0.25).abs() < 1e-9, "{:?}", tip.at);
    // The middle of the block: it stands under its first pixels.
    assert!((tip.at.x_mm - found[0].at.x_mm).abs() < 0.2, "{:?}", tip.at);
    assert!((tip.at.y_mm - found[0].at.y_mm).abs() < 0.2, "{:?}", tip.at);
}

#[test]
fn a_smaller_island_waits_until_it_has_grown_to_the_tip() {
    // 0.2 mm wide for two layers, then 1 mm: a 0.5 mm tip fits on layer 6.
    let builder = body(10)
        .model(4..6, Shape::rect(100.0, 40.0, 102.0, 42.0))
        .model(6..10, Shape::rect(96.0, 36.0, 106.0, 46.0));
    let plan = plan_for(builder, 0.5);
    assert_eq!(plan.tips[0].layer, 6);
    assert!(plan.too_narrow.is_empty());
}

#[test]
fn an_island_that_never_reaches_the_tip_gets_nothing() {
    let builder = body(10).model(4..10, Shape::rect(100.0, 40.0, 102.0, 42.0));
    let plan = plan_for(builder, 0.5);
    assert!(plan.tips.is_empty());
    assert_eq!(plan.too_narrow, [0]);
}

#[test]
fn a_tip_goes_right_above_the_underside_of_the_pixel_it_stands_on() {
    // The first pixels are too small; the spot picked was lit two layers
    // lower than the one it was found on.
    let builder = body(12)
        .model(4..6, Shape::rect(100.0, 40.0, 110.0, 42.0))
        .model(6..12, Shape::rect(100.0, 40.0, 110.0, 50.0));
    let plan = plan_for(builder, 0.5);
    let tip = &plan.tips[0];
    assert_eq!(tip.layer, 6);
    assert!((tip.at.z_mm - 0.35).abs() < 1e-9, "{:?}", tip.at);
}

#[test]
fn the_surface_gained_asks_for_a_tip_every_ten_square_millimetres() {
    // 9 mm², then 16, 25 and 36: 27 gained, without a jump.
    let builder = body(12)
        .model(4..5, Shape::rect(100.0, 40.0, 130.0, 70.0))
        .model(5..6, Shape::rect(95.0, 35.0, 135.0, 75.0))
        .model(6..7, Shape::rect(90.0, 30.0, 140.0, 80.0))
        .model(7..12, Shape::rect(85.0, 25.0, 145.0, 85.0));
    let plan = plan_for(builder, 0.3);
    let layers: Vec<usize> = plan.tips.iter().map(|tip| tip.layer).collect();
    assert_eq!(layers, [4, 6, 7]);
}

#[test]
fn a_jump_asks_for_two_tips_and_a_chained_one_for_three_more() {
    // 1 mm² → 10.2 → 51.8.
    let builder = body(12)
        .model(4..5, Shape::rect(100.0, 60.0, 110.0, 70.0))
        .model(5..6, Shape::rect(90.0, 50.0, 122.0, 82.0))
        .model(6..12, Shape::rect(70.0, 30.0, 142.0, 102.0));
    let plan = plan_for(builder, 0.3);
    // 1 first, 2 for ×10, 1 for the first 10 mm², 3 for ×5, 4 for the
    // rest of the 50 mm² gained.
    assert_eq!(plan.tips.len(), 1 + 2 + 1 + 3 + 4, "{:?}", plan.tips);
    assert_eq!(plan.tips[0].layer, 4);
    assert_eq!(plan.tips[1].layer, 5);
}

#[test]
fn what_does_not_fit_on_a_layer_goes_higher() {
    // 0.25 mm² jumps to 2.56 (1.6 mm a side): with a 0.8 mm tip, 1.6 mm from
    // the others, one fits there; the wider top takes the rest.
    let builder = body(12)
        .model(4..5, Shape::rect(100.0, 60.0, 105.0, 65.0))
        .model(5..7, Shape::rect(97.0, 57.0, 113.0, 73.0))
        .model(7..12, Shape::rect(80.0, 40.0, 130.0, 90.0));
    let plan = plan_for(builder, 0.8);
    let layers: Vec<usize> = plan.tips.iter().map(|tip| tip.layer).collect();
    assert_eq!(layers[0], 5, "{:?}", plan.tips);
    assert!(layers.len() >= 3, "{:?}", plan.tips);
    assert!(layers[1..].iter().all(|layer| *layer == 7), "{layers:?}");
}

#[test]
fn what_never_fits_is_not_placed() {
    let builder = body(12)
        .model(4..5, Shape::rect(100.0, 60.0, 104.0, 64.0))
        .model(5..12, Shape::rect(99.0, 59.0, 112.0, 72.0));
    let plan = plan_for(builder, 0.6);
    assert_eq!(plan.tips.len(), 1, "{:?}", plan.tips);
}

#[test]
fn tips_stand_apart() {
    let builder = body(12)
        .model(4..5, Shape::rect(100.0, 60.0, 110.0, 70.0))
        .model(5..12, Shape::rect(70.0, 30.0, 141.0, 101.0));
    let plan = plan_for(builder, 0.3);
    let spacing = SPACING_DIAMETERS * 0.3;
    for (i, a) in plan.tips.iter().enumerate() {
        for b in &plan.tips[..i] {
            let d = (a.at.x_mm - b.at.x_mm).hypot(a.at.y_mm - b.at.y_mm);
            assert!(d >= spacing - 1e-9, "{a:?} {b:?}");
        }
    }
}

#[test]
fn the_tips_an_island_has_count_for_its_growth_not_for_its_start() {
    // A tip stands in the island, higher than its first pixels: it pays
    // for one of the two a jump asks for, and the first tip still goes in.
    let shape = || {
        body(12)
            .model(4..5, Shape::rect(100.0, 60.0, 110.0, 70.0))
            .model(5..12, Shape::rect(90.0, 50.0, 122.0, 82.0))
    };
    let bare = plan_for(shape(), 0.3);
    assert_eq!(bare.tips.len(), 1 + 2, "{:?}", bare.tips);
    let found = islands(shape().tip(120.0, 80.0, 5, 3.0));
    assert_eq!(found[0].tips_mm.len(), 1, "{:?}", found[0].tips_mm);
    let with_tip = plan_tips(&found, &TipPlanOptions::for_tip(0.3));
    assert_eq!(with_tip.tips.len(), 1 + 1, "{:?}", with_tip.tips);
    assert_eq!(with_tip.tips[0].layer, 4);
}

#[test]
fn a_mirrored_screen_keeps_the_tip_under_the_island() {
    use sc_print_primitives::Mirror;
    for (x, y) in [(true, false), (false, true), (true, true)] {
        let found = islands(
            body(10)
                .mirror(Mirror { x, y })
                .model(4..10, Shape::rect(100.0, 40.0, 130.0, 70.0)),
        );
        let plan = plan_tips(&found, &TipPlanOptions::for_tip(0.3));
        let tip = &plan.tips[0];
        assert!(
            (tip.at.x_mm - found[0].at.x_mm).abs() < 0.2,
            "{x} {y} {tip:?}"
        );
        assert!(
            (tip.at.y_mm - found[0].at.y_mm).abs() < 0.2,
            "{x} {y} {tip:?}"
        );
    }
}

#[test]
fn a_huge_island_is_sampled_and_still_planned() {
    let builder = FixtureBuilder::new(8).model(4..8, Shape::rect(10.0, 10.0, 190.0, 110.0));
    let plan = plan_for(builder, 0.3);
    assert_eq!(plan.tips.len(), 1);
}

#[test]
fn a_first_tip_comes_with_the_lowest_pixels_as_others() {
    let builder = body(10).model(4..10, Shape::rect(100.0, 40.0, 130.0, 70.0));
    let found = islands(builder);
    let plan = plan_tips(&found, &TipPlanOptions::for_tip(0.3));
    let tip = &plan.tips[0];
    assert!(!tip.others.is_empty());
    assert!(tip.others.iter().all(|other| *other != tip.at));
    assert!(
        (tip.others[0].z_mm - 0.25).abs() < 1e-9,
        "{:?}",
        tip.others[0]
    );
    let from = |p: &PlatePoint| (p.x_mm - tip.at.x_mm).hypot(p.y_mm - tip.at.y_mm);
    let distances: Vec<f64> = tip.others.iter().take(5).map(from).collect();
    assert!(distances.is_sorted(), "{distances:?}");
}

#[test]
fn erosion_keeps_the_centres_a_disc_fits_around() {
    let runs: Vec<Run> = (0..10).map(|row| Run::new(row, 0, 10)).collect();
    let room = erode_ellipse(&runs, 2.0, 2.0);
    assert_eq!(room.len(), 6);
    assert!(room.iter().all(|run| run.x0 == 2 && run.x1 == 8));
    assert!(erode_ellipse(&runs, 6.0, 6.0).is_empty());
    assert_eq!(erode_ellipse(&runs, 0.0, 0.0), runs);
}

#[test]
fn erosion_sees_a_gap_between_two_runs_of_a_row() {
    let mut runs = Vec::new();
    for row in 0..9 {
        runs.push(Run::new(row, 0, 4));
        runs.push(Run::new(row, 5, 9));
    }
    let room = erode_ellipse(&runs, 2.0, 1.0);
    assert!(room.iter().all(|run| run.x0 != 4 && run.x1 != 5));
}
