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
use crate::fill::erase_stack;
use crate::finding::{CheckKind, Finding, RoleScope, Severity, SuggestedFix};
use crate::input::{ReviewLeaf, Role};
use crate::regions::PlateRegions;
use crate::runner::ReviewContext;
use crate::test_support::{Fixture, FixtureBuilder, Shape};
use sc_sl1cer::contour::ContourStack;

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
            (CheckKind::PlateEdge, Severity::Warning, 0, 5),
            (CheckKind::PlateEdge, Severity::Warning, 0, 5),
            (CheckKind::PlateEdge, Severity::Warning, 2, 5),
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
    titled(&findings, CUT);
    let outside = titled(&findings, OUTSIDE);
    // 3 mm past the plate's -X edge at -10 mm.
    assert_near(outside.focus.x_mm, -13.0);
    assert!(outside.detail.contains("3.00 mm"), "{}", outside.detail);
}

#[test]
fn a_part_off_the_plate_is_reported_beside_another_part_cut_by_the_same_edge() {
    // A support cut by the left edge, and 'Figure' 3 to 8 mm left of the
    // plate: two parts, two findings.
    let fixture = FixtureBuilder::new(6)
        .support(.., Shape::rect(-20.0, 10.0, 10.0, 20.0))
        .shape(
            "Figure",
            Role::Model,
            ..,
            Shape::rect(-80.0, 40.0, -30.0, 80.0),
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
    let outside = titled(&findings, OUTSIDE);
    assert!(
        outside
            .detail
            .starts_with("'Figure' reaches up to 8.00 mm past the edge of the plate on 6 layers"),
        "{}",
        outside.detail
    );
    assert_eq!(outside.scope, RoleScope::Model);
    let cut = titled(&findings, CUT);
    assert_eq!(cut.scope, RoleScope::Support);
    assert!(cut.detail.contains("2.00 mm"), "{}", cut.detail);
}

#[test]
fn a_part_off_the_plate_is_reported_beside_a_part_on_the_last_pixels() {
    // The block lies wholly on the plate but lights column 0; 'Stray' lies
    // 2 to 5 mm left of the plate.
    let fixture = FixtureBuilder::new(4)
        .model(.., Shape::rect(0.4, 30.0, 10.0, 40.0))
        .shape(
            "Stray",
            Role::Model,
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
            Role::Model,
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
fn slivers_of_a_part_across_the_edge_belong_to_it() {
    // Mesh slivers beside a support cut by the left edge, on layers 2 and
    // 4: too thin to light a pixel even on the grid past the screen, they
    // join the polygon whose box they meet.
    let fixture = FixtureBuilder::new(6)
        .support(.., Shape::rect(-20.0, 10.0, 10.0, 20.0))
        .support(2..3, Shape::rect(-20.1, 12.0, -20.0, 12.1))
        .support(4..5, Shape::rect(-15.0, 20.0, -14.9, 20.1))
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![(CheckKind::PlateEdge, Severity::Error, 0, 5)]
    );
    assert_eq!(findings[0].title, CUT);
}

#[test]
fn a_supported_part_across_the_edge_is_reported_once() {
    // A model cut by the left edge, and a support that stands on its part
    // past the plate (layers 2 to 4) and leans back onto the screen
    // (layers 5 to 7). The support and the model meet only past the plate,
    // where they are different leaves: one part all the same. On the
    // screen the support starts in mid-air, but what holds it lies beyond
    // the screen, which the cut finding already says.
    let fixture = FixtureBuilder::new(8)
        .model(.., Shape::rect(-50.0, 20.0, 30.0, 40.0))
        .support(2..5, Shape::rect(-30.0, 35.0, -10.0, 50.0))
        .support(5..8, Shape::rect(-15.0, 42.0, 5.0, 55.0))
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![(CheckKind::PlateEdge, Severity::Error, 0, 7)]
    );
    assert_eq!(findings[0].title, CUT);
    assert_eq!(findings[0].scope, RoleScope::Both);
    assert!(
        findings[0]
            .detail
            .contains("5.00 mm past the edge of the plate on 8 layers"),
        "{}",
        findings[0].detail
    );
}

#[test]
fn a_supported_part_off_the_plate_is_one_finding() {
    // 'Figure' lies wholly left of the plate on layers 3 to 7; its support
    // stands on the plate's left edge line 1 mm farther out, and reaches it
    // only past the plate. One part: one finding over both leaves.
    let fixture = FixtureBuilder::new(8)
        .support(0..4, Shape::rect(-60.0, 40.0, -50.0, 50.0))
        .shape(
            "Figure",
            Role::Model,
            3..8,
            Shape::rect(-80.0, 30.0, -30.0, 80.0),
        )
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![(CheckKind::PlateEdge, Severity::Error, 0, 7)]
    );
    let finding = &findings[0];
    assert_eq!(finding.title, OUTSIDE);
    assert_eq!(finding.scope, RoleScope::Both);
    assert!(
        finding.detail.starts_with(
            "'Figure' and 'Supports' reach up to 8.00 mm past the edge of the plate on 8 layers"
        ),
        "{}",
        finding.detail
    );
}

