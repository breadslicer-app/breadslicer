/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 EntoMind Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use crate::resolution::{FieldSource, ResolutionRecord};

#[test]
fn new_record_has_no_prior_value() {
    let r = ResolutionRecord::new("radius_mm", FieldSource::Requested, "1.5");
    assert_eq!(r.field, "radius_mm");
    assert_eq!(r.source, FieldSource::Requested);
    assert_eq!(r.prior, None);
    assert_eq!(r.final_value, "1.5");
}

#[test]
fn with_prior_records_the_value_it_replaced() {
    let r = ResolutionRecord::new(
        "radius_mm",
        FieldSource::ConstraintClamped { hard: true },
        "2.0",
    )
    .with_prior("5.0");
    assert_eq!(r.prior, Some("5.0".to_string()));
    assert_eq!(r.final_value, "2.0");
}
