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

use sc_print_primitives::{Mirror, PixelResolution};
use sc_sl1cer::contour::ContourLayer;
use sc_sl1cer::profile::AaLevel;
use sc_sl1cer::raster::pixel_map::PixelMap;
use sc_sl1cer::raster::rle::LayerRaster;
use sc_sl1cer::raster::{rasterize_layer, rasterize_layer_binary};

use super::*;
use crate::fill::PlateRect;
use crate::input::ReviewLeaf;
use crate::runner::{ReviewOptions, check_regions};
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

/// The options under which every trap is an error, whatever its size. The
/// tiers have tests of their own.
fn all_errors() -> TrapOptions {
    TrapOptions {
        info_volume_mm3: 0.0,
        error_volume_mm3: 0.0,
        ..TrapOptions::default()
    }
}

fn traps(fixture: &Fixture) -> Vec<Finding> {
    traps_with(fixture, &all_errors())
}

fn traps_with(fixture: &Fixture, options: &TrapOptions) -> Vec<Finding> {
    fixture.check(|ctx| run(ctx, options, &SweepOptions::default()).0)
}

/// The findings of `options`, with air opened `width` pixels wide.
fn traps_of_width(fixture: &Fixture, options: &TrapOptions, width: u32) -> Vec<Finding> {
    let sweep = SweepOptions {
        drain_width_px: width,
        ..SweepOptions::default()
    };
    let input = fixture.input();
    let regions = PlateRegions::compute(&input, &sweep);
    let mut findings = run(&ReviewContext::new(&input, &regions), options, &sweep).0;
    crate::finding::sort_findings(&mut findings);
    findings
}

/// The review of `input` with the default options.
fn reviewed(input: &ReviewInput<'_>) -> crate::runner::ReviewReport {
    let regions = PlateRegions::compute(input, &SweepOptions::default());
    check_regions(
        &ReviewContext::new(input, &regions),
        &ReviewOptions::default(),
    )
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

/// The polygons of `layer` of `fixture`, with `fills` merged.
fn merged_with_fills(fixture: &Fixture, fills: &[CavityFill], layer: usize) -> ContourLayer {
    let filled = crate::fill::fill_stack(fills, fixture.schedule()).expect("a fill");
    let input = fixture.input_with(ReviewLeaf::new(Some("Filled cavities"), &filled));
    let z_mm = filled.layers()[layer].z_mm();
    ContourLayer::new(
        z_mm,
        input
            .leaves()
            .iter()
            .flat_map(|leaf| leaf.stack.layers()[layer].polygons().iter().cloned())
            .collect(),
    )
}

/// Asserts that `finding` is offered a fill and no drain.
fn assert_fill_only(finding: &Finding) {
    assert!(
        matches!(finding.fix, Some(SuggestedFix::Fill { .. })),
        "{:?}",
        finding.fix
    );
}

/// Asserts that `finding` asks for drain holes and offers no fill.
fn assert_drain_only(finding: &Finding) {
    assert!(
        matches!(finding.fix, Some(SuggestedFix::AddDrain { .. })),
        "{:?}",
        finding.fix
    );
}

/// Where a cavity's fix points: its floor.
fn aimed_at(finding: &Finding) -> PlatePoint {
    match finding.fix {
        Some(SuggestedFix::AddDrain { at } | SuggestedFix::Fill { at, .. }) => at,
        other => panic!("expected a cavity's fix, got {other:?}"),
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
    let findings = traps_with(&fixture, &TrapOptions::default());
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
    let fixture = FixtureBuilder::new(12)
        .model(4..12, walls())
        .build();
    assert!(traps(&fixture).is_empty(), "{:?}", traps(&fixture));
}

#[test]
fn a_dome_on_supports_is_not_a_trap() {
    // A box with a lid and no floor, held by pillars: its cavity opens
    // toward the plate, into the open air between the pillars.
    let fixture = FixtureBuilder::new(14)
        .model(5..10, walls())
        .model(10..12, block())
        .build();
    assert!(traps(&fixture).is_empty(), "{:?}", traps(&fixture));
}

#[test]
fn a_drain_in_the_floor_drains_the_cavity() {
    // A 4 x 4 pixel drain (16 pixels) through the floor of a closed box held
    // by pillars.
    let fixture = FixtureBuilder::new(18)
        .model(4..6, block_with_hole(68.0, 53.0, 72.0, 57.0))
        .model(6..14, walls())
        .model(14..16, block())
        .build();
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
        min_cup_share: 0.0,
        ..all_errors()
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
    let drain = aimed_at(trap);
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
    let findings = traps_with(&fixture, &TrapOptions::default());
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
            "fill"
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
    assert_eq!(tier(&medium, &strict).2, "fill");
    assert_eq!(trap_severity(0.99, &defaults), Severity::Info);
    assert_eq!(trap_severity(1.0, &defaults), Severity::Warning);
    assert_eq!(trap_severity(26.9, &defaults), Severity::Warning);
    assert_eq!(trap_severity(27.0, &defaults), Severity::Error);
}

/// Under [`SMALL_TRAP_MAX_ML`] a trap is filled; from there on it may be a
/// hollow, and it only asks for drain holes (#431). 70 x 70 pixels over 405
/// and 410 layers hold 992.25 and 1004.5 mm³.
#[test]
fn a_trap_is_filled_only_under_a_millilitre() {
    let review = |layers| {
        let fixture = sealed_box(70.0, layers).build();
        let input = fixture.input();
        let regions = fixture.regions();
        run(
            &ReviewContext::new(&input, &regions),
            &TrapOptions::default(),
            &SweepOptions::default(),
        )
    };
    let (findings, fills) = review(405);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_fill_only(&findings[0]);
    assert_eq!(fills.len(), 1);
    assert_eq!(fills[0].slabs.len(), 405);

    let (findings, fills) = review(410);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].check, CheckKind::ResinTrap);
    assert_eq!(findings[0].severity, Severity::Error);
    assert_drain_only(&findings[0]);
    assert!(findings[0].detail.contains("1004.5 mm³"), "{findings:?}");
    assert!(findings[0].detail.ends_with("Add drain holes."));
    assert!(fills.is_empty(), "no pixels to fill");
    // Its pixels are read only as far as they are highlighted.
    let footprint = findings[0].footprint.as_ref().expect("a footprint");
    assert_eq!(footprint.slabs.len(), MAX_FOOTPRINT_LAYERS);
    assert!(footprint.slabs.iter().all(|slab| slab.rim.is_empty()));
    let first = &footprint.slabs[0];
    assert_close(first.bottom_mm, 2.0 * 0.05);
    assert_close(first.rects.iter().map(PlateRect::area_mm2).sum(), 49.0);
}

