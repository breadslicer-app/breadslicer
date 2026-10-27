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
use crate::regions::PlateRegions;
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

/// The options under which every trap is large and no cup is filled: most
/// tests below pin where a drain goes, whatever the cavity's size. The tiers
/// have tests of their own.
fn drains_only() -> TrapOptions {
    TrapOptions {
        info_volume_mm3: 0.0,
        error_volume_mm3: 0.0,
        ..TrapOptions::default()
    }
}

fn traps(fixture: &Fixture) -> Vec<Finding> {
    fixture.check(|ctx| run(ctx, &drains_only()))
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

/// Kind and layer range of every finding.
fn spans(findings: &[Finding]) -> Vec<(CheckKind, usize, usize)> {
    findings
        .iter()
        .map(|f| (f.check, f.first_layer, f.last_layer))
        .collect()
}

/// The walls of the standard box on `layer`, the left one cut by a gap
/// `rows` pixels tall from row 55.
fn walls_with_gap(builder: FixtureBuilder, layer: usize, rows: f64) -> FixtureBuilder {
    let on = layer..layer + 1;
    builder
        .model(on.clone(), Shape::rect(40.0, 30.0, 45.0, 55.0))
        .model(on.clone(), Shape::rect(40.0, 55.0 + rows, 45.0, 80.0))
        .model(on.clone(), Shape::rect(95.0, 30.0, 100.0, 80.0))
        .model(on.clone(), Shape::rect(45.0, 30.0, 95.0, 35.0))
        .model(on, Shape::rect(45.0, 75.0, 95.0, 80.0))
}

/// The recess under a block standing on the plate: the standard walls on
/// layers 0-4, a lid on 5-7.
fn recess() -> FixtureBuilder {
    FixtureBuilder::new(10)
        .model(0..5, walls())
        .model(5..8, block())
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
    // smaller than a drain on every layer, never does.
    let everything = TrapOptions {
        min_trap_volume_mm3: 0.0,
        min_cup_area_mm2: 0.0,
        ..drains_only()
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
        "A sealed cavity of 10.0 mm³ over 10 layers (0.50 mm), up to 20.0 mm² wide, has no opening at least 3 pixels wide and 10 pixels in area, so its liquid resin stays inside."
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

    // The whole review reports it too: 10 mm³ is a medium trap there, to
    // fill.
    let reviewed = fixture.review();
    assert_eq!(reviewed.len(), 1, "{reviewed:?}");
    assert_eq!(spans(&reviewed), spans(&findings));
    assert_eq!(reviewed[0].severity, Severity::Warning);
    assert!(matches!(
        reviewed[0].fix,
        Some(SuggestedFix::Fill { cavity: 0, .. })
    ));
}

/// A sealed `side` x `side` pixel cavity over `layers` layers, in walls 5
/// pixels thick, with two layers of floor and two of lid.
fn sealed_box(side: f64, layers: usize) -> FixtureBuilder {
    let outside = || Shape::rect(40.0, 30.0, 50.0 + side, 40.0 + side);
    FixtureBuilder::new(layers + 4)
        .model(0..2, outside())
        .model(
            2..2 + layers,
            Shape::frame(40.0, 30.0, 50.0 + side, 40.0 + side, 5.0),
        )
        .model(2 + layers..layers + 4, outside())
}

#[test]
fn a_tiny_trap_is_a_note_to_fill() {
    // A 10 x 10 pixel cavity over 4 layers: 0.2 mm³.
    let fixture = sealed_box(10.0, 4).build();
    let findings = fixture.check(|ctx| run(ctx, &TrapOptions::default()));
    assert_eq!(findings.len(), 1, "{findings:?}");
    let trap = &findings[0];
    assert_eq!(trap.check, CheckKind::ResinTrap);
    assert_eq!(trap.severity, Severity::Info);
    assert_eq!(trap.title, "Tiny sealed cavity");
    assert!(trap.detail.contains("0.200 mm³"), "{findings:?}");
    let Some(SuggestedFix::Fill { at, cavity: 0 }) = trap.fix else {
        panic!("expected the first fill, got {:?}", trap.fix);
    };
    // Aimed at the floor, like a drain would be.
    assert_close(at.z_mm, 2.0 * 0.05);
    assert_close(at.x_mm, trap.focus.x_mm);
}

#[test]
fn traps_are_tiered_by_volume() {
    // 0.2 mm³, 10 mm³ and 40 mm³ (40 x 40 pixels over 50 layers).
    let tier = |fixture: &Fixture, options: &TrapOptions| {
        let findings = traps_with(fixture, options);
        assert_eq!(findings.len(), 1, "{findings:?}");
        let fix = match findings[0].fix {
            Some(SuggestedFix::Fill { .. }) => "fill",
            Some(SuggestedFix::AddDrain { .. }) => "drain",
            _ => "none",
        };
        (findings[0].severity, findings[0].title.clone(), fix)
    };
    let defaults = TrapOptions::default();
    let tiny = sealed_box(10.0, 4).build();
    let medium = sealed_box(50.0, 8).build();
    let large = sealed_box(40.0, 50).build();
    assert_eq!(
        tier(&tiny, &defaults),
        (Severity::Info, "Tiny sealed cavity".to_owned(), "fill")
    );
    assert_eq!(
        tier(&medium, &defaults),
        (
            Severity::Warning,
            "Small sealed cavity traps resin".to_owned(),
            "fill"
        )
    );
    assert_eq!(
        tier(&large, &defaults),
        (
            Severity::Error,
            "Sealed cavity traps resin".to_owned(),
            "drain"
        )
    );

    // The thresholds are options.
    let strict = TrapOptions {
        info_volume_mm3: 0.1,
        error_volume_mm3: 5.0,
        ..TrapOptions::default()
    };
    assert_eq!(tier(&tiny, &strict).0, Severity::Warning);
    assert_eq!(tier(&medium, &strict).0, Severity::Error);
    assert_eq!(tier(&medium, &strict).2, "drain");
    assert_eq!(trap_severity(0.99, &defaults), Severity::Info);
    assert_eq!(trap_severity(1.0, &defaults), Severity::Warning);
    assert_eq!(trap_severity(26.9, &defaults), Severity::Warning);
    assert_eq!(trap_severity(27.0, &defaults), Severity::Error);
}

#[test]
fn a_fill_holds_the_pixels_of_the_cavity() {
    // 50 x 40 pixels over layers 3-12 of the standard box: 10 mm³.
    let fixture = FixtureBuilder::new(16)
        .model(0..3, block())
        .model(3..13, walls())
        .model(13..16, block())
        .build();
    let input = fixture.input();
    let regions = fixture.regions();
    let ctx = ReviewContext::new(&input, &regions);
    let (findings, fills) = run_with_fills(&ctx, &TrapOptions::default());
    assert_eq!(findings.len(), 1);
    assert_eq!(fills.len(), 1);
    let fill = &fills[0];
    assert_eq!(fill.slabs.len(), 10);
    assert_close(fill.slabs[0].bottom_mm, 3.0 * 0.05);
    assert_close(fill.slabs[9].top_mm, 13.0 * 0.05);
    assert!(
        (fill.volume_mm3() - 10.0).abs() < 1e-9,
        "{}",
        fill.volume_mm3()
    );
    // The hole is one rectangle per layer: x 45-95 px, y 35-75 px.
    let [x0, y0] = fixture.pixel_to_plate([45.0, 35.0]);
    let [x1, y1] = fixture.pixel_to_plate([95.0, 75.0]);
    let rect = fill.slabs[4].rects[0];
    assert_eq!(fill.slabs[4].rects.len(), 1);
    assert_close(rect.x0_mm, x0.min(x1));
    assert_close(rect.x1_mm, x0.max(x1));
    assert_close(rect.y0_mm, y0.min(y1));
    assert_close(rect.y1_mm, y0.max(y1));
}

#[test]
fn a_filled_cavity_is_gone_on_the_next_review() {
    // A tiny and a medium trap side by side, and a large one: fill the two
    // that offer it, as the app does on the next slice.
    let sealed = |builder: FixtureBuilder, [x0, y0, x1, y1]: [f64; 4], top: usize| {
        builder
            .model(0..2, Shape::rect(x0, y0, x1, y1))
            .model(2..top, Shape::frame(x0, y0, x1, y1, 5.0))
            .model(top..top + 2, Shape::rect(x0, y0, x1, y1))
    };
    // 0.2 mm3, 10 mm3 and 40 mm3.
    let builder = sealed(FixtureBuilder::new(56), [10.0, 10.0, 30.0, 30.0], 6);
    let builder = sealed(builder, [40.0, 10.0, 100.0, 70.0], 10);
    let fixture = sealed(builder, [110.0, 10.0, 160.0, 60.0], 52).build();
    let input = fixture.input();
    let report = crate::runner::review_with(&input, &crate::runner::ReviewOptions::default());
    let trap_tiers = |findings: &[Finding]| {
        let mut tiers: Vec<Severity> = findings
            .iter()
            .filter(|f| f.check == CheckKind::ResinTrap)
            .map(|f| f.severity)
            .collect();
        tiers.sort();
        tiers
    };
    assert_eq!(
        trap_tiers(&report.findings),
        [Severity::Error, Severity::Warning, Severity::Info],
        "{:?}",
        report.findings
    );
    let picked: Vec<&CavityFill> = report
        .findings
        .iter()
        .filter_map(|f| match f.fix {
            Some(SuggestedFix::Fill { cavity, .. }) => Some(&report.fills[cavity]),
            _ => None,
        })
        .collect();
    assert_eq!(picked.len(), 2);
    let filled = crate::fill::fill_stack(picked, fixture.schedule()).expect("two cavities to fill");
    let again = fixture.input_with(crate::input::ReviewLeaf::new(
        Some("Filled cavities"),
        Role::Model,
        &filled,
    ));
    let after = crate::runner::review_with(&again, &crate::runner::ReviewOptions::default());
    assert_eq!(
        trap_tiers(&after.findings),
        [Severity::Error],
        "{:?}",
        after.findings
    );
    assert!(after.fills.is_empty());
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
        ..drains_only()
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
    // Below its floor the block stands on the plate: a drain there would
    // end at the plate and relieve nothing while printing.
    assert_eq!(cup.fix, None);
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

// ---- Openings as wide as a drain --------------------------------------------

#[test]
fn a_one_pixel_gap_in_a_wall_does_not_drain_a_trap() {
    // The closed box of `a_closed_hollow_box_is_a_resin_trap`, its left wall
    // cut on layer 8 by a gap one pixel tall: a hairline, like the 3 x 3
    // pinhole vent of `a_pinhole_vent_leaves_a_trap`.
    let fixture = walls_with_gap(
        FixtureBuilder::new(16)
            .model(0..3, block())
            .model(3..8, walls())
            .model(9..13, walls())
            .model(13..16, block()),
        8,
        1.0,
    )
    .build();
    assert_eq!(spans(&traps(&fixture)), vec![(CheckKind::ResinTrap, 3, 12)]);
}

#[test]
fn a_gap_as_wide_as_a_drain_opens_the_wall() {
    // The same box with a gap 3 pixels tall: the cavity below it is a cup
    // that opens there, and the part above drains through it.
    let fixture = walls_with_gap(
        FixtureBuilder::new(16)
            .model(0..3, block())
            .model(3..8, walls())
            .model(9..13, walls())
            .model(13..16, block()),
        8,
        3.0,
    )
    .build();
    assert_eq!(spans(&traps(&fixture)), vec![(CheckKind::SuctionCup, 3, 7)]);
}

#[test]
fn a_one_pixel_wide_slit_does_not_drain_a_trap() {
    // The box of `a_drain_in_the_floor_drains_the_cavity` with a 1 x 20
    // pixel slit for a drain: 20 pixels, but a hairline.
    let fixture = corner_pillars(FixtureBuilder::new(18), 0..4)
        .model(4..6, block_with_hole(68.0, 45.0, 69.0, 65.0))
        .model(6..14, walls())
        .model(14..16, block())
        .build();
    assert_eq!(spans(&traps(&fixture)), vec![(CheckKind::ResinTrap, 6, 13)]);

    // Without the width rule its 20 pixels drain the box.
    let any_width = TrapOptions {
        drain_width_px: 1,
        ..drains_only()
    };
    assert!(traps_with(&fixture, &any_width).is_empty());
}

#[test]
fn a_one_pixel_wide_crack_is_not_a_trap() {
    // A sealed 40 x 1 pixel crack through a block over 6 layers: 40 pixels
    // a layer, but a hairline that light bleed cures shut.
    let fixture = FixtureBuilder::new(12)
        .model(0..2, block())
        .model(2..8, block_with_hole(50.0, 55.0, 90.0, 56.0))
        .model(8..12, block())
        .build();
    assert!(traps(&fixture).is_empty(), "{:?}", traps(&fixture));

    let any_width = TrapOptions {
        drain_width_px: 1,
        min_trap_volume_mm3: 0.0,
        ..drains_only()
    };
    assert_eq!(
        spans(&traps_with(&fixture, &any_width)),
        vec![(CheckKind::ResinTrap, 2, 7)]
    );
}

// ---- Support pockets --------------------------------------------------------

#[test]
fn a_support_ring_under_a_hollow_model_is_still_a_cup() {
    // Support walls on layers 0-3 carry the model walls and lid: the cavity
    // is walled by the model on layers 4-11.
    let fixture = FixtureBuilder::new(16)
        .support(0..4, walls())
        .model(4..12, walls())
        .model(12..14, block())
        .build();
    let findings = traps(&fixture);
    assert_eq!(spans(&findings), vec![(CheckKind::SuctionCup, 0, 11)]);
    assert_eq!(findings[0].scope, RoleScope::Both);
}

#[test]
fn support_pads_in_a_recess_do_not_hide_it() {
    // Pads on layers 0-1, fused to the inside of the left and right walls,
    // make half the outer wall support there, but not above.
    let fixture = recess()
        .support(0..2, Shape::rect(45.0, 35.0, 50.0, 75.0))
        .support(0..2, Shape::rect(90.0, 35.0, 95.0, 75.0))
        .build();
    let findings = traps(&fixture);
    assert_eq!(spans(&findings), vec![(CheckKind::SuctionCup, 0, 4)]);
    assert_eq!(findings[0].scope, RoleScope::Both);
}

#[test]
fn a_support_sliver_on_the_plate_does_not_hide_a_recess() {
    // A 1-pixel support line across the recess on layer 0.
    let fixture = recess()
        .support(0..1, Shape::rect(69.0, 35.0, 70.0, 75.0))
        .build();
    assert_eq!(spans(&traps(&fixture)), vec![(CheckKind::SuctionCup, 0, 4)]);
}

#[test]
fn a_hollow_support_foot_in_a_recess_does_not_drain_it() {
    // A support ring on layers 0-2 inside a recess on layers 0-5: its bore
    // joins the recess where the ring ends.
    let fixture = FixtureBuilder::new(10)
        .model(0..6, walls())
        .model(6..9, block())
        .support(0..3, Shape::ring(70.0, 55.0, 10.0, 6.0))
        .build();
    assert_eq!(spans(&traps(&fixture)), vec![(CheckKind::SuctionCup, 0, 5)]);
}

#[test]
fn a_raft_hole_under_an_open_box_is_still_a_cup() {
    // A raft with a hole on layers 0-2, under a box with no floor and with a
    // lid: the hole is the bottom of the cavity of the box.
    let fixture = FixtureBuilder::new(16)
        .support(0..3, Shape::frame(20.0, 10.0, 180.0, 110.0, 30.0))
        .model(3..12, Shape::frame(40.0, 30.0, 160.0, 90.0, 15.0))
        .model(12..15, Shape::rect(40.0, 30.0, 160.0, 90.0))
        .build();
    let findings = traps(&fixture);
    assert_eq!(spans(&findings), vec![(CheckKind::SuctionCup, 0, 11)]);
    assert_eq!(findings[0].scope, RoleScope::Both);
}

/// A 50 x 10 pixel recess on layers 0-4 with nothing above, its long walls
/// of role `long` and its short walls of role `short`; mirrored across a
/// diagonal, which swaps rows and columns (10 x 50 pixels), when `tall`.
fn walled_recess(long: Role, short: Role, tall: bool) -> Fixture {
    let place = |x0: f64, y0: f64, x1: f64, y1: f64| {
        if tall {
            // Mirrored about the line x - y = 20: (x, y) to (y + 20, x - 20).
            Shape::rect(y0 + 20.0, x0 - 20.0, y1 + 20.0, x1 - 20.0)
        } else {
            Shape::rect(x0, y0, x1, y1)
        }
    };
    let leaf = |role: Role| match role {
        Role::Model => "Model",
        Role::Support => "Supports",
    };
    FixtureBuilder::new(10)
        .shape(leaf(long), long, 0..5, place(45.0, 45.0, 105.0, 50.0))
        .shape(leaf(long), long, 0..5, place(45.0, 60.0, 105.0, 65.0))
        .shape(leaf(short), short, 0..5, place(45.0, 50.0, 50.0, 60.0))
        .shape(leaf(short), short, 0..5, place(100.0, 50.0, 105.0, 60.0))
        .build()
}

#[test]
fn the_support_pocket_rule_does_not_depend_on_orientation() {
    for tall in [false, true] {
        // Model on the long walls: a recess in the model, a cup.
        let model = walled_recess(Role::Model, Role::Support, tall);
        assert_eq!(
            spans(&traps(&model)),
            vec![(CheckKind::SuctionCup, 0, 4)],
            "tall: {tall}"
        );
        // Support on the long walls: a pocket in the supports.
        let support = walled_recess(Role::Support, Role::Model, tall);
        assert!(traps(&support).is_empty(), "tall: {tall}");
    }
}

#[test]
fn a_pocket_in_the_supports_above_the_plate_is_not_reported() {
    // A solid raft on layers 0-2. On layers 3-5 its rim and the support
    // bases fused in the middle leave a trench, open from layer 6 on, where
    // a trunk carries a block.
    let fixture = FixtureBuilder::new(12)
        .support(0..3, Shape::rect(20.0, 10.0, 180.0, 110.0))
        .support(3..6, Shape::frame(20.0, 10.0, 180.0, 110.0, 10.0))
        .support(3..6, Shape::rect(60.0, 40.0, 140.0, 80.0))
        .support(6..10, Shape::rect(95.0, 55.0, 105.0, 65.0))
        .model(10..12, Shape::rect(80.0, 50.0, 120.0, 70.0))
        .build();
    assert!(traps(&fixture).is_empty(), "{:?}", traps(&fixture));
}

#[test]
fn support_walls_on_a_model_floor_are_a_cup() {
    // A support ring standing on a block: the floor of the pocket is model.
    let fixture = FixtureBuilder::new(10)
        .model(0..3, block())
        .support(3..8, walls())
        .build();
    let findings = traps(&fixture);
    assert_eq!(spans(&findings), vec![(CheckKind::SuctionCup, 3, 7)]);
    assert_eq!(findings[0].scope, RoleScope::Both);
}

// ---- Scope ------------------------------------------------------------------

#[test]
fn walls_in_front_and_behind_count_for_the_scope() {
    // `a_cavity_walled_by_model_and_support_has_both_scopes` with the
    // support wall along the first rows instead of on the right.
    let fixture = FixtureBuilder::new(10)
        .model(0..2, block())
        .model(2..7, Shape::rect(40.0, 30.0, 45.0, 80.0))
        .model(2..7, Shape::rect(95.0, 30.0, 100.0, 80.0))
        .model(2..7, Shape::rect(45.0, 75.0, 95.0, 80.0))
        .support(2..7, Shape::rect(45.0, 30.0, 95.0, 35.0))
        .model(7..9, block())
        .build();
    let findings = traps(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].scope, RoleScope::Both);
}

#[test]
fn a_trap_on_a_model_floor_between_support_sides_gets_a_drain() {
    // Model floor, lid, and front and back walls; support left and right.
    let fixture = FixtureBuilder::new(10)
        .model(0..2, block())
        .model(2..7, Shape::rect(40.0, 30.0, 100.0, 35.0))
        .model(2..7, Shape::rect(40.0, 75.0, 100.0, 80.0))
        .support(2..7, Shape::rect(40.0, 35.0, 45.0, 75.0))
        .support(2..7, Shape::rect(95.0, 35.0, 100.0, 75.0))
        .model(7..9, block())
        .build();
    let findings = traps(&fixture);
    assert_eq!(spans(&findings), vec![(CheckKind::ResinTrap, 2, 6)]);
    assert_eq!(findings[0].scope, RoleScope::Both);
    assert_close(drain_at(&findings[0]).z_mm, 2.0 * 0.05);
}

// ---- Drains for cups --------------------------------------------------------

/// A box with a floor on layers 4-5 and walls up to the last layer, held by
/// corner pillars over the open air.
fn cup_on_pillars() -> FixtureBuilder {
    corner_pillars(FixtureBuilder::new(12), 0..4)
        .model(4..6, block())
        .model(6..12, walls())
}

#[test]
fn a_cup_over_open_air_gets_a_drain_through_its_floor() {
    let fixture = cup_on_pillars().build();
    let findings = traps(&fixture);
    assert_eq!(spans(&findings), vec![(CheckKind::SuctionCup, 6, 11)]);
    let [x, y] = fixture.pixel_to_plate(hole_point(&fixture, 6));
    let drain = drain_at(&findings[0]);
    assert_close(drain.x_mm, x);
    assert_close(drain.y_mm, y);
    assert_close(drain.z_mm, 6.0 * 0.05);

    // Through a floor thicker than the longest drain, none.
    let short = TrapOptions {
        max_drain_mm: 0.05,
        ..drains_only()
    };
    assert_eq!(traps_with(&fixture, &short)[0].fix, None);
}

#[test]
fn a_cup_whose_drain_would_end_in_a_support_gets_none() {
    // A pillar under the middle of the floor.
    let fixture = cup_on_pillars()
        .support(0..4, Shape::rect(60.0, 45.0, 80.0, 65.0))
        .build();
    let findings = traps(&fixture);
    assert_eq!(spans(&findings), vec![(CheckKind::SuctionCup, 6, 11)]);
    assert_eq!(findings[0].fix, None);
}

// ---- Severity ---------------------------------------------------------------

#[test]
fn a_wide_cup_is_an_error() {
    // A 150 x 90 pixel recess (135 mm²) under a block on the plate.
    let fixture = FixtureBuilder::new(20)
        .model(0..17, Shape::frame(20.0, 10.0, 180.0, 110.0, 5.0))
        .model(17..19, Shape::rect(20.0, 10.0, 180.0, 110.0))
        .build();
    let findings = traps(&fixture);
    assert_eq!(spans(&findings), vec![(CheckKind::SuctionCup, 0, 16)]);
    assert_eq!(findings[0].severity, Severity::Error);
    assert!(findings[0].detail.contains("135.0 mm²"), "{findings:?}");

    let lenient = TrapOptions {
        error_cup_area_mm2: 200.0,
        ..drains_only()
    };
    assert_eq!(
        traps_with(&fixture, &lenient)[0].severity,
        Severity::Warning
    );
}

#[test]
fn a_small_cup_no_drain_helps_is_offered_a_fill() {
    // The recess under a block on the plate: 20 mm² over 5 layers, 5 mm³.
    let fixture = recess().build();
    let input = fixture.input();
    let regions = fixture.regions();
    let ctx = ReviewContext::new(&input, &regions);
    let (findings, fills) = run_with_fills(&ctx, &TrapOptions::default());
    assert_eq!(spans(&findings), vec![(CheckKind::SuctionCup, 0, 4)]);
    assert!(matches!(
        findings[0].fix,
        Some(SuggestedFix::Fill { cavity: 0, .. })
    ));
    // Filled, the recess is gone.
    let filled = crate::fill::fill_stack(&fills, fixture.schedule()).expect("a fill");
    let again = fixture.input_with(crate::input::ReviewLeaf::new(None, Role::Model, &filled));
    let regions = PlateRegions::compute(&again);
    let after = run(
        &ReviewContext::new(&again, &regions),
        &TrapOptions::default(),
    );
    assert!(after.is_empty(), "{after:?}");

    // A cup holding more than a fill is worth keeps no fix.
    let big = TrapOptions {
        error_volume_mm3: 4.0,
        ..TrapOptions::default()
    };
    assert_eq!(traps_with(&fixture, &big)[0].fix, None);
}
