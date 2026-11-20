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
    assert_eq!(loads_of(&at(0.0, 0.0)), loads_of(&at(37.0, 21.0)));
}
