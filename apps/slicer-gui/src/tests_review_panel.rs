/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The Review card's rows and summary, built from findings.

use std::collections::BTreeSet;

use sc_review::{CheckKind, Finding, PlatePoint, RoleScope, Severity, SuggestedFix};

use super::*;

fn finding(
    check: CheckKind,
    severity: Severity,
    first: usize,
    last: usize,
    title: &str,
) -> Finding {
    Finding::new(
        check,
        severity,
        first,
        last,
        PlatePoint::new(1.0, 2.0, 0.05 + first as f64 * 0.1),
        RoleScope::Model,
        title.to_owned(),
        format!("{title} on layer {}.", first + 1),
    )
}

/// A review in listing order (by layer): a warning, an error with a support
/// fix, a note, an error with a drain fix and a second warning.
fn findings() -> Vec<Finding> {
    let at = PlatePoint::new(1.0, 2.0, 3.0);
    vec![
        finding(CheckKind::SuctionCup, Severity::Warning, 2, 9, "Cup"),
        finding(CheckKind::Island, Severity::Error, 10, 24, "Island")
            .with_fix(SuggestedFix::AddSupport { at }),
        finding(CheckKind::EmptyLayer, Severity::Info, 30, 30, "Empty"),
        finding(CheckKind::ResinTrap, Severity::Error, 40, 48, "Trap")
            .with_fix(SuggestedFix::AddDrain { at }),
        finding(CheckKind::ThinFeature, Severity::Warning, 50, 52, "Thin"),
    ]
}

fn titles(rows: &[ReviewRow]) -> Vec<&str> {
    rows.iter().map(|r| r.title.as_str()).collect()
}

#[test]
fn rows_are_grouped_by_severity_in_listing_order() {
    let rows = review_rows(&findings(), &[false; 5], &BTreeSet::new());
    assert_eq!(titles(&rows), ["Island", "Trap", "Cup", "Thin", "Empty"]);
    let ids: Vec<i32> = rows.iter().map(|r| r.id).collect();
    assert_eq!(ids, [1, 3, 0, 4, 2]);
    let severities: Vec<i32> = rows.iter().map(|r| r.severity).collect();
    assert_eq!(severities, [0, 0, 1, 1, 2]);
}

#[test]
fn the_first_row_of_each_group_carries_its_heading() {
    let rows = review_rows(&findings(), &[false; 5], &BTreeSet::new());
    let headers: Vec<&str> = rows.iter().map(|r| r.header.as_str()).collect();
    assert_eq!(headers, ["Errors", "", "Warnings", "", "Notes"]);
}

#[test]
fn rows_name_their_layers_from_one() {
    let rows = review_rows(&findings(), &[false; 5], &BTreeSet::new());
    let layers: Vec<&str> = rows.iter().map(|r| r.layers.as_str()).collect();
    assert_eq!(
        layers,
        [
            "Layers 11–25",
            "Layers 41–49",
            "Layers 3–10",
            "Layers 51–53",
            "Layer 31"
        ]
    );
    assert_eq!(rows[0].detail, "Island on layer 11.");
}

#[test]
fn a_drain_is_offered_only_where_it_can_be_drilled() {
    let support = FixKind::AddSupport as i32;
    let rows = review_rows(&findings(), &[false; 5], &BTreeSet::new());
    let fixes: Vec<i32> = rows.iter().map(|r| r.fix).collect();
    assert_eq!(fixes, [support, FixKind::DrainNeedsHollow as i32, 0, 0, 0]);

    let drainable = [false, false, false, true, false];
    let rows = review_rows(&findings(), &drainable, &BTreeSet::new());
    let fixes: Vec<i32> = rows.iter().map(|r| r.fix).collect();
    assert_eq!(fixes, [support, FixKind::AddDrain as i32, 0, 0, 0]);
}

#[test]
fn an_applied_fix_marks_its_row() {
    let rows = review_rows(&findings(), &[false; 5], &BTreeSet::from([1]));
    let fixed: Vec<bool> = rows.iter().map(|r| r.fixed).collect();
    assert_eq!(fixed, [true, false, false, false, false]);
}

#[test]
fn the_summary_counts_each_severity() {
    assert_eq!(
        review_summary(&findings()),
        "2 errors · 2 warnings · 1 note"
    );
    assert_eq!(review_summary(&findings()[..1]), "1 warning");
    assert_eq!(review_summary(&findings()[1..2]), "1 error");
    assert_eq!(review_summary(&[]), "No problems found");
}

#[test]
fn the_worst_severity_leads() {
    assert_eq!(worst_severity(&findings()), 0);
    assert_eq!(worst_severity(&findings()[..1]), 1);
    assert_eq!(worst_severity(&findings()[2..3]), 2);
    assert_eq!(worst_severity(&[]), -1);
}

#[test]
fn an_empty_review_has_no_rows() {
    assert!(review_rows(&[], &[], &BTreeSet::new()).is_empty());
}

#[test]
fn plate_points_are_world_points() {
    assert_eq!(
        world(PlatePoint::new(8.33, -31.5, 1.05)),
        [8.33, -31.5, 1.05]
    );
}
