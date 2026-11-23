/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Peel-load tests (#240).

use super::*;
use crate::regions::SweepOptions;
use crate::test_support::{Fixture, FixtureBuilder, Shape};

fn loads_of(fixture: &Fixture) -> PlateLoads {
    let input = fixture.input();
    let regions = PlateRegions::compute(&input, &SweepOptions::default());
    plate_loads(
        &ReviewContext::new(&input, &regions),
        &LoadOptions::default(),
    )
}

fn near(found: f64, expected: f64) {
    assert!(
        (found - expected).abs() < 1e-9,
        "found {found}, expected {expected}"
    );
}

/// The pull of `mm2` of plain model, N.
fn pull(mm2: f64) -> f64 {
    PeelModel::default().region_force_n(mm2, None, 0.0).mid
}

#[test]
fn a_block_pulls_with_its_area() {
    // 40 x 30 pixels of 0.1 mm: 12 mm².
    let fixture = FixtureBuilder::new(6)
        .model(0..6, Shape::rect(20.0, 20.0, 60.0, 50.0))
        .build();
    let loads = loads_of(&fixture);
    assert_eq!(loads.model_force_n.len(), 6);
    for layer in 0..6 {
        near(loads.model_force_n[layer], pull(12.0));
        near(loads.cup_mm2[layer], 0.0);
    }
}

#[test]
fn layers_add_up_their_parts() {
    let fixture = FixtureBuilder::new(4)
        .model(0..4, Shape::rect(20.0, 20.0, 60.0, 50.0))
        .model(2..4, Shape::rect(120.0, 20.0, 140.0, 40.0))
        .build();
    let loads = loads_of(&fixture);
    near(loads.model_force_n[0], pull(12.0));
    near(loads.model_force_n[3], pull(12.0) + pull(4.0));
}

#[test]
fn a_cup_closed_toward_the_plate_sucks_with_its_section() {
    // A floor, then walls: the 50 x 40 pixel hole inside is sealed below.
    let fixture = FixtureBuilder::new(8)
        .model(0..2, Shape::rect(40.0, 30.0, 100.0, 80.0))
        .model(2..8, Shape::frame(40.0, 30.0, 100.0, 80.0, 5.0))
        .build();
    let loads = loads_of(&fixture);
    near(loads.cup_mm2[1], 0.0);
    let model = PeelModel::default();
    for layer in 2..8 {
        near(loads.cup_mm2[layer], 20.0);
        // The walls pull, and the cup with them.
        near(
            loads.model_force_n[layer],
            model.region_force_n(30.0 - 20.0, None, 20.0).mid,
        );
        let input = fixture.input();
        let regions = PlateRegions::compute(&input, &SweepOptions::default());
        let ctx = ReviewContext::new(&input, &regions);
        near(
            loads.region_force_n(&ctx, &model, layer, 0),
            loads.model_force_n[layer],
        );
    }
}

#[test]
fn a_tube_open_toward_the_plate_does_not_suck() {
    // Walls that start on the plate over open air: their air drains below.
    let fixture = FixtureBuilder::new(6)
        .model(0..6, Shape::rect(140.0, 20.0, 160.0, 40.0))
        .model(2..6, Shape::frame(40.0, 30.0, 100.0, 80.0, 5.0))
        .build();
    let loads = loads_of(&fixture);
    for layer in 0..6 {
        near(loads.cup_mm2[layer], 0.0);
    }
}

#[test]
fn a_cavity_on_the_plate_is_sealed_by_it() {
    let fixture = FixtureBuilder::new(4)
        .model(0..4, Shape::frame(40.0, 30.0, 100.0, 80.0, 5.0))
        .build();
    let loads = loads_of(&fixture);
    for layer in 0..4 {
        near(loads.cup_mm2[layer], 20.0);
    }
}

#[test]
fn moving_the_plate_changes_nothing() {
    let at = |dx: f64, dy: f64| {
        FixtureBuilder::new(5)
            .model(
                0..2,
                Shape::rect(40.0 + dx, 30.0 + dy, 100.0 + dx, 80.0 + dy),
            )
            .model(
                2..5,
                Shape::frame(40.0 + dx, 30.0 + dy, 100.0 + dx, 80.0 + dy, 5.0),
            )
            .build()
    };
    let (here, there) = (loads_of(&at(0.0, 0.0)), loads_of(&at(37.0, 21.0)));
    assert_eq!(here.cup_mm2, there.cup_mm2);
    assert_eq!(here.model_force_n, there.model_force_n);
    assert_eq!(here.region_cup_mm2, there.region_cup_mm2);
    assert_eq!(here.cups.len(), there.cups.len());
    for (a, b) in here.cups.iter().zip(&there.cups) {
        assert_eq!(
            (a.layer, a.cup_mm2, a.on_tips),
            (b.layer, b.cup_mm2, b.on_tips)
        );
        assert!((b.at_mm[0] - a.at_mm[0] - 3.7).abs() < 1e-9);
    }
}

// Bodies and tips (#241).

/// The loads without a sideways share, so a share is the bolt group's.
fn straight_loads_of(fixture: &Fixture) -> PlateLoads {
    let input = fixture.input();
    let regions = PlateRegions::compute(&input, &SweepOptions::default());
    let options = LoadOptions {
        model: PeelModel {
            lateral_share: 0.0,
            ..PeelModel::default()
        },
        ..LoadOptions::default()
    };
    plate_loads(&ReviewContext::new(&input, &regions), &options)
}

/// An 8 x 8 mm slab from layer 5 up: 64 mm².
fn slab() -> FixtureBuilder {
    FixtureBuilder::new(10).model(5..10, Shape::rect(40.0, 20.0, 120.0, 100.0))
}

#[test]
fn four_tips_share_a_centred_pull() {
    let fixture = slab()
        .tip(50.0, 30.0, 5, 2.0)
        .tip(110.0, 30.0, 5, 2.0)
        .tip(50.0, 90.0, 5, 2.0)
        .tip(110.0, 90.0, 5, 2.0)
        .build();
    let loads = straight_loads_of(&fixture);
    assert_eq!(loads.tips.len(), 4);
    for tip in &loads.tips {
        assert_eq!(tip.layer, 5);
        assert!((tip.worst_n - pull(64.0) / 4.0).abs() < 1e-6, "{tip:?}");
    }
    assert_eq!(loads.bodies.len(), 1);
    assert_eq!(loads.bodies[0].tips, 4);
    assert_eq!(loads.bodies[0].starts, vec![(5, 0)]);
}

#[test]
fn tips_off_centre_follow_the_lever() {
    // Two tips 2 mm apart, the pull centred 2 mm right of their middle.
    let fixture = slab()
        .tip(50.0, 60.0, 5, 2.0)
        .tip(70.0, 60.0, 5, 2.0)
        .build();
    let loads = straight_loads_of(&fixture);
    let force = pull(64.0);
    let near_tip = loads.tips.iter().map(|tip| tip.worst_n).fold(0.0, f64::max);
    assert!((near_tip - 1.5 * force).abs() < 1e-6, "{near_tip}");
    let peak = &loads.bodies[0];
    assert!((peak.worst_n.mid - 1.5 * force).abs() < 1e-6);
    assert!(peak.utilization > 1.0);
}

#[test]
fn the_plate_takes_the_pull_of_what_stands_on_it() {
    let fixture = FixtureBuilder::new(10)
        .model(0..10, Shape::rect(40.0, 20.0, 120.0, 100.0))
        .tip(80.0, 60.0, 5, 2.0)
        .build();
    let loads = loads_of(&fixture);
    assert_eq!(loads.tips.len(), 1);
    assert_eq!(loads.tips[0].worst_n, 0.0);
    assert!(loads.bodies.is_empty());
}

#[test]
fn a_part_that_joins_the_plate_unloads_its_tips() {
    // The slab hangs from one tip until a bridge joins it to a column on
    // the plate, and then grows: none of that reaches the tip.
    let fixture = FixtureBuilder::new(14)
        .model(0..14, Shape::rect(150.0, 40.0, 170.0, 60.0))
        .model(5..8, Shape::rect(40.0, 20.0, 120.0, 100.0))
        .model(8..14, Shape::rect(40.0, 20.0, 160.0, 100.0))
        .tip(80.0, 60.0, 5, 2.0)
        .build();
    let loads = straight_loads_of(&fixture);
    assert_eq!(loads.bodies.len(), 1);
    assert!(loads.bodies[0].layer < 8, "{:?}", loads.bodies[0]);
    assert!(loads.tips[0].worst_layer < 8);
    assert!((loads.tips[0].worst_n - pull(64.0)).abs() < 1e-6);
}

#[test]
fn a_sideways_share_loads_the_tips_more() {
    let build = || {
        slab()
            .tip(50.0, 30.0, 5, 2.0)
            .tip(110.0, 30.0, 5, 2.0)
            .tip(50.0, 90.0, 5, 2.0)
            .tip(110.0, 90.0, 5, 2.0)
            .build()
    };
    let straight = straight_loads_of(&build());
    let sideways = loads_of(&build());
    for (a, b) in straight.tips.iter().zip(&sideways.tips) {
        assert!(b.worst_n > a.worst_n);
        // Taller is worse: the worst layer is the top one.
        assert_eq!(b.worst_layer, 9);
    }
}

#[test]
fn moving_the_plate_moves_the_tips_alone() {
    let at = |dx: f64, dy: f64| {
        slab_at(dx, dy)
            .tip(50.0 + dx, 60.0 + dy, 5, 2.0)
            .tip(70.0 + dx, 70.0 + dy, 5, 2.0)
            .tip(100.0 + dx, 40.0 + dy, 5, 2.0)
            .build()
    };
    let (here, there) = (loads_of(&at(0.0, 0.0)), loads_of(&at(30.0, 10.0)));
    for (a, b) in here.tips.iter().zip(&there.tips) {
        assert!((a.worst_n - b.worst_n).abs() < 1e-9);
        assert_eq!(a.worst_layer, b.worst_layer);
    }
}

fn slab_at(dx: f64, dy: f64) -> FixtureBuilder {
    FixtureBuilder::new(10).model(
        5..10,
        Shape::rect(40.0 + dx, 20.0 + dy, 120.0 + dx, 100.0 + dy),
    )
}

// Along the print (#245).

#[test]
fn each_layer_keeps_its_most_loaded_tip_and_each_tip_its_curve() {
    let fixture = slab()
        .tip(50.0, 60.0, 5, 2.0)
        .tip(70.0, 60.0, 5, 2.0)
        .build();
    let loads = straight_loads_of(&fixture);
    assert_eq!(loads.layer_utilization.len(), 10);
    assert_eq!(loads.curve_layers, 1);
    // Nothing peels before the slab starts.
    assert!(loads.layer_utilization[..5].iter().all(|&u| u == 0.0));
    let peak = &loads.bodies[0];
    let top = loads.layer_utilization.iter().copied().fold(0.0, f64::max);
    assert!((top - peak.utilization).abs() < 1e-9);
    assert!((loads.layer_utilization[peak.layer] - peak.utilization).abs() < 1e-9);
    for (index, tip) in loads.tips.iter().enumerate() {
        let curve = &loads.tip_curves[index];
        assert_eq!(curve.len(), 10);
        let worst = curve.iter().copied().fold(0.0f32, f32::max);
        assert!(
            (f64::from(worst) - tip.worst_n).abs() < 1e-5,
            "{worst} {tip:?}"
        );
        assert!((loads.tip_load_at(index, tip.worst_layer) - tip.worst_n).abs() < 1e-5);
        assert_eq!(loads.tip_load_at(index, 2), 0.0);
    }
}

#[test]
fn a_tip_the_plate_relieves_has_no_curve() {
    let fixture = FixtureBuilder::new(10)
        .model(0..10, Shape::rect(40.0, 20.0, 120.0, 100.0))
        .tip(80.0, 60.0, 5, 2.0)
        .build();
    let loads = loads_of(&fixture);
    assert!(loads.tip_curves[0].is_empty());
    assert!(loads.layer_utilization.iter().all(|&u| u == 0.0));
}

// Critical layers as frame cases (#244).

#[test]
fn a_critical_layer_records_what_holds_each_body() {
    let fixture = slab()
        .tip(50.0, 30.0, 5, 2.0)
        .tip(110.0, 30.0, 5, 2.0)
        .tip(50.0, 90.0, 5, 2.0)
        .tip(110.0, 90.0, 5, 2.0)
        .build();
    let loads = straight_loads_of(&fixture);
    let peak = loads.bodies[0].layer;
    let case = loads
        .cases
        .iter()
        .find(|case| case.layer == peak)
        .expect("the body's worst layer is a case");
    assert!(loads.cases.len() <= CASE_LAYERS);
    assert_eq!(case.bodies.len(), 1);
    let body = &case.bodies[0];
    assert!(!body.held);
    let mut tips = body.tips.clone();
    tips.sort_unstable();
    assert_eq!(tips, vec![0, 1, 2, 3]);
    near(body.force_n.mid, pull(64.0));
    assert!((case.z_mm - fixture.input().layers()[peak].top_mm).abs() < 1e-12);
}

#[test]
fn a_body_the_plate_comes_to_hold_is_held_in_later_cases() {
    let fixture = FixtureBuilder::new(14)
        .model(0..14, Shape::rect(150.0, 40.0, 170.0, 60.0))
        .model(5..8, Shape::rect(40.0, 20.0, 120.0, 100.0))
        .model(8..14, Shape::rect(40.0, 20.0, 160.0, 100.0))
        .tip(80.0, 60.0, 5, 2.0)
        .build();
    let loads = straight_loads_of(&fixture);
    assert!(!loads.cases.is_empty());
    for case in &loads.cases {
        let body = case
            .bodies
            .iter()
            .find(|b| b.tips == vec![0])
            .expect("the tip's body");
        assert_eq!(body.held, case.layer >= 8, "layer {}", case.layer);
    }
}

#[test]
fn feet_join_the_body_they_stand_on() {
    let fixture = slab()
        .tip(50.0, 30.0, 5, 2.0)
        .tip(110.0, 90.0, 5, 2.0)
        .build();
    let foot = {
        let input = fixture.input();
        let top = input.layers()[7].top_mm;
        let [x, y] = input.pixel_to_plate([80.0, 60.0]);
        [x, y, top]
    };
    let input = fixture.input().with_feet(vec![foot]);
    let regions = PlateRegions::compute(&input, &SweepOptions::default());
    let loads = plate_loads(
        &ReviewContext::new(&input, &regions),
        &LoadOptions::default(),
    );
    let last = loads.cases.iter().max_by_key(|c| c.layer).expect("a case");
    assert!(last.layer >= 7);
    assert_eq!(last.bodies.len(), 1);
    assert_eq!(last.bodies[0].feet, vec![foot]);
}
