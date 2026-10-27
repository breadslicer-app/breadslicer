/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Output-check tests (#93).
//!
//! The default fake printer: a 200 x 120 pixel screen of 0.1 mm pixels (a
//! 20 x 12 mm plate centred on the origin), 0.05 mm layers and a 10 mm
//! maximum height. Shapes are in raster pixels.

use sc_print_primitives::Mirror;

use super::*;
use crate::finding::{CheckKind, Finding, RoleScope, Severity, SuggestedFix};
use crate::input::Role;
use crate::test_support::{Fixture, FixtureBuilder, Shape};

fn output(fixture: &Fixture) -> Vec<Finding> {
    fixture.check(|ctx| run(ctx, &OutputOptions::default()))
}

/// Check, severity and layer range of every finding, in listing order.
fn summary(findings: &[Finding]) -> Vec<(CheckKind, Severity, usize, usize)> {
    findings
        .iter()
        .map(|f| (f.check, f.severity, f.first_layer, f.last_layer))
        .collect()
}

fn assert_near(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "{actual} is not {expected}"
    );
}

/// Asserts that `focus` is the centre of raster pixel `pixel` at the
/// cutting height of `layer` (0.05 mm layers, cut at their middle).
fn assert_focus(fixture: &Fixture, finding: &Finding, pixel: [u32; 2], layer: usize) {
    let [x, y] = fixture.pixel_to_plate([f64::from(pixel[0]) + 0.5, f64::from(pixel[1]) + 0.5]);
    assert_near(finding.focus.x_mm, x);
    assert_near(finding.focus.y_mm, y);
    assert_near(finding.focus.z_mm, (layer as f64 + 0.5) * 0.05);
}

// ---------------------------------------------------------------------------
// Clean scenes.
// ---------------------------------------------------------------------------

