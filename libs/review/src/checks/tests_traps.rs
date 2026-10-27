/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Resin-trap and suction-cup tests (#83).
//!
//! The default fake printer has 0.1 mm pixels (0.01 mm² each) and 0.05 mm
//! layers, so one air pixel on one layer is 0.0005 mm³. Most plates below
//! use the same box: a 60 x 50 pixel block whose walls, 5 pixels thick,
//! surround a 50 x 40 pixel hole (2000 pixels, 20 mm²).

use sc_print_primitives::Mirror;

use super::*;
use crate::input::Role;
use crate::test_support::{Fixture, FixtureBuilder, Shape};

/// The outside of the standard box.
fn block() -> Shape {
    Shape::rect(40.0, 30.0, 100.0, 80.0)
}

/// The walls of the standard box, around its 50 x 40 pixel hole.
fn walls() -> Shape {
    Shape::frame(40.0, 30.0, 100.0, 80.0, 5.0)
}

/// The corners of a rectangle of pixels, for [`Shape::polygon`].
fn ring(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<[f64; 2]> {
    vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
}

/// The standard block pierced by the hole `x0..x1` by `y0..y1`.
fn block_with_hole(x0: f64, y0: f64, x1: f64, y1: f64) -> Shape {
    Shape::polygon(ring(40.0, 30.0, 100.0, 80.0), vec![ring(x0, y0, x1, y1)])
}

/// Four support pillars under the corners of the standard box.
fn corner_pillars(builder: FixtureBuilder, layers: std::ops::Range<usize>) -> FixtureBuilder {
    builder
        .support(layers.clone(), Shape::rect(40.0, 30.0, 44.0, 34.0))
        .support(layers.clone(), Shape::rect(96.0, 30.0, 100.0, 34.0))
        .support(layers.clone(), Shape::rect(40.0, 76.0, 44.0, 80.0))
        .support(layers, Shape::rect(96.0, 76.0, 100.0, 80.0))
}

fn traps(fixture: &Fixture) -> Vec<Finding> {
    fixture.check(|ctx| run(ctx, &TrapOptions::default()))
}

fn traps_with(fixture: &Fixture, options: &TrapOptions) -> Vec<Finding> {
    fixture.check(|ctx| run(ctx, options))
}

/// The inside point of the first enclosed air region of `layer`: where a
/// finding on a one-region cavity aims.
fn hole_point(fixture: &Fixture, layer: usize) -> [f64; 2] {
    let regions = fixture.regions();
    regions
        .layer(layer)
        .air
        .iter()
        .find(|region| !region.touches_border)
        .expect("an enclosed air region")
        .inside_point()
}

fn assert_close(found: f64, expected: f64) {
    assert!((found - expected).abs() < 1e-9, "{found} != {expected}");
}

fn drain_at(finding: &Finding) -> PlatePoint {
    match finding.fix {
        Some(SuggestedFix::AddDrain { at }) => at,
        other => panic!("expected a drain, got {other:?}"),
    }
}

// ---- Clean shapes stay quiet ------------------------------------------------

#[test]
fn a_solid_block_has_no_trap() {
    // 40 x 30 pixels (4 x 3 mm) on every layer of an 8-layer plate.
    let fixture = FixtureBuilder::new(8)
        .model(.., Shape::rect(40.0, 30.0, 80.0, 60.0))
        .build();
    let findings = fixture.check(|ctx| run(ctx, &TrapOptions::default()));
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn an_empty_plate_has_no_trap() {
    let fixture = FixtureBuilder::new(4).build();
    assert!(traps(&fixture).is_empty());
}

#[test]
fn an_open_tube_on_supports_is_quiet() {
    // Open at the bottom, between the pillars, and at the top.
    let fixture = corner_pillars(FixtureBuilder::new(12), 0..4)
        .model(4..12, walls())
        .build();
    assert!(traps(&fixture).is_empty(), "{:?}", traps(&fixture));
}

#[test]
fn a_dome_on_supports_is_not_a_trap() {
    // A box with a lid and no floor, held by pillars: its cavity opens
    // toward the plate, into the open air between the pillars.
    let fixture = corner_pillars(FixtureBuilder::new(14), 0..5)
        .model(5..10, walls())
        .model(10..12, block())
        .build();
    assert!(traps(&fixture).is_empty(), "{:?}", traps(&fixture));
}

#[test]
fn a_drain_in_the_floor_drains_the_cavity() {
    // A 4 x 4 pixel drain (16 pixels) through the floor of a closed box held
    // by pillars.
    let fixture = corner_pillars(FixtureBuilder::new(18), 0..4)
        .model(4..6, block_with_hole(68.0, 53.0, 72.0, 57.0))
        .model(6..14, walls())
        .model(14..16, block())
        .build();
    assert!(traps(&fixture).is_empty(), "{:?}", traps(&fixture));
}

#[test]
fn holes_between_support_trunks_are_open_air() {
    // Four trunks braced by a ring on layer 4, holding a block. Inside the
    // brace the air is enclosed within its layer, but it sits on the open
    // air between the trunks.
    let mut builder = FixtureBuilder::new(12);
    for (x, y) in [(128.0, 60.0), (72.0, 60.0), (100.0, 88.0), (100.0, 32.0)] {
        builder = builder.support(0..8, Shape::circle(x, y, 4.0));
    }
    let fixture = builder
        .support(4..5, Shape::ring(100.0, 60.0, 30.0, 26.0))
        .model(8..12, Shape::rect(65.0, 25.0, 135.0, 95.0))
        .build();
    assert!(
        fixture
            .regions()
            .layer(4)
            .air
            .iter()
            .any(|r| !r.touches_border),
        "the brace encloses air"
    );
    assert!(traps(&fixture).is_empty(), "{:?}", traps(&fixture));
}

#[test]
fn a_hole_in_a_support_raft_is_open_air_at_plate_level() {
    // A raft with a hole cut around a block that stands on the plate.
    let fixture = FixtureBuilder::new(10)
        .support(0..3, Shape::frame(20.0, 10.0, 180.0, 110.0, 30.0))
        .model(.., Shape::rect(80.0, 50.0, 120.0, 70.0))
        .build();
    assert!(
        fixture
            .regions()
            .layer(0)
            .air
            .iter()
            .any(|r| !r.touches_border),
        "the raft encloses air"
    );
    assert!(traps(&fixture).is_empty(), "{:?}", traps(&fixture));
}

#[test]
fn cavities_too_small_to_matter_are_not_reported() {
    let left = || Shape::rect(10.0, 10.0, 30.0, 30.0);
    let middle = || Shape::rect(50.0, 10.0, 70.0, 30.0);
    let right = || Shape::rect(90.0, 10.0, 110.0, 30.0);
    let pierced = |x0: f64, y0: f64, x1: f64, y1: f64, side: f64| {
        Shape::polygon(
            ring(side, 10.0, side + 20.0, 30.0),
            vec![ring(x0, y0, x1, y1)],
        )
    };
    let fixture = FixtureBuilder::new(12)
        // A 3 x 3 pixel hairline void sealed over 6 layers.
        .model(0..2, left())
        .model(2..8, pierced(18.0, 18.0, 21.0, 21.0, 10.0))
        .model(8..12, left())
        // A 4 x 4 pixel void on one layer: 0.008 mm³.
        .model(0..2, middle())
        .model(2..3, pierced(58.0, 18.0, 62.0, 22.0, 50.0))
        .model(3..12, middle())
        // A 6 x 6 pixel cup open on the last layer: 0.36 mm².
        .model(0..6, right())
        .model(6..12, pierced(97.0, 17.0, 103.0, 23.0, 90.0))
        .build();
    assert!(traps(&fixture).is_empty(), "{:?}", traps(&fixture));

    // Without the size limits the void and the cup show; the hairline,
    // narrower than a drain on every layer, never does.
    let everything = TrapOptions {
        min_trap_volume_mm3: 0.0,
        min_cup_area_mm2: 0.0,
        ..TrapOptions::default()
    };
    let findings = traps_with(&fixture, &everything);
    let found: Vec<(CheckKind, usize, usize)> = findings
        .iter()
        .map(|f| (f.check, f.first_layer, f.last_layer))
        .collect();
    assert_eq!(
        found,
        vec![(CheckKind::ResinTrap, 2, 2), (CheckKind::SuctionCup, 6, 11)]
    );
}

// ---- Resin traps ------------------------------------------------------------

#[test]
fn a_closed_hollow_box_is_a_resin_trap() {
    // Floor on layers 0-2, walls on 3-12, lid on 13-15.
    let fixture = FixtureBuilder::new(16)
        .model(0..3, block())
        .model(3..13, walls())
        .model(13..16, block())
        .build();
    let findings = traps(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    let trap = &findings[0];
    assert_eq!(trap.check, CheckKind::ResinTrap);
    assert_eq!(trap.severity, Severity::Error);
    assert_eq!((trap.first_layer, trap.last_layer), (3, 12));
    assert_eq!(trap.scope, RoleScope::Model);
    assert_eq!(trap.title, "Sealed cavity traps resin");
    assert_eq!(
        trap.detail,
        "A sealed cavity of 10.0 mm³ over 10 layers (0.50 mm), up to 20.0 mm² wide, has no opening of 10 pixels or more, so its liquid resin stays inside."
    );

    // Aimed at the floor: the hole's inside point on layer 3; the drain at
    // the bottom of that layer.
    let [x, y] = fixture.pixel_to_plate(hole_point(&fixture, 3));
    assert_close(trap.focus.x_mm, x);
    assert_close(trap.focus.y_mm, y);
    assert_close(trap.focus.z_mm, 3.5 * 0.05);
    let drain = drain_at(trap);
    assert_close(drain.x_mm, x);
    assert_close(drain.y_mm, y);
    assert_close(drain.z_mm, 3.0 * 0.05);

    // The whole review reports it too.
    assert!(fixture.review().contains(trap));
}

#[test]
fn a_small_trap_is_a_warning() {
    // A 10 x 10 pixel cavity over 4 layers: 0.2 mm³.
    let outside = || Shape::rect(40.0, 30.0, 60.0, 50.0);
    let fixture = FixtureBuilder::new(8)
        .model(0..2, outside())
        .model(2..6, Shape::frame(40.0, 30.0, 60.0, 50.0, 5.0))
        .model(6..8, outside())
        .build();
    let findings = traps(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].check, CheckKind::ResinTrap);
    assert_eq!(findings[0].severity, Severity::Warning);
    assert!(findings[0].detail.contains("0.200 mm³"), "{findings:?}");

    let strict = TrapOptions {
        error_volume_mm3: 0.1,
        ..TrapOptions::default()
    };
    assert_eq!(traps_with(&fixture, &strict)[0].severity, Severity::Error);
}

#[test]
fn a_pinhole_drain_leaves_a_trap() {
    // The same box as `a_drain_in_the_floor_drains_the_cavity`, with a
    // 3 x 3 pixel drain: 9 pixels, under the 10-pixel threshold.
    let fixture = corner_pillars(FixtureBuilder::new(18), 0..4)
        .model(4..6, block_with_hole(68.0, 53.0, 71.0, 56.0))
        .model(6..14, walls())
        .model(14..16, block())
        .build();
    let findings = traps(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    let trap = &findings[0];
    assert_eq!(trap.check, CheckKind::ResinTrap);
    assert_eq!((trap.first_layer, trap.last_layer), (6, 13));
    assert_close(drain_at(trap).z_mm, 6.0 * 0.05);

    // A lower threshold lets the same drain work.
    let loose = TrapOptions {
        drain_px: 9,
        ..TrapOptions::default()
    };
    assert!(traps_with(&fixture, &loose).is_empty());
}

#[test]
fn a_pinhole_vent_leaves_a_trap() {
    // A lid with a 3 x 3 pixel vent to the open resin above.
    let fixture = FixtureBuilder::new(16)
        .model(0..3, block())
        .model(3..13, walls())
        .model(13..15, block_with_hole(69.0, 54.0, 72.0, 57.0))
        .build();
    let findings = traps(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].check, CheckKind::ResinTrap);
    assert_eq!((findings[0].first_layer, findings[0].last_layer), (3, 12));
}

#[test]
fn chambers_joined_above_are_one_cavity() {
    // Two 40 x 40 pixel chambers on layers 2-7 meet in one 90 x 40 pixel
    // chamber on layers 8-9.
    let outside = || Shape::rect(40.0, 30.0, 140.0, 80.0);
    let fixture = FixtureBuilder::new(14)
        .model(0..2, outside())
        .model(
            2..8,
            Shape::polygon(
                ring(40.0, 30.0, 140.0, 80.0),
                vec![ring(45.0, 35.0, 85.0, 75.0), ring(95.0, 35.0, 135.0, 75.0)],
            ),
        )
        .model(8..10, Shape::frame(40.0, 30.0, 140.0, 80.0, 5.0))
        .model(10..12, outside())
        .build();
    let findings = traps(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!((findings[0].first_layer, findings[0].last_layer), (2, 9));
    // (2 x 1600 x 6 + 3600 x 2) pixel layers of 0.0005 mm³.
    assert!(findings[0].detail.contains("13.2 mm³"), "{findings:?}");
}

#[test]
fn separate_cavities_are_separate_findings() {
    let box_at = |builder: FixtureBuilder, x0: f64, floor: usize, lid: usize| {
        builder
            .model(0..floor, Shape::rect(x0, 20.0, x0 + 40.0, 60.0))
            .model(floor..lid, Shape::frame(x0, 20.0, x0 + 40.0, 60.0, 5.0))
            .model(lid..lid + 2, Shape::rect(x0, 20.0, x0 + 40.0, 60.0))
    };
    let fixture = box_at(box_at(FixtureBuilder::new(12), 20.0, 2, 6), 120.0, 4, 9).build();
    let findings = traps(&fixture);
    let found: Vec<(CheckKind, usize, usize)> = findings
        .iter()
        .map(|f| (f.check, f.first_layer, f.last_layer))
        .collect();
    assert_eq!(
        found,
        vec![(CheckKind::ResinTrap, 2, 5), (CheckKind::ResinTrap, 4, 8)]
    );
}

#[test]
fn a_trap_walled_by_supports_has_support_scope_and_no_drain() {
    let fixture = FixtureBuilder::new(12)
        .support(0..3, block())
        .support(3..8, walls())
        .support(8..10, block())
        .build();
    let findings = traps(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].check, CheckKind::ResinTrap);
    assert_eq!(findings[0].scope, RoleScope::Support);
    assert_eq!(findings[0].fix, None);
}

#[test]
fn a_cavity_walled_by_model_and_support_has_both_scopes() {
    // Three model walls; a support closes the fourth side.
    let fixture = FixtureBuilder::new(10)
        .model(0..2, block())
        .model(2..7, Shape::rect(40.0, 30.0, 45.0, 80.0))
        .model(2..7, Shape::rect(45.0, 30.0, 95.0, 35.0))
        .model(2..7, Shape::rect(45.0, 75.0, 95.0, 80.0))
        .support(2..7, Shape::rect(95.0, 30.0, 100.0, 80.0))
        .model(7..9, block())
        .build();
    let findings = traps(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].scope, RoleScope::Both);
    assert!(matches!(
        findings[0].fix,
        Some(SuggestedFix::AddDrain { .. })
    ));
}

#[test]
fn volumes_follow_the_pixel_size_and_the_layer_height() {
    // 0.2 mm pixels and 0.1 mm layers: a 30 x 30 pixel cavity over 3
    // layers is 900 x 0.04 x 0.1 x 3 = 10.8 mm³.
    let fixture = FixtureBuilder::new(7)
        .screen(100, 60, 0.2)
        .layer_height_mm(0.1)
        .model(0..2, Shape::rect(20.0, 10.0, 60.0, 50.0))
        .model(2..5, Shape::frame(20.0, 10.0, 60.0, 50.0, 5.0))
        .model(5..7, Shape::rect(20.0, 10.0, 60.0, 50.0))
        .build();
    let findings = traps(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(
        findings[0]
            .detail
            .contains("10.8 mm³ over 3 layers (0.30 mm), up to 36.0 mm² wide"),
        "{findings:?}"
    );
    assert_close(drain_at(&findings[0]).z_mm, 0.2);
}

#[test]
fn the_focus_and_the_drain_follow_the_mirror() {
    let fixture = FixtureBuilder::new(16)
        .mirror(Mirror { x: true, y: true })
        .model(0..3, block())
        .model(3..13, walls())
        .model(13..16, block())
        .build();
    let findings = traps(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    let [x, y] = fixture.pixel_to_plate(hole_point(&fixture, 3));
    assert_close(findings[0].focus.x_mm, x);
    assert_close(findings[0].focus.y_mm, y);
    let drain = drain_at(&findings[0]);
    assert_close(drain.x_mm, x);
    assert_close(drain.y_mm, y);
}

// ---- Suction cups -----------------------------------------------------------

#[test]
fn a_cavity_reaching_the_last_layer_is_a_suction_cup() {
    // Nothing above the last layer is represented: it is open resin.
    let fixture = FixtureBuilder::new(10)
        .model(0..3, block())
        .model(3..10, walls())
        .build();
    let findings = traps(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    let cup = &findings[0];
    assert_eq!(cup.check, CheckKind::SuctionCup);
    assert_eq!(cup.severity, Severity::Warning);
    assert_eq!((cup.first_layer, cup.last_layer), (3, 9));
    assert_eq!(cup.title, "Cavity closed toward the plate");
    assert_eq!(
        cup.detail,
        "A cavity of 7.00 mm³ over 7 layers (0.35 mm) is closed toward the plate and opens only above, so up to 20.0 mm² of it pulls on the film at every peel."
    );
    assert_close(drain_at(cup).z_mm, 3.0 * 0.05);
}

#[test]
fn a_cavity_that_opens_above_is_a_suction_cup() {
    // Above layer 7 only the left wall goes on, so the cavity opens into
    // the open air.
    let fixture = FixtureBuilder::new(14)
        .model(0..3, block())
        .model(3..8, walls())
        .model(8..14, Shape::rect(40.0, 30.0, 45.0, 80.0))
        .build();
    let findings = traps(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].check, CheckKind::SuctionCup);
    assert_eq!((findings[0].first_layer, findings[0].last_layer), (3, 7));
}

#[test]
fn a_window_in_the_wall_ends_the_cup_below_it() {
    // On layer 7 the top wall is missing: the cavity below it is a cup that
    // opens there, and the part above drains through it.
    let fixture = FixtureBuilder::new(16)
        .model(0..3, block())
        .model(3..7, walls())
        .model(7..8, Shape::rect(40.0, 30.0, 45.0, 75.0))
        .model(7..8, Shape::rect(95.0, 30.0, 100.0, 75.0))
        .model(7..8, Shape::rect(45.0, 30.0, 95.0, 35.0))
        .model(8..13, walls())
        .model(13..16, block())
        .build();
    let findings = traps(&fixture);
    let found: Vec<(CheckKind, usize, usize)> = findings
        .iter()
        .map(|f| (f.check, f.first_layer, f.last_layer))
        .collect();
    assert_eq!(found, vec![(CheckKind::SuctionCup, 3, 6)]);
}

#[test]
fn a_vent_in_the_lid_turns_the_trap_into_a_cup() {
    // A 4 x 4 pixel vent (16 pixels) through the lid, open resin above.
    let fixture = FixtureBuilder::new(16)
        .model(0..3, block())
        .model(3..13, walls())
        .model(13..15, block_with_hole(68.0, 53.0, 72.0, 57.0))
        .build();
    let findings = traps(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].check, CheckKind::SuctionCup);
    assert_eq!((findings[0].first_layer, findings[0].last_layer), (3, 14));
}

#[test]
fn a_cavity_on_the_plate_is_a_suction_cup_without_a_drain() {
    // A recess under a block standing on the plate, closed above.
    let fixture = FixtureBuilder::new(8)
        .model(0..5, walls())
        .model(5..8, block())
        .build();
    let findings = traps(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    let cup = &findings[0];
    assert_eq!(cup.check, CheckKind::SuctionCup);
    assert_eq!((cup.first_layer, cup.last_layer), (0, 4));
    assert_eq!(cup.title, "Cavity sealed by the build plate");
    assert!(cup.detail.contains("closed by the build plate"), "{cup:?}");
    assert_eq!(cup.fix, None);
}

#[test]
fn a_recess_under_a_model_inside_a_raft_is_still_a_cup() {
    // The raft's hole is open air, but the recess under the block is walled
    // by the model.
    let fixture = FixtureBuilder::new(10)
        .support(0..3, Shape::frame(20.0, 10.0, 180.0, 110.0, 30.0))
        .model(0..6, Shape::frame(70.0, 45.0, 130.0, 75.0, 5.0))
        .model(6..9, Shape::rect(70.0, 45.0, 130.0, 75.0))
        .build();
    let findings = traps(&fixture);
    let found: Vec<(CheckKind, usize, usize, RoleScope)> = findings
        .iter()
        .map(|f| (f.check, f.first_layer, f.last_layer, f.scope))
        .collect();
    assert_eq!(found, vec![(CheckKind::SuctionCup, 0, 5, RoleScope::Model)]);
}

#[test]
fn a_single_layer_ring_on_the_plate_is_a_cup() {
    // One layer: the hole is closed by the plate and open to the resin
    // above at once.
    let fixture = FixtureBuilder::new(1)
        .shape(
            "Ring",
            Role::Model,
            ..,
            Shape::ring(100.0, 60.0, 20.0, 15.0),
        )
        .build();
    let findings = traps(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].check, CheckKind::SuctionCup);
    assert_eq!((findings[0].first_layer, findings[0].last_layer), (0, 0));
}
