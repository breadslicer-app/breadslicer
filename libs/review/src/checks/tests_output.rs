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
use crate::finding::{CheckKind, Finding, RoleScope, Severity};
use crate::test_support::{Fixture, FixtureBuilder, Shape};

fn output(fixture: &Fixture) -> Vec<Finding> {
    fixture.check(|ctx| run(ctx, &OutputOptions::default(), &SweepOptions::default()))
}

/// Check, severity and layer range of every finding, in listing order.
fn summary(findings: &[Finding]) -> Vec<(CheckKind, Severity, usize, usize)> {
    findings
        .iter()
        .map(|f| (f.check, f.severity, f.first_layer, f.last_layer))
        .collect()
}

/// The title of a part cut by the edge of the plate.
const CUT: &str = "Part cut by the plate edge";
/// The title of a part on the screen's outermost pixels.
const TOUCH: &str = "Part touches the screen edge";
/// The title of contours past the plate that light no pixel.
const OUTSIDE: &str = "Part outside the printable area";

/// The one finding titled `title`.
fn titled<'f>(findings: &'f [Finding], title: &str) -> &'f Finding {
    let mut found = findings.iter().filter(|f| f.title == title);
    let finding = found
        .next()
        .unwrap_or_else(|| panic!("no {title}: {findings:?}"));
    assert!(found.next().is_none(), "two {title}: {findings:?}");
    finding
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
    let findings = fixture.check(|ctx| run(ctx, &OutputOptions::default(), &SweepOptions::default()));
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn an_empty_plate_has_no_output_problem() {
    let fixture = FixtureBuilder::new(5).build();
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
    // Its contour runs along the plate's edge: nothing is cut off.
    assert_eq!(
        summary(&findings),
        vec![(CheckKind::PlateEdge, Severity::Warning, 0, 7)]
    );
    let finding = &findings[0];
    assert_eq!(finding.scope, RoleScope::Model);
    assert_eq!(finding.title, TOUCH);
    assert!(finding.detail.contains("on 8 layers"), "{}", finding.detail);
    assert_eq!(finding.fix, None);
    // Column 0, the row nearest the centroid (y = 45): rows 44 and 45 tie,
    // and the first in scan order wins.
    assert_focus(&fixture, finding, [0, 44], 0);
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
        vec![(CheckKind::PlateEdge, Severity::Warning, 3, 4)]
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
    assert_eq!(finding.title, OUTSIDE);
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
fn a_part_crossing_the_edge_is_reported_once() {
    // Cut by the left edge: the solid touches column 0 and the contour runs
    // 2 mm past it; one finding covers both.
    let fixture = FixtureBuilder::new(6)
        .model(.., Shape::rect(-20.0, 30.0, 30.0, 60.0))
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![(CheckKind::PlateEdge, Severity::Error, 0, 5)]
    );
    let finding = &findings[0];
    assert_eq!(finding.title, CUT);
    assert_eq!(finding.scope, RoleScope::Model);
    assert_eq!(
        finding.detail,
        "It reaches up to 2.00 mm past the edge of the plate on 6 layers, and nothing beyond the screen is printed."
    );
    // Column 0, the row nearest the centroid (y = 45): rows 44 and 45 tie.
    assert_focus(&fixture, finding, [0, 44], 0);
}