/// A trap's fill and index stay paired when a larger trap is listed too.
#[test]
fn a_small_trap_names_its_fill_beside_a_large_one() {
    // Side by side over 300 layers: 70 x 40 pixels (420 mm³ is small) and
    // 90 x 90 (1215 mm³ is not).
    let outside = || Shape::rect(10.0, 10.0, 190.0, 110.0);
    let fixture = FixtureBuilder::new(304)
        .model(0..2, outside())
        .model(
            2..302,
            Shape::polygon(
                ring(10.0, 10.0, 190.0, 110.0),
                vec![ring(15.0, 15.0, 85.0, 55.0), ring(95.0, 15.0, 185.0, 105.0)],
            ),
        )
        .model(302..304, outside())
        .build();
    let input = fixture.input();
    let regions = fixture.regions();
    let ctx = ReviewContext::new(&input, &regions);
    let (findings, fills) = run(&ctx, &TrapOptions::default(), &SweepOptions::default());
    assert_eq!(findings.len(), 2, "{findings:?}");
    assert_eq!(fills.len(), 1);
    let small = findings
        .iter()
        .find(|f| matches!(f.fix, Some(SuggestedFix::Fill { .. })))
        .expect("the small trap");
    assert!(matches!(
        small.fix,
        Some(SuggestedFix::Fill { cavity: 0, .. })
    ));
    assert!(small.detail.contains("420.0 mm³"), "{small:?}");
    assert_close(fills[0].volume_mm3(), 420.0);
    assert_eq!(
        findings
            .iter()
            .filter(|f| matches!(f.fix, Some(SuggestedFix::AddDrain { .. })))
            .count(),
        1
    );
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
    let (findings, fills) = run(&ctx, &TrapOptions::default(), &SweepOptions::default());
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
fn a_pinhole_drain_leaves_a_trap() {
    // The same box as `a_drain_in_the_floor_drains_the_cavity`, with a
    // 3 x 3 pixel drain: 9 pixels, under the 10-pixel threshold.
    let fixture = FixtureBuilder::new(18)
        .model(4..6, block_with_hole(68.0, 53.0, 71.0, 56.0))
        .model(6..14, walls())
        .model(14..16, block())
        .build();
    let findings = traps(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    let trap = &findings[0];
    assert_eq!(trap.check, CheckKind::ResinTrap);
    assert_eq!((trap.first_layer, trap.last_layer), (6, 13));
    assert_close(aimed_at(trap).z_mm, 6.0 * 0.05);

    // A lower threshold lets the same drain work.
    let loose = TrapOptions {
        drain_px: 9,
        ..all_errors()
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
    assert_close(aimed_at(&findings[0]).z_mm, 0.2);
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
    let drain = aimed_at(&findings[0]);
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
        "A cavity of 7.00 mm³ over 7 layers (0.35 mm) is closed toward the plate and opens only above, so up to 20.0 mm² of it pulls on the film at every peel. Add drain holes."
    );
    assert_drain_only(cup);
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
fn a_cavity_on_the_plate_is_a_suction_cup() {
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
    assert!(cup.detail.ends_with("Add drain holes."), "{cup:?}");
    assert_drain_only(cup);
}

#[test]
fn a_cup_small_beside_its_layer_is_not_reported() {
    // A 10 x 10 pixel recess under a 180 x 100 pixel block on the plate:
    // under 1 % of what each of its layers cures (#156).
    let fixture = FixtureBuilder::new(8)
        .model(
            0..5,
            Shape::polygon(
                ring(10.0, 10.0, 190.0, 110.0),
                vec![ring(95.0, 55.0, 105.0, 65.0)],
            ),
        )
        .model(5..8, Shape::rect(10.0, 10.0, 190.0, 110.0))
        .build();
    let any_area = TrapOptions {
        min_cup_area_mm2: 0.0,
        ..TrapOptions::default()
    };
    assert!(traps_with(&fixture, &any_area).is_empty());
    let any_share = TrapOptions {
        min_cup_share: 0.0,
        ..any_area
    };
    let found = traps_with(&fixture, &any_share);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].check, CheckKind::SuctionCup);
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
    let fixture = FixtureBuilder::new(18)
        .model(4..6, block_with_hole(68.0, 45.0, 69.0, 65.0))
        .model(6..14, walls())
        .model(14..16, block())
        .build();
    assert_eq!(spans(&traps(&fixture)), vec![(CheckKind::ResinTrap, 6, 13)]);

    // Without the width rule its 20 pixels drain the box.
    assert!(traps_of_width(&fixture, &all_errors(), 1).is_empty());
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

    let any_size = TrapOptions {
        min_trap_volume_mm3: 0.0,
        ..all_errors()
    };
    assert_eq!(
        spans(&traps_of_width(&fixture, &any_size, 1)),
        vec![(CheckKind::ResinTrap, 2, 7)]
    );
}

#[test]
fn a_single_layer_ring_on_the_plate_is_a_cup() {
    // One layer: the hole is closed by the plate and open to the resin
    // above at once.
    let fixture = FixtureBuilder::new(1)
        .shape("Ring", .., Shape::ring(100.0, 60.0, 20.0, 15.0))
        .build();
    let findings = traps(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].check, CheckKind::SuctionCup);
    assert_eq!((findings[0].first_layer, findings[0].last_layer), (0, 0));
}

