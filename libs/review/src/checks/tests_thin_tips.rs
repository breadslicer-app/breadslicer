/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Thin-tip tests (#724).

use crate::checks::thin_tips;
use crate::finding::{CheckKind, Severity};
use crate::test_support::{FixtureBuilder, Shape};

/// A slab from layer 10 up, on tips of `radius_px` pixels.
fn slab_on(radius_px: f64) -> Vec<crate::finding::Finding> {
    FixtureBuilder::new(20)
        .model(10..20, Shape::rect(20.0, 10.0, 180.0, 110.0))
        .tip(50.0, 60.0, 10, radius_px)
        .tip(100.0, 60.0, 10, radius_px)
        .tip(150.0, 60.0, 10, 3.0)
        .build()
        .check(thin_tips::run)
}

#[test]
fn tips_under_three_pixels_across_are_told() {
    let findings = slab_on(1.0);
    assert_eq!(findings.len(), 1, "{findings:?}");
    let finding = &findings[0];
    assert_eq!(finding.check, CheckKind::ThinTip);
    assert_eq!(finding.severity, Severity::Warning);
    assert!(finding.detail.starts_with("2 tips"), "{}", finding.detail);
}

#[test]
fn tips_of_three_pixels_and_more_are_not() {
    assert!(slab_on(1.5).is_empty());
}