#[test]
fn a_mirrored_screen_matches_overhangs_to_their_pixels() {
    // Mirrored in X, raster column 0 is the plate's +X side. The part cut
    // by column 0 reaches past +X and lights the column: cut. The part past
    // the last column lies past -X and lights nothing: outside.
    let fixture = FixtureBuilder::new(6)
        .mirror(Mirror { x: true, y: false })
        .model(.., Shape::rect(-20.0, 30.0, 30.0, 60.0))
        .shape(
            "Stray",
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
    titled(&findings, CUT);
    let outside = titled(&findings, OUTSIDE);
    // 3 mm past the plate's -X edge at -10 mm.
    assert_near(outside.focus.x_mm, -13.0);
    assert!(outside.detail.contains("3.00 mm"), "{}", outside.detail);
}

#[test]
fn a_part_off_the_plate_is_reported_beside_a_part_on_the_last_pixels() {
    // The block lies wholly on the plate but lights column 0; 'Stray' lies
    // 2 to 5 mm left of the plate.
    let fixture = FixtureBuilder::new(4)
        .model(.., Shape::rect(0.4, 30.0, 10.0, 40.0))
        .shape(
            "Stray",
            ..,
            Shape::rect(-50.0, 80.0, -20.0, 100.0),
        )
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![
            (CheckKind::PlateEdge, Severity::Error, 0, 3),
            (CheckKind::PlateEdge, Severity::Warning, 0, 3),
        ]
    );
    assert_eq!(findings[0].title, OUTSIDE);
    assert!(
        findings[0]
            .detail
            .starts_with("'Stray' reaches up to 5.00 mm")
    );
    assert_eq!(findings[1].title, TOUCH);
}

#[test]
fn an_unrelated_edge_contact_does_not_split_a_part_off_the_plate() {
    // 'Stray' is off the plate on all 10 layers; the pillar's arm reaches
    // the same edge on layer 5 only.
    let fixture = FixtureBuilder::new(10)
        .model(.., Shape::rect(20.0, 20.0, 40.0, 40.0))
        .model(5..6, Shape::rect(0.0, 25.0, 20.0, 35.0))
        .shape(
            "Stray",
            ..,
            Shape::rect(-50.0, 80.0, -20.0, 100.0),
        )
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![
            (CheckKind::PlateEdge, Severity::Error, 0, 9),
            (CheckKind::PlateEdge, Severity::Warning, 5, 5),
        ]
    );
    assert_eq!(findings[0].title, OUTSIDE);
    assert!(
        findings[0].detail.contains("on 10 layers"),
        "{}",
        findings[0].detail
    );
}

#[test]
fn a_part_across_a_corner_is_reported_once() {
    // On the plate the triangle touches column 0 only; off the plate it
    // reaches 3 mm past both the left and the bottom edges.
    let fixture = FixtureBuilder::new(4)
        .model(
            ..,
            Shape::polygon(
                vec![[-30.0, -30.0], [20.0, 60.0], [-30.0, 60.0]],
                Vec::new(),
            ),
        )
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![(CheckKind::PlateEdge, Severity::Error, 0, 3)]
    );
    assert_eq!(findings[0].title, CUT);
    assert!(
        findings[0].detail.contains("3.00 mm"),
        "{}",
        findings[0].detail
    );
}

#[test]
fn an_l_shaped_part_across_the_edge_is_reported_once() {
    // On the plate only the upright shows, along column 0; the foot, off
    // the plate, reaches past both the left and the bottom edges.
    let fixture = FixtureBuilder::new(4)
        .model(
            ..,
            Shape::polygon(
                vec![
                    [-30.0, -20.0],
                    [-20.0, -20.0],
                    [-20.0, 10.0],
                    [10.0, 10.0],
                    [10.0, 60.0],
                    [-30.0, 60.0],
                ],
                Vec::new(),
            ),
        )
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![(CheckKind::PlateEdge, Severity::Error, 0, 3)]
    );
    assert_eq!(findings[0].title, CUT);
    assert!(
        findings[0].detail.contains("3.00 mm"),
        "{}",
        findings[0].detail
    );
}

#[test]
fn a_part_whose_top_leaves_the_screen_is_reported_once() {
    // Cut by the left edge on layers 0 to 3; on layers 4 and 5 only a
    // sliver too thin to light a pixel is left on the plate.
    let fixture = FixtureBuilder::new(6)
        .model(.., Shape::rect(100.0, 30.0, 140.0, 60.0))
        .model(0..4, Shape::rect(-20.0, 30.0, 10.0, 60.0))
        .model(4..6, Shape::rect(-20.0, 30.0, 0.3, 60.0))
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![(CheckKind::PlateEdge, Severity::Error, 0, 5)]
    );
    assert_eq!(findings[0].title, CUT);
    assert!(
        findings[0].detail.contains("on 6 layers"),
        "{}",
        findings[0].detail
    );
}