// ---- Drains for cups --------------------------------------------------------

#[test]
fn a_cup_over_open_air_asks_for_a_drain_at_its_floor() {
    let fixture = FixtureBuilder::new(12)
        .model(4..6, block())
        .model(6..12, walls())
        .build();
    let findings = traps(&fixture);
    assert_eq!(spans(&findings), vec![(CheckKind::SuctionCup, 6, 11)]);
    let [x, y] = fixture.pixel_to_plate(hole_point(&fixture, 6));
    let drain = aimed_at(&findings[0]);
    assert_close(drain.x_mm, x);
    assert_close(drain.y_mm, y);
    assert_close(drain.z_mm, 6.0 * 0.05);
}

#[test]
fn a_filled_cavity_is_gone_on_the_next_review() {
    // A tiny and a medium trap side by side, and one of more than a
    // millilitre: fill the two that offer it, as the app does on the next
    // slice.
    let sealed = |builder: FixtureBuilder, [x0, y0, x1, y1]: [f64; 4], top: usize| {
        builder
            .model(0..2, Shape::rect(x0, y0, x1, y1))
            .model(2..top, Shape::frame(x0, y0, x1, y1, 5.0))
            .model(top..top + 2, Shape::rect(x0, y0, x1, y1))
    };
    // 0.2 mm3, 10 mm3 and 1064.7 mm3.
    let builder = sealed(FixtureBuilder::new(344), [10.0, 10.0, 30.0, 30.0], 6);
    let builder = sealed(builder, [40.0, 10.0, 100.0, 70.0], 10);
    let fixture = sealed(builder, [110.0, 10.0, 190.0, 110.0], 340).build();
    let report = reviewed(&fixture.input());
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
    let again = fixture.input_with(ReviewLeaf::new(Some("Filled cavities"), &filled));
    let after = reviewed(&again);
    assert_eq!(
        trap_tiers(&after.findings),
        [Severity::Error],
        "{:?}",
        after.findings
    );
    // The trap left has no pixels to fill.
    assert!(after.fills.is_empty());
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
        ..all_errors()
    };
    assert_eq!(
        traps_with(&fixture, &lenient)[0].severity,
        Severity::Warning
    );
}