#[test]
fn a_block_inside_the_plate_has_no_output_problem() {
    // 40 x 30 pixels (4 x 3 mm) on every layer of an 8-layer plate.
    let fixture = FixtureBuilder::new(8)
        .model(.., Shape::rect(40.0, 30.0, 80.0, 60.0))
        .build();
    let findings = fixture.check(|ctx| run(ctx, &OutputOptions::default()));
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn an_empty_plate_has_no_output_problem() {
    let fixture = FixtureBuilder::new(5).build();
    assert!(output(&fixture).is_empty());
}

#[test]
fn a_supported_scene_on_a_raft_is_clean() {
    // A raft, four pillars and a model resting on them, one pixel short of
    // every screen edge, under the maximum height.
    let mut builder = FixtureBuilder::new(12).support(0..2, Shape::rect(1.0, 1.0, 199.0, 119.0));
    for (x, y) in [(50.0, 40.0), (150.0, 40.0), (50.0, 80.0), (150.0, 80.0)] {
        builder = builder.support(2..7, Shape::circle(x, y, 4.0));
    }
    let fixture = builder
        .model(6..12, Shape::rect(40.0, 30.0, 160.0, 90.0))
        .build();
    let findings = output(&fixture);
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn a_model_held_by_a_support_does_not_float() {
    let fixture = FixtureBuilder::new(8)
        .model(.., Shape::rect(20.0, 20.0, 60.0, 50.0))
        .support(0..4, Shape::circle(130.0, 30.0, 4.0))
        .model(4..8, Shape::rect(120.0, 20.0, 140.0, 40.0))
        .build();
    assert!(output(&fixture).is_empty());
}

#[test]
fn a_branch_that_joins_the_body_above_does_not_float() {
    // The right pillar starts in mid-air (an island), but a bridge joins it
    // to the left pillar, which stands on the plate: one printed body.
    let fixture = FixtureBuilder::new(8)
        .model(.., Shape::rect(20.0, 20.0, 40.0, 40.0))
        .model(3..8, Shape::rect(60.0, 20.0, 80.0, 40.0))
        .model(6..8, Shape::rect(20.0, 20.0, 80.0, 40.0))
        .build();
    assert!(output(&fixture).is_empty());
}

// ---------------------------------------------------------------------------
// Plate edge.
// ---------------------------------------------------------------------------

#[test]
fn a_block_on_the_screen_edge_touches_it() {
    let fixture = FixtureBuilder::new(8)
        .model(.., Shape::rect(0.0, 30.0, 40.0, 60.0))
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![(CheckKind::PlateEdge, Severity::Error, 0, 7)]
    );
    let finding = &findings[0];
    assert_eq!(finding.scope, RoleScope::Model);
    assert_eq!(finding.title, "Part touches the screen edge");
    assert!(finding.detail.contains("on 8 layers"), "{}", finding.detail);
    assert_eq!(finding.fix, None);
    // Column 0, the row nearest the centroid (y = 45): rows 44 and 45 tie,
    // and the first in scan order wins.
    assert_focus(&fixture, finding, [0, 44], 0);
}

#[test]
fn each_body_on_the_edge_is_its_own_finding() {
    let fixture = FixtureBuilder::new(6)
        .model(.., Shape::rect(0.0, 10.0, 20.0, 30.0))
        .model(.., Shape::rect(60.0, 40.0, 100.0, 80.0))
        .support(.., Shape::rect(180.0, 80.0, 200.0, 100.0))
        .model(2..6, Shape::rect(120.0, 100.0, 140.0, 120.0))
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![
            (CheckKind::PlateEdge, Severity::Error, 0, 5),
            (CheckKind::PlateEdge, Severity::Error, 0, 5),
            (CheckKind::PlateEdge, Severity::Error, 2, 5),
            // The block on the top edge also starts in mid-air.
            (CheckKind::FloatingPart, Severity::Error, 2, 5),
        ]
    );
    let mut edges: Vec<(RoleScope, [u32; 2])> = Vec::new();
    for finding in &findings[..3] {
        for (scope, pixel) in [
            (RoleScope::Model, [0, 19]),
            (RoleScope::Support, [199, 89]),
            (RoleScope::Model, [130, 119]),
        ] {
            let [x, y] =
                fixture.pixel_to_plate([f64::from(pixel[0]) + 0.5, f64::from(pixel[1]) + 0.5]);
            if (finding.focus.x_mm - x).abs() < 1e-9 && (finding.focus.y_mm - y).abs() < 1e-9 {
                assert_eq!(finding.scope, scope);
                edges.push((scope, pixel));
            }
        }
    }
    assert_eq!(edges.len(), 3, "{findings:?}");
}

#[test]
fn a_body_spans_only_the_layers_it_touches_the_edge_on() {
    // A pillar with an arm that reaches the edge on layers 3 and 4.
    let fixture = FixtureBuilder::new(8)
        .model(.., Shape::rect(20.0, 20.0, 40.0, 40.0))
        .model(3..5, Shape::rect(0.0, 25.0, 20.0, 35.0))
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![(CheckKind::PlateEdge, Severity::Error, 3, 4)]
    );
    assert!(findings[0].detail.contains("on 2 layers"));
    assert_eq!(
        findings[0].focus.z_mm,
        fixture.input().layers()[3].sample_mm
    );
}

#[test]
fn a_part_off_the_plate_is_outside_the_printable_area() {
    // "Stray" lies 2 to 5 mm left of the plate on layers 2 to 5.
    let fixture = FixtureBuilder::new(8)
        .model(.., Shape::rect(40.0, 30.0, 80.0, 60.0))
        .shape(
            "Stray",
            Role::Model,
            2..6,
            Shape::rect(-50.0, 30.0, -20.0, 60.0),
        )
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![(CheckKind::PlateEdge, Severity::Error, 2, 5)]
    );
    let finding = &findings[0];
    assert_eq!(finding.title, "Part outside the printable area");
    assert_eq!(finding.scope, RoleScope::Model);
    assert!(
        finding
            .detail
            .starts_with("'Stray' reaches up to 5.00 mm past the edge"),
        "{}",
        finding.detail
    );
    assert!(finding.detail.contains("on 4 layers"), "{}", finding.detail);
    // On the farthest vertex, 5 mm left of the plate's -10 mm edge.
    assert_near(finding.focus.x_mm, -15.0);
    assert_near(finding.focus.z_mm, fixture.input().layers()[2].sample_mm);
}