#[test]
fn a_huge_part_across_the_edge_scans_only_rows_near_the_screen() {
    // 2 km tall in raster pixels, as a model in the wrong units would be:
    // only the rows on and near the screen are scanned.
    let fixture = FixtureBuilder::new(3)
        .model(.., Shape::rect(-20.0, -1e7, 10.0, 1e7))
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![(CheckKind::PlateEdge, Severity::Error, 0, 2)]
    );
    assert_eq!(findings[0].title, CUT);
}

#[test]
fn a_part_on_the_last_pixels_is_a_warning() {
    // Wholly on the plate, its contour 0.04 mm inside the edge, but on the
    // screen's first column: nothing is cut, there is no margin left.
    let fixture = FixtureBuilder::new(4)
        .model(.., Shape::rect(0.4, 30.0, 10.0, 40.0))
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![(CheckKind::PlateEdge, Severity::Warning, 0, 3)]
    );
    assert_eq!(findings[0].title, TOUCH);
    assert_eq!(
        findings[0].detail,
        "It lights the outermost pixels of the screen on 4 layers: nothing is cut off, but it has no margin left."
    );
}

#[test]
fn many_parts_on_the_edge_of_one_layer_get_one_finding_each() {
    // Ten blocks along the left edge, all touching it from layer 0.
    let mut builder = FixtureBuilder::new(3);
    for i in 0..10 {
        let y = 2.0 + 12.0 * f64::from(i);
        builder = builder.model(.., Shape::rect(0.0, y, 5.0, y + 8.0));
    }
    let fixture = builder.build();
    let findings = output(&fixture);
    assert_eq!(findings.len(), 10, "{findings:?}");
    for (i, finding) in findings.iter().enumerate() {
        assert_eq!(
            (finding.check, finding.severity),
            (CheckKind::PlateEdge, Severity::Warning)
        );
        // Rows 5 and 6 of each block tie on its centroid; the first wins.
        assert_focus(&fixture, finding, [0, 5 + 12 * i as u32], 0);
    }
}

#[test]
fn contours_on_the_plate_edge_itself_are_not_outside() {
    // The contour runs exactly along the plate's left edge: the solid
    // touches the screen edge, but nothing lies past the plate.
    let fixture = FixtureBuilder::new(4)
        .model(.., Shape::rect(0.0, 30.0, 10.0, 40.0))
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![(CheckKind::PlateEdge, Severity::Warning, 0, 3)]
    );
    assert_eq!(findings[0].title, TOUCH);
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

// ---------------------------------------------------------------------------
// Empty layers.
// ---------------------------------------------------------------------------

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
        "2 empty layers leave a 0.10 mm gap, so everything above it has nothing to rest on. The printer file still lights a pixel on every layer, so the printer does not stall on them."
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
fn an_unlit_layer_at_the_top_is_info() {
    // The last layer has a contour, a tip too small to light a pixel.
    let fixture = FixtureBuilder::new(8)
        .model(0..7, Shape::rect(40.0, 30.0, 80.0, 60.0))
        .model(7..8, Shape::rect(60.2, 44.2, 60.4, 44.4))
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![(CheckKind::EmptyLayer, Severity::Info, 7, 7)]
    );
    assert_eq!(findings[0].title, "Empty layer at the top");
    assert_eq!(
        findings[0].detail,
        "The last layer is empty (0.05 mm): it prints nothing, so the printer file leaves it out."
    );
    assert_focus(&fixture, &findings[0], [60, 44], 7);
}

