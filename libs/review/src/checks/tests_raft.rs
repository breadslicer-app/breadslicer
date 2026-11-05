/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Raft-check tests (#131).
//!
//! The default fake printer: a 200 x 120 pixel screen of 0.1 mm pixels,
//! 0.05 mm layers. Shapes are in raster pixels. A raft top of 0.25 mm makes
//! the first five layers the raft zone; a support lift of 0.4 mm makes the
//! first eight the layers a model in the raft leaves no room under.

use super::*;
use crate::finding::{CheckKind, Finding, RoleScope, Severity, SuggestedFix};
use crate::input::ReviewInput;
use crate::regions::{PlateRegions, SweepOptions};
use crate::runner::{ReviewOptions, check_regions};
use crate::test_support::{Fixture, FixtureBuilder, Shape};

const RAFT_TOP_MM: f64 = 0.25;
const LIFT_MM: f64 = 0.4;

/// `fixture` with its supports on a raft whose top is `raft_top_mm`, and the
/// support lift.
fn rafted(fixture: &Fixture, raft_top_mm: Option<f64>) -> ReviewInput<'_> {
    fixture
        .input()
        .with_raft_top_mm(raft_top_mm)
        .with_support_lift_mm(Some(LIFT_MM))
        .with_support_extent(0..12, None)
}

/// The raft check on `fixture`.
fn raft(fixture: &Fixture, raft_top_mm: Option<f64>) -> Vec<Finding> {
    let input = rafted(fixture, raft_top_mm);
    let regions = PlateRegions::compute(&input, &SweepOptions::default());
    run(
        &ReviewContext::new(&input, &regions),
        &RaftOptions::default(),
    )
}

/// The whole review of `fixture`.
fn review(fixture: &Fixture, raft_top_mm: Option<f64>) -> Vec<Finding> {
    let input = rafted(fixture, raft_top_mm);
    let regions = PlateRegions::compute(&input, &SweepOptions::default());
    check_regions(
        &ReviewContext::new(&input, &regions),
        &ReviewOptions::default(),
    )
    .findings
}

/// A model standing on the plate beside a support on its raft.
fn model_on_the_plate() -> FixtureBuilder {
    FixtureBuilder::new(12).model(0..12, Shape::rect(20.0, 20.0, 60.0, 50.0))
}

#[test]
fn a_model_on_the_plate_is_inside_the_raft() {
    let fixture = model_on_the_plate().build();
    let findings = raft(&fixture, Some(RAFT_TOP_MM));
    assert_eq!(findings.len(), 1, "{findings:?}");
    let finding = &findings[0];
    assert_eq!(finding.check, CheckKind::ModelInRaft);
    assert_eq!(finding.severity, Severity::Error);
    assert_eq!(finding.scope, RoleScope::Model);
    assert_eq!((finding.first_layer, finding.last_layer), (0, 4));
    assert_eq!(finding.title, "Model inside the raft");
    assert!(
        finding.detail.contains("5 layers (0.25 mm)")
            && finding.detail.contains("no room for supports")
            && finding.detail.contains("lift it to 0.40 mm"),
        "{}",
        finding.detail
    );
    let Some(SuggestedFix::ClearRaft { lift_to_mm }) = finding.fix else {
        panic!("no raft fix: {finding:?}");
    };
    assert!((lift_to_mm - LIFT_MM).abs() < 1e-9, "{lift_to_mm}");
    // Centred on the model's footprint (pixels 20..60 x 20..50).
    let [x, y] = fixture.pixel_to_plate([40.0, 35.0]);
    assert!((finding.focus.x_mm - x).abs() < 1e-9);
    assert!((finding.focus.y_mm - y).abs() < 1e-9);
    assert!((finding.focus.z_mm - 0.025).abs() < 1e-9);
}

#[test]
fn without_a_support_lift_the_raft_top_is_recommended() {
    let fixture = model_on_the_plate().build();
    let input = fixture
        .input()
        .with_raft_top_mm(Some(RAFT_TOP_MM))
        .with_support_extent(0..12, None);
    let regions = PlateRegions::compute(&input, &SweepOptions::default());
    let findings = run(
        &ReviewContext::new(&input, &regions),
        &RaftOptions::default(),
    );
    let Some(SuggestedFix::ClearRaft { lift_to_mm }) = findings[0].fix else {
        panic!("no raft fix: {findings:?}");
    };
    assert!((lift_to_mm - 0.25).abs() < 1e-9, "{lift_to_mm}");
}

#[test]
fn a_model_partly_lifted_is_still_in_the_raft() {
    let fixture = FixtureBuilder::new(12)
        .model(3..12, Shape::rect(20.0, 20.0, 60.0, 50.0))
        .build();
    let findings = raft(&fixture, Some(RAFT_TOP_MM));
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!((findings[0].first_layer, findings[0].last_layer), (3, 4));
    assert!(matches!(
        findings[0].fix,
        Some(SuggestedFix::ClearRaft { lift_to_mm }) if (lift_to_mm - LIFT_MM).abs() < 1e-9
    ));
}

#[test]
fn a_model_lifted_above_the_raft_is_clear() {
    // Above the raft, yet below the support lift: not this check's concern.
    let fixture = FixtureBuilder::new(12)
        .model(5..12, Shape::rect(20.0, 20.0, 60.0, 50.0))
        .build();
    assert!(raft(&fixture, Some(RAFT_TOP_MM)).is_empty());
}