#[test]
fn a_part_off_the_plate_is_reported_on_an_otherwise_empty_plate() {
    let fixture = FixtureBuilder::new(4)
        .support(.., Shape::rect(30.0, 130.0, 60.0, 150.0))
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![(CheckKind::PlateEdge, Severity::Error, 0, 3)]
    );
    assert_eq!(findings[0].scope, RoleScope::Support);
    assert!(
        findings[0]
            .detail
            .starts_with("'Supports' reaches up to 3.00 mm")
    );
}

#[test]
fn a_part_crossing_the_edge_is_reported_once() {
    // Cut by the left edge: the solid touches column 0 and the contour runs
    // 2 mm past it; the body finding covers both.
    let fixture = FixtureBuilder::new(6)
        .model(.., Shape::rect(-20.0, 30.0, 30.0, 60.0))
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![(CheckKind::PlateEdge, Severity::Error, 0, 5)]
    );
    assert_eq!(findings[0].title, "Part touches the screen edge");
}

#[test]
fn a_mirrored_screen_keeps_overhangs_on_their_edge() {
    // Mirrored in X, raster column 0 is the plate's +X side. The part cut
    // by column 0 reaches past +X: one finding. The part past the last
    // column lies past -X, where nothing touches the screen: a second one.
    let fixture = FixtureBuilder::new(6)
        .mirror(Mirror { x: true, y: false })
        .model(.., Shape::rect(-20.0, 30.0, 30.0, 60.0))
        .shape(
            "Stray",
            Role::Model,
            ..,
            Shape::rect(210.0, 30.0, 230.0, 60.0),
        )
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![
            (CheckKind::PlateEdge, Severity::Error, 0, 5),
            (CheckKind::PlateEdge, Severity::Error, 0, 5),
        ]
    );
    let titles: Vec<&str> = findings.iter().map(|f| f.title.as_str()).collect();
    assert!(titles.contains(&"Part touches the screen edge"));
    assert!(titles.contains(&"Part outside the printable area"));
    let outside = findings
        .iter()
        .find(|f| f.title == "Part outside the printable area")
        .unwrap();
    // 3 mm past the plate's -X edge at -10 mm.
    assert_near(outside.focus.x_mm, -13.0);
    assert!(outside.detail.contains("3.00 mm"), "{}", outside.detail);
}

#[test]
fn contours_on_the_plate_edge_itself_are_not_outside() {
    // The contour runs exactly along the plate's left edge: the solid
    // touches the screen edge, but nothing lies past the plate.
    let fixture = FixtureBuilder::new(4)
        .model(.., Shape::rect(0.0, 30.0, 10.0, 40.0))
        .build();
    let findings = output(&fixture);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].title, "Part touches the screen edge");
}

// ---------------------------------------------------------------------------
// Too tall.
// ---------------------------------------------------------------------------

#[test]
fn a_print_above_the_maximum_height_is_too_tall() {
    // 10 layers of 0.05 mm: 0.5 mm tall on a 0.3 mm printer. Layer 6 is the
    // first to end above 0.3 mm.
    let fixture = FixtureBuilder::new(10)
        .max_z_mm(0.3)
        .model(.., Shape::rect(40.0, 30.0, 80.0, 60.0))
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![(CheckKind::TooTall, Severity::Error, 6, 9)]
    );
    let finding = &findings[0];
    assert_eq!(finding.scope, RoleScope::Model);
    assert_eq!(
        finding.detail,
        "The print reaches 0.50 mm, 0.20 mm above the printer's maximum height of 0.30 mm, so its top 4 layers cannot be printed."
    );
    assert_focus(&fixture, finding, [60, 44], 6);
}

#[test]
fn a_print_exactly_at_the_maximum_height_fits() {
    let fixture = FixtureBuilder::new(10)
        .max_z_mm(0.5)
        .model(.., Shape::rect(40.0, 30.0, 80.0, 60.0))
        .build();
    assert!(output(&fixture).is_empty());
}