#[test]
fn a_last_layer_without_contours_is_info() {
    // The model ends below the height the last layer is cut at, as in
    // slices saved before the layer count stopped at the top (#226): that
    // layer prints nothing, and slicing again leaves it out.
    let fixture = FixtureBuilder::new(8)
        .model(0..7, Shape::rect(40.0, 30.0, 80.0, 60.0))
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![(CheckKind::EmptyLayer, Severity::Info, 7, 7)]
    );
    assert_eq!(findings[0].title, "Empty layer at the top");
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
fn a_part_starting_on_the_second_layer_rests_on_the_plate() {
    // Layers 0 and 1 count as the plate (the raft and bottom layers absorb
    // a part that starts one layer up), as in the island check.
    let fixture = FixtureBuilder::new(8)
        .model(.., Shape::rect(20.0, 20.0, 60.0, 50.0))
        .model(1..8, Shape::rect(120.0, 20.0, 140.0, 40.0))
        .build();
    let findings = output(&fixture);
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn an_empty_first_layer_is_reported_under_a_part_on_the_second() {
    let fixture = FixtureBuilder::new(8)
        .model(1..8, Shape::rect(20.0, 20.0, 60.0, 50.0))
        .build();
    assert_eq!(
        summary(&output(&fixture)),
        vec![(CheckKind::EmptyLayer, Severity::Error, 0, 0)]
    );
}

#[test]
fn a_block_joined_by_a_corner_pixel_belongs_to_the_body() {
    // Solid pixels that touch by a corner are one region (8-connectivity),
    // so the block on layers 4 to 7 is joined to the pillar on layer 4 and
    // does not float. How strongly it is held is not this check's question.
    let fixture = FixtureBuilder::new(8)
        .model(.., Shape::rect(20.0, 20.0, 40.0, 40.0))
        .model(4..8, Shape::rect(40.0, 40.0, 60.0, 60.0))
        .build();
    assert!(output(&fixture).is_empty());
}

#[test]
fn layers_only_supports_stand_on_are_not_empty() {
    // A model from layer 5, on supports: nothing is missing below it.
    let fixture = FixtureBuilder::new(12)
        .model(5..12, Shape::rect(20.0, 20.0, 60.0, 50.0))
        .support_layers(0..12)
        .build();
    assert!(output(&fixture).is_empty());
}

#[test]
fn layers_under_the_supports_are_still_empty() {
    let fixture = FixtureBuilder::new(12)
        .model(5..12, Shape::rect(20.0, 20.0, 60.0, 50.0))
        .support_layers(2..12)
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![(CheckKind::EmptyLayer, Severity::Error, 0, 1)]
    );
}

#[test]
fn supports_past_the_plate_are_cut() {
    let fixture = FixtureBuilder::new(4)
        .model(.., Shape::rect(20.0, 20.0, 60.0, 50.0))
        .build();
    let input = fixture
        .input()
        .with_support_extent(0..4, Some([-11.0, -5.0, 5.0, 5.0, 3.0]));
    let found = supports_past_the_plate(&input, &OutputOptions::default()).expect("past the plate");
    assert_eq!(found.check, CheckKind::PlateEdge);
    assert_eq!(found.scope, RoleScope::Support);
    assert!(found.detail.contains("1.00 mm past"), "{}", found.detail);
    let inside = fixture
        .input()
        .with_support_extent(0..4, Some([-9.0, -5.0, 5.0, 5.0, 3.0]));
    assert!(supports_past_the_plate(&inside, &OutputOptions::default()).is_none());
}

#[test]
fn supports_above_the_printer_are_too_tall() {
    let fixture = FixtureBuilder::new(4)
        .model(.., Shape::rect(20.0, 20.0, 60.0, 50.0))
        .build();
    let input = fixture
        .input()
        .with_support_extent(0..4, Some([-5.0, -5.0, 5.0, 5.0, 12.0]));
    let found = supports_past_the_plate(&input, &OutputOptions::default()).expect("too tall");
    assert_eq!(found.check, CheckKind::TooTall);
    assert_eq!(found.scope, RoleScope::Support);
}