/// A cup may be the hollow itself, however little it holds (#431).
#[test]
fn no_cup_is_ever_offered_a_fill() {
    // The recess under a block on the plate: 20 mm² over 5 layers, 5 mm³.
    let fixture = recess().build();
    let input = fixture.input();
    let regions = fixture.regions();
    let ctx = ReviewContext::new(&input, &regions);
    let (findings, fills) = run(&ctx, &TrapOptions::default(), &SweepOptions::default());
    assert_eq!(spans(&findings), vec![(CheckKind::SuctionCup, 0, 4)]);
    assert_drain_only(&findings[0]);
    assert!(fills.is_empty(), "no pixels to fill");
    // It still shows its pixels.
    let footprint = findings[0].footprint.as_ref().expect("a footprint");
    assert_eq!(footprint.slabs.len(), 5);
}

/// The pixel values of `raster`, row by row.
fn raster_pixels(raster: &LayerRaster) -> Vec<u8> {
    raster
        .runs()
        .iter()
        .flat_map(|run| std::iter::repeat_n(run.value, run.length as usize))
        .collect()
}

#[test]
fn a_fill_closes_flush_with_the_model() {
    // #371: a round sealed cavity around a pillar. Its fill must close
    // against the curved walls and the pillar with no seam: no partly lit
    // pixel in an anti-aliased layer, no gap a finer grid (the slice view's
    // cut) can see, and no binary pixel changed by the rim.
    let (cx, cy, r) = (100.0, 60.0, 45.0);
    let fixture = FixtureBuilder::new(23)
        .model(0..3, Shape::circle(cx, cy, r))
        .model(3..20, Shape::ring(cx, cy, r, 35.3))
        .model(3..20, Shape::circle(112.3, 63.1, 4.2))
        .model(20..23, Shape::circle(cx, cy, r))
        .build();
    let input = fixture.input();
    let regions = fixture.regions();
    let ctx = ReviewContext::new(&input, &regions);
    let (_, fills) = run(&ctx, &TrapOptions::default(), &SweepOptions::default());
    assert_eq!(fills.len(), 1);
    assert!(fills[0].slabs.iter().all(|slab| !slab.rim.is_empty()));
    let layer = 10;
    let merged = merged_with_fills(&fixture, &fills, layer);
    // Well inside the solid disc the layer now is, a pixel and a half from
    // its outer edge.
    let inside = |x: f64, y: f64| (x - cx).hypot(y - cy) < r - 1.5;
    let pixel_map = *input.pixel_map();
    let (width, height) = (input.width_px(), input.height_px());
    let pixels = |raster: &LayerRaster| {
        let values = raster_pixels(raster);
        (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .filter(|&(x, y)| inside(f64::from(x) + 0.5, f64::from(y) + 0.5))
            .map(|(x, y)| values[(y * width + x) as usize])
            .collect::<Vec<u8>>()
    };
    let aa = pixels(&rasterize_layer(&merged, &pixel_map, AaLevel::X8, 0.0));
    let partial = aa.iter().filter(|&&v| v < 255).count();
    assert_eq!(partial, 0, "{partial} pixels not fully lit");

    // The rim lights no pixel the binary layer did not light already.
    let mut bare = fills.clone();
    for slab in &mut bare[0].slabs {
        slab.rim.clear();
    }
    let without_rim = merged_with_fills(&fixture, &bare, layer);
    assert_eq!(
        rasterize_layer_binary(&merged, &pixel_map).runs(),
        rasterize_layer_binary(&without_rim, &pixel_map).runs()
    );
    assert!(
        pixels(&rasterize_layer_binary(&merged, &pixel_map))
            .iter()
            .all(|&v| v == 255)
    );

    // A finer grid, offset from the screen's, as the slice view's cut
    // samples the layer.
    let (fine_w, fine_h) = (1733_u32, 1039_u32);
    let fine = PixelMap::new(
        pixel_map.plate_x(),
        pixel_map.plate_y(),
        PixelResolution::new(fine_w, fine_h).expect("a grid"),
        Mirror::default(),
    );
    let values = raster_pixels(&rasterize_layer_binary(&merged, &fine));
    let (sx, sy) = (
        f64::from(width) / f64::from(fine_w),
        f64::from(height) / f64::from(fine_h),
    );
    let holes = (0..fine_h)
        .flat_map(|y| (0..fine_w).map(move |x| (x, y)))
        .filter(|&(x, y)| inside((f64::from(x) + 0.5) * sx, (f64::from(y) + 0.5) * sy))
        .filter(|&(x, y)| values[(y * fine_w + x) as usize] == 0)
        .count();
    assert_eq!(holes, 0, "{holes} holes along the seam");
}