#[test]
fn empty_layers_above_the_maximum_height_are_not_too_tall() {
    // The solid ends at 0.3 mm; only empty layers rise above the limit.
    let fixture = FixtureBuilder::new(10)
        .max_z_mm(0.3)
        .model(0..6, Shape::rect(40.0, 30.0, 80.0, 60.0))
        .build();
    assert_eq!(
        summary(&output(&fixture)),
        vec![(CheckKind::EmptyLayer, Severity::Info, 6, 9)]
    );
}

#[test]
fn a_too_tall_support_is_scoped_to_supports() {
    let fixture = FixtureBuilder::new(10)
        .max_z_mm(0.3)
        .model(0..5, Shape::rect(40.0, 30.0, 80.0, 60.0))
        .support(.., Shape::rect(100.0, 30.0, 110.0, 40.0))
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![(CheckKind::TooTall, Severity::Error, 6, 9)]
    );
    assert_eq!(findings[0].scope, RoleScope::Support);
}

// ---------------------------------------------------------------------------
// Empty layers.
// ---------------------------------------------------------------------------

#[test]
fn nothing_on_the_build_plate_is_an_error() {
    let fixture = FixtureBuilder::new(8)
        .model(2..8, Shape::rect(40.0, 30.0, 80.0, 60.0))
        .build();
    let findings = output(&fixture);
    // The block floats because of the empty layers: one finding, not two.
    assert_eq!(
        summary(&findings),
        vec![(CheckKind::EmptyLayer, Severity::Error, 0, 1)]
    );
    let finding = &findings[0];
    assert_eq!(finding.title, "Nothing on the build plate");
    assert_eq!(finding.scope, RoleScope::Both);
    assert_eq!(
        finding.detail,
        "The first 2 layers are empty, so the first solid layer cures 0.10 mm above the plate with nothing to stick to."
    );
    // The block's centre, at the first empty layer's height.
    assert_focus(&fixture, finding, [60, 44], 0);
}

#[test]
fn a_gap_in_the_middle_is_an_error() {
    let fixture = FixtureBuilder::new(8)
        .model(0..3, Shape::rect(40.0, 30.0, 80.0, 60.0))
        .model(5..8, Shape::rect(40.0, 30.0, 80.0, 60.0))
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![(CheckKind::EmptyLayer, Severity::Error, 3, 4)]
    );
    assert_eq!(findings[0].title, "Gap in the print");
    assert_eq!(
        findings[0].detail,
        "2 empty layers leave a 0.10 mm gap, so everything above it has nothing to rest on."
    );
    assert_focus(&fixture, &findings[0], [60, 44], 3);
}

#[test]
fn separate_gaps_are_separate_findings() {
    let fixture = FixtureBuilder::new(12)
        .model(0..2, Shape::rect(40.0, 30.0, 80.0, 60.0))
        .model(3..5, Shape::rect(40.0, 30.0, 80.0, 60.0))
        .model(7..12, Shape::rect(40.0, 30.0, 80.0, 60.0))
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![
            (CheckKind::EmptyLayer, Severity::Error, 2, 2),
            (CheckKind::EmptyLayer, Severity::Error, 5, 6),
        ]
    );
    assert!(
        findings[0]
            .detail
            .starts_with("1 empty layer leaves a 0.05 mm gap")
    );
}

#[test]
fn an_empty_layer_at_the_top_is_info() {
    // Like a real plate whose top falls below the last layer's cut.
    let fixture = FixtureBuilder::new(8)
        .model(0..7, Shape::rect(40.0, 30.0, 80.0, 60.0))
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![(CheckKind::EmptyLayer, Severity::Info, 7, 7)]
    );
    assert_eq!(findings[0].title, "Empty layer at the top");
    assert_eq!(
        findings[0].detail,
        "The last layer is empty (0.05 mm), which adds print time and prints nothing."
    );
    assert_focus(&fixture, &findings[0], [60, 44], 7);
}