#[test]
fn separate_supports_off_the_plate_are_separate_findings() {
    // Two pillars of the same support leaf, both left of the plate and
    // apart from each other: two parts.
    let fixture = FixtureBuilder::new(4)
        .model(.., Shape::rect(40.0, 30.0, 80.0, 60.0))
        .support(.., Shape::rect(-50.0, 10.0, -40.0, 20.0))
        .support(.., Shape::rect(-50.0, 80.0, -40.0, 90.0))
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![
            (CheckKind::PlateEdge, Severity::Error, 0, 3),
            (CheckKind::PlateEdge, Severity::Error, 0, 3),
        ]
    );
    assert!(findings.iter().all(|f| f.title == OUTSIDE));
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
fn a_support_that_leaves_the_plate_further_down_is_reported_once() {
    // A leaning pillar: wholly off the plate on layers 0 to 3, across the
    // edge on layers 4 to 7, where it joins a block standing on the plate.
    let fixture = FixtureBuilder::new(8)
        .model(.., Shape::rect(5.0, 30.0, 40.0, 40.0))
        .support(0..4, Shape::rect(-40.0, 30.0, -20.0, 40.0))
        .support(4..8, Shape::rect(-25.0, 30.0, 5.0, 40.0))
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![(CheckKind::PlateEdge, Severity::Error, 0, 7)]
    );
    let finding = &findings[0];
    assert_eq!(finding.title, CUT);
    assert_eq!(finding.scope, RoleScope::Both);
    assert_eq!(
        finding.detail,
        "It reaches up to 4.00 mm past the edge of the plate on 8 layers, and nothing beyond the screen is printed."
    );
    // Nothing of it is on the screen on layer 0: the farthest vertex, 4 mm
    // left of the plate.
    assert_near(finding.focus.x_mm, -14.0);
    assert_near(finding.focus.z_mm, fixture.input().layers()[0].sample_mm);
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
        "The last layer is empty (0.05 mm), which adds print time and prints nothing."
    );
    assert_focus(&fixture, &findings[0], [60, 44], 7);
}

