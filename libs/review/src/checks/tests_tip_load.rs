/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Tip-load tests (#129).

use crate::finding::{CheckKind, Severity, SuggestedFix};
use crate::test_support::{FixtureBuilder, Shape};

/// A 16 x 10 mm slab from layer 10 up, held only by the tips `tips` adds.
fn slab_on(tips: impl FnOnce(FixtureBuilder) -> FixtureBuilder) -> FixtureBuilder {
    tips(FixtureBuilder::new(20).model(10..20, Shape::rect(20.0, 10.0, 180.0, 110.0)))
}

#[test]
fn a_wide_part_on_one_thin_tip_is_an_error() {
    let fixture = slab_on(|b| b.tip(100.0, 60.0, 10, 1.5)).build();
    let report = fixture.report();
    let finding = report
        .findings
        .iter()
        .find(|f| f.check == CheckKind::TipLoad)
        .unwrap_or_else(|| panic!("no tip load in {:?}", report.findings));
    assert_eq!(finding.severity, Severity::Error);
    assert_eq!(finding.first_layer, 10);
    assert!(
        finding.detail.contains("not yet calibrated"),
        "{}",
        finding.detail
    );
    let Some(SuggestedFix::AddSupport { at }) = finding.fix else {
        panic!("no tip offered: {finding:?}");
    };
    // On the part's first surface, at the bottom of its first layer.
    assert!((at.z_mm - 0.5).abs() < 1e-9, "{at:?}");
}

#[test]
fn enough_tips_hold_it_quietly() {
    let fixture = slab_on(|mut b| {
        for x in [40.0, 80.0, 120.0, 160.0] {
            for y in [25.0, 60.0, 95.0] {
                b = b.tip(x, y, 10, 3.0);
            }
        }
        b
    })
    .build();
    let report = fixture.report();
    assert!(
        report
            .findings
            .iter()
            .all(|f| f.check != CheckKind::TipLoad),
        "{:?}",
        report.findings
    );
    assert_eq!(report.loads.tips.len(), 12);
}

#[test]
fn a_part_on_the_plate_needs_no_tip() {
    let fixture = FixtureBuilder::new(10)
        .model(0..10, Shape::rect(20.0, 10.0, 180.0, 110.0))
        .build();
    let report = fixture.report();
    assert!(
        report
            .findings
            .iter()
            .all(|f| f.check != CheckKind::TipLoad)
    );
}

#[test]
fn a_cup_says_what_it_pulls_with() {
    // A cup closed toward the plate, on the plate.
    let fixture = FixtureBuilder::new(8)
        .model(0..2, Shape::rect(40.0, 30.0, 100.0, 80.0))
        .model(2..8, Shape::frame(40.0, 30.0, 100.0, 80.0, 5.0))
        .build();
    let report = fixture.report();
    let cup = report
        .findings
        .iter()
        .find(|f| f.check == CheckKind::SuctionCup)
        .unwrap_or_else(|| panic!("no cup in {:?}", report.findings));
    assert!(cup.detail.contains(" N on the film"), "{}", cup.detail);
    // The plate holds it: its area decides, 20 mm² is a warning.
    assert_eq!(cup.severity, Severity::Warning);
}

#[test]
fn a_cup_on_overloaded_tips_is_an_error() {
    // The same cup hanging from one thin tip.
    let fixture = FixtureBuilder::new(14)
        .model(6..8, Shape::rect(40.0, 30.0, 100.0, 80.0))
        .model(8..14, Shape::frame(40.0, 30.0, 100.0, 80.0, 5.0))
        .tip(70.0, 55.0, 6, 1.5)
        .build();
    let report = fixture.report();
    let cup = report
        .findings
        .iter()
        .find(|f| f.check == CheckKind::SuctionCup)
        .unwrap_or_else(|| panic!("no cup in {:?}", report.findings));
    assert_eq!(cup.severity, Severity::Error, "{}", cup.detail);
    assert!(
        cup.detail.contains("times what they hold"),
        "{}",
        cup.detail
    );
}