#[test]
fn without_a_raft_nothing_is_reported() {
    let fixture = model_on_the_plate().build();
    assert!(raft(&fixture, None).is_empty());
}

#[test]
fn every_model_in_the_raft_is_reported() {
    let fixture = model_on_the_plate()
        .shape("Second", 0..12, Shape::rect(20.0, 70.0, 50.0, 100.0))
        .shape("Lifted", 6..12, Shape::rect(170.0, 70.0, 190.0, 100.0))
        .build();
    let findings = raft(&fixture, Some(RAFT_TOP_MM));
    assert_eq!(findings.len(), 2, "{findings:?}");
}

/// The model on the plate with a piece of it that starts in mid-air on
/// layer `start`, beside it.
fn with_overhang(start: usize) -> Fixture {
    model_on_the_plate()
        .model(start..12, Shape::rect(64.0, 20.0, 80.0, 40.0))
        .build()
}

fn islands(findings: &[Finding]) -> Vec<(usize, usize)> {
    findings
        .iter()
        .filter(|f| f.check == CheckKind::Island)
        .map(|f| (f.first_layer, f.last_layer))
        .collect()
}

fn raft_findings(findings: &[Finding]) -> usize {
    findings
        .iter()
        .filter(|f| f.check == CheckKind::ModelInRaft)
        .count()
}

#[test]
fn islands_inside_the_raft_zone_are_covered_by_the_raft_finding() {
    let fixture = with_overhang(3);
    let without = review(&fixture, None);
    assert_eq!(islands(&without), [(3, 11)], "{without:?}");
    assert_eq!(raft_findings(&without), 0);

    let with = review(&fixture, Some(RAFT_TOP_MM));
    assert!(islands(&with).is_empty(), "{with:?}");
    assert_eq!(raft_findings(&with), 1, "{with:?}");
}

#[test]
fn islands_above_the_raft_but_below_the_support_lift_are_covered() {
    let fixture = with_overhang(6);
    let without = review(&fixture, None);
    assert_eq!(islands(&without), [(6, 11)], "{without:?}");
    let with = review(&fixture, Some(RAFT_TOP_MM));
    assert!(islands(&with).is_empty(), "{with:?}");
    assert_eq!(raft_findings(&with), 1, "{with:?}");
}

#[test]
fn islands_above_the_support_lift_are_still_reported() {
    let fixture = with_overhang(9);
    let with = review(&fixture, Some(RAFT_TOP_MM));
    assert_eq!(islands(&with), islands(&review(&fixture, None)), "{with:?}");
    assert!(!islands(&with).is_empty(), "{with:?}");
}

#[test]
fn islands_of_a_model_clear_of_the_raft_are_still_reported() {
    // The model stands above the raft: no raft finding, so nothing is
    // folded, even below the support lift.
    let fixture = FixtureBuilder::new(12)
        .model(5..12, Shape::rect(20.0, 20.0, 60.0, 50.0))
        .model(6..12, Shape::rect(64.0, 20.0, 80.0, 40.0))
        .build();
    let with = review(&fixture, Some(RAFT_TOP_MM));
    assert_eq!(raft_findings(&with), 0, "{with:?}");
    assert_eq!(islands(&with), islands(&review(&fixture, None)));
    assert!(!islands(&with).is_empty(), "{with:?}");
}

#[test]
fn only_islands_near_the_footprint_and_below_the_lift_are_covered() {
    let fixture = model_on_the_plate().build();
    let input = rafted(&fixture, Some(RAFT_TOP_MM));
    let regions = PlateRegions::compute(&input, &SweepOptions::default());
    let clashes = clashes(
        &ReviewContext::new(&input, &regions),
        &RaftOptions::default(),
    );
    assert_eq!(clashes.len(), 1);
    assert_eq!(clashes[0].cover_layers, 8);
    let [x0, y0, x1, y1] = clashes[0].footprint_mm;
    let island = |layer: usize, x: f64, y: f64, scope: RoleScope| {
        Finding::new(
            CheckKind::Island,
            Severity::Error,
            layer,
            layer + 2,
            PlatePoint::new(x, y, 0.0),
            scope,
            "Unsupported island",
            "Detail.",
        )
    };
    let (cx, cy) = ((x0 + x1) * 0.5, (y0 + y1) * 0.5);
    assert!(covers(&clashes, &island(3, cx, cy, RoleScope::Model), 1.0));
    assert!(covers(&clashes, &island(7, cx, cy, RoleScope::Model), 1.0));
    assert!(covers(
        &clashes,
        &island(4, x1 + 0.5, cy, RoleScope::Both),
        1.0
    ));
    // The supports crammed under the model are its doing too.
    assert!(covers(
        &clashes,
        &island(3, cx, cy, RoleScope::Support),
        1.0
    ));
    assert!(!covers(&clashes, &island(8, cx, cy, RoleScope::Model), 1.0));
    assert!(!covers(
        &clashes,
        &island(3, x1 + 3.0, cy, RoleScope::Model),
        1.0
    ));
    let mut not_island = island(3, cx, cy, RoleScope::Model);
    not_island.check = CheckKind::ResinTrap;
    assert!(!covers(&clashes, &not_island, 1.0));
}