#[test]
fn the_last_layer_left_empty_by_the_layer_schedule_is_quiet() {
    // The model ends inside the last slab, below the height it is cut at:
    // the last layer has no contour at all. Every plate whose height is not
    // a whole number of layers ends like this, and nothing can be done
    // about it.
    let fixture = FixtureBuilder::new(8)
        .model(0..7, Shape::rect(40.0, 30.0, 80.0, 60.0))
        .build();
    let findings = output(&fixture);
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn a_plate_that_lights_no_pixel_has_nothing_to_print() {
    // A speck too small to light a pixel, on every layer.
    let fixture = FixtureBuilder::new(20)
        .model(.., Shape::rect(100.2, 60.2, 100.4, 60.4))
        .build();
    assert!(fixture.regions().layers().iter().all(|l| l.is_empty()));
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![(CheckKind::EmptyLayer, Severity::Error, 0, 19)]
    );
    let finding = &findings[0];
    assert_eq!(finding.title, "Nothing to print");
    assert_eq!(finding.scope, RoleScope::Model);
    assert_eq!(
        finding.detail,
        "All 20 layers are empty: the parts on the plate are too small or too thin to light a single pixel, so the printer file prints nothing."
    );
    // The centre of the speck, at the first layer's height.
    assert_near(finding.focus.x_mm, fixture.pixel_to_plate([100.3, 60.3])[0]);
    assert_near(finding.focus.y_mm, fixture.pixel_to_plate([100.3, 60.3])[1]);
    assert_near(finding.focus.z_mm, fixture.input().layers()[0].sample_mm);
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
    // The support goes on its underside, at the bottom of that layer.
    assert_eq!(
        finding.fix,
        Some(SuggestedFix::AddSupport {
            at: fixture.input().underside(4, finding.focus)
        })
    );
}

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
fn a_part_starting_on_the_third_layer_floats() {
    let fixture = FixtureBuilder::new(8)
        .model(.., Shape::rect(20.0, 20.0, 60.0, 50.0))
        .model(2..8, Shape::rect(120.0, 20.0, 140.0, 40.0))
        .build();
    assert_eq!(
        summary(&output(&fixture)),
        vec![(CheckKind::FloatingPart, Severity::Error, 2, 7)]
    );
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
fn a_floating_speck_is_a_warning_to_erase() {
    // One pixel on one layer: 0.0005 mm³, too small for a support to hold.
    let fixture = FixtureBuilder::new(8)
        .model(.., Shape::rect(20.0, 20.0, 60.0, 50.0))
        .model(4..5, Shape::rect(120.0, 20.0, 121.0, 21.0))
        .build();
    let findings = output(&fixture);
    assert_eq!(
        summary(&findings),
        vec![(CheckKind::FloatingPart, Severity::Warning, 4, 4)]
    );
    let finding = &findings[0];
    assert_eq!(finding.title, "Floating speck");
    assert_eq!(
        finding.detail,
        "This 0.000500 mm³ speck starts 0.20 mm above the build plate with nothing below it. It is too small to hold with a support: it may cure as loose debris in the vat, or not at all."
    );
    assert_eq!(
        finding.fix,
        Some(SuggestedFix::Erase {
            at: finding.focus,
            speck: 0
        })
    );
    assert_focus(&fixture, finding, [120, 20], 4);
}

/// The output findings of `fixture` with the erasures they offer.
fn erasures(fixture: &Fixture) -> (Vec<Finding>, Vec<CavityFill>) {
    let input = fixture.input();
    let regions = PlateRegions::compute(&input);
    run_with_erasures(
        &ReviewContext::new(&input, &regions),
        &OutputOptions::default(),
    )
}

#[test]
fn an_erasure_holds_the_speck_and_a_pixel_around_it() {
    // A 2 x 1 pixel speck on layers 4 and 5, a speck under 0.01 mm3.
    let fixture = FixtureBuilder::new(8)
        .model(.., Shape::rect(20.0, 20.0, 60.0, 50.0))
        .model(4..6, Shape::rect(120.0, 20.0, 122.0, 21.0))
        .build();
    let input = fixture.input();
    let regions = PlateRegions::compute(&input);
    let options = OutputOptions {
        speck_volume_mm3: 0.01,
        ..OutputOptions::default()
    };
    let (findings, fills) = run_with_erasures(&ReviewContext::new(&input, &regions), &options);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(fills.len(), 1);
    let fill = &fills[0];
    assert_eq!(fill.slabs.len(), 2);
    for slab in &fill.slabs {
        // 4 x 3 pixels of 0.01 mm²: the speck and its margin.
        let area: f64 = slab.rects.iter().map(|r| r.area_mm2()).sum();
        assert!((area - 0.12).abs() < 1e-9, "{slab:?}");
    }
}

#[test]
fn close_specks_never_erase_a_pixel_twice() {
    // Two one-pixel specks with one air pixel between them: their margins
    // meet on that column, which goes to the first.
    let fixture = FixtureBuilder::new(8)
        .model(.., Shape::rect(20.0, 20.0, 60.0, 50.0))
        .model(4..5, Shape::rect(120.0, 20.0, 121.0, 21.0))
        .model(4..5, Shape::rect(122.0, 20.0, 123.0, 21.0))
        .build();
    let (findings, fills) = erasures(&fixture);
    assert_eq!(findings.len(), 2, "{findings:?}");
    let pixels: Vec<f64> = fills
        .iter()
        .map(|f| f.slabs[0].rects.iter().map(|r| r.area_mm2()).sum::<f64>() / 0.01)
        .collect();
    assert!((pixels[0] - 9.0).abs() < 1e-6, "{pixels:?}");
    assert!((pixels[1] - 6.0).abs() < 1e-6, "{pixels:?}");
    assert!(!fills[0].overlaps(&fills[1]));
}

#[test]
fn an_erased_speck_is_gone_and_its_neighbours_stay() {
    // A round model speck, a support speck two air pixels from the block,
    // and the block itself.
    let fixture = FixtureBuilder::new(8)
        .model(.., Shape::rect(20.0, 20.0, 60.0, 50.0))
        .model(4..5, Shape::circle(120.5, 20.5, 0.6))
        .support(4..5, Shape::rect(62.0, 30.0, 63.0, 31.0))
        .build();
    let (findings, fills) = erasures(&fixture);
    assert_eq!(findings.len(), 2, "{findings:?}");
    let input = fixture.input();
    let stacks = |role: Role| -> Vec<&ContourStack> {
        input
            .leaves()
            .iter()
            .filter(|leaf| leaf.role == role)
            .map(|leaf| leaf.stack)
            .collect()
    };
    let model =
        erase_stack(&fills, &stacks(Role::Model), fixture.schedule()).expect("the model speck");
    let support =
        erase_stack(&fills, &stacks(Role::Support), fixture.schedule()).expect("the support speck");
    let again = fixture.input_with_all([
        ReviewLeaf::new(Some("Erased"), Role::Model, &model),
        ReviewLeaf::new(Some("Erased"), Role::Support, &support),
    ]);
    let regions = PlateRegions::compute(&again);
    let after = run(
        &ReviewContext::new(&again, &regions),
        &OutputOptions::default(),
    );
    assert!(after.is_empty(), "{after:?}");
    // Only the block is left, with every pixel it had.
    let before = PlateRegions::compute(&input);
    for layer in 0..8 {
        assert_eq!(regions.layer(layer).solid.len(), 1, "layer {layer}");
        let block = |r: &PlateRegions| r.layer(layer).solid_area_px();
        assert_eq!(block(&regions), 40 * 30, "layer {layer}");
        assert!(block(&before) >= block(&regions));
    }
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