#[test]
fn trailing_empty_layers_are_one_finding() {
    let fixture = FixtureBuilder::new(8)
        .model(0..5, Shape::rect(40.0, 30.0, 80.0, 60.0))
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![(CheckKind::EmptyLayer, Severity::Info, 5, 7)]
    );
    assert_eq!(findings[0].title, "Empty layers at the top");
    assert!(
        findings[0]
            .detail
            .starts_with("The last 3 layers are empty (0.15 mm)")
    );
}

// ---------------------------------------------------------------------------
// Floating parts.
// ---------------------------------------------------------------------------

#[test]
fn a_part_with_nothing_below_it_floats() {
    let fixture = FixtureBuilder::new(8)
        .model(.., Shape::rect(20.0, 20.0, 60.0, 50.0))
        .model(4..8, Shape::rect(120.0, 20.0, 140.0, 40.0))
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![(CheckKind::FloatingPart, Severity::Error, 4, 7)]
    );
    let finding = &findings[0];
    assert_eq!(finding.scope, RoleScope::Model);
    // 20 x 20 pixels of 0.01 mm² on 4 layers of 0.05 mm: 0.8 mm³.
    assert_eq!(
        finding.detail,
        "This 0.80 mm³ part starts 0.20 mm above the build plate and never joins anything standing on the plate, so it will not stay attached."
    );
    // The block's inside pixel: column 130, rows 29 and 30 tie on the
    // centroid row and the first in scan order wins.
    assert_focus(&fixture, finding, [130, 29], 4);
    assert_eq!(
        finding.fix,
        Some(SuggestedFix::AddSupport { at: finding.focus })
    );
}

#[test]
fn a_floating_support_has_no_fix() {
    let fixture = FixtureBuilder::new(8)
        .model(.., Shape::rect(20.0, 20.0, 60.0, 50.0))
        .support(3..6, Shape::circle(130.0, 30.0, 4.0))
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![(CheckKind::FloatingPart, Severity::Error, 3, 5)]
    );
    assert_eq!(findings[0].scope, RoleScope::Support);
    assert_eq!(findings[0].fix, None);
}

#[test]
fn a_part_shifted_sideways_without_overlap_floats() {
    // The upper block touches the lower one only along an edge, one layer
    // up: nothing straight below it, so it is not attached.
    let fixture = FixtureBuilder::new(8)
        .model(0..4, Shape::rect(20.0, 20.0, 40.0, 40.0))
        .model(4..8, Shape::rect(40.0, 20.0, 60.0, 40.0))
        .build();
    assert_eq!(
        summary(&output(&fixture)),
        vec![(CheckKind::FloatingPart, Severity::Error, 4, 7)]
    );
}

#[test]
fn a_part_floating_above_a_gap_is_still_reported() {
    // Everything above the gap floats because of it (one empty-layer
    // finding); the small block further up floats on its own.
    let fixture = FixtureBuilder::new(10)
        .model(0..3, Shape::rect(20.0, 20.0, 60.0, 50.0))
        .model(5..10, Shape::rect(20.0, 20.0, 60.0, 50.0))
        .model(7..10, Shape::rect(120.0, 20.0, 140.0, 40.0))
        .build();
    assert_eq!(
        summary(&output(&fixture)),
        vec![
            (CheckKind::EmptyLayer, Severity::Error, 3, 4),
            (CheckKind::FloatingPart, Severity::Error, 7, 9),
        ]
    );
}

#[test]
fn a_floating_model_on_a_floating_support_is_one_part() {
    // A support pillar that starts in mid-air, holding a model: one body,
    // focused and fixed at the pillar's foot, which has no model in it.
    let fixture = FixtureBuilder::new(10)
        .model(.., Shape::rect(20.0, 20.0, 60.0, 50.0))
        .support(3..6, Shape::circle(130.0, 30.0, 4.0))
        .model(6..10, Shape::rect(120.0, 20.0, 140.0, 40.0))
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![(CheckKind::FloatingPart, Severity::Error, 3, 9)]
    );
    assert_eq!(findings[0].scope, RoleScope::Both);
    assert_eq!(findings[0].fix, None);
}
