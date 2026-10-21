/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::value_source::ValueSource;

#[test]
fn as_str_matches_label_per_variant() {
    assert_eq!(ValueSource::Calibrated.as_str(), "calibrated");
    assert_eq!(ValueSource::Default.as_str(), "default");
    assert_eq!(ValueSource::Derived.as_str(), "derived");
    assert_eq!(ValueSource::Override.as_str(), "override");
}

#[test]
fn display_matches_as_str() {
    assert_eq!(format!("{}", ValueSource::Calibrated), "calibrated");
    assert_eq!(format!("{}", ValueSource::Override), "override");
}

#[test]
fn equality_is_structural() {
    assert_eq!(ValueSource::Calibrated, ValueSource::Calibrated);
    assert_ne!(ValueSource::Calibrated, ValueSource::Default);
}

#[test]
fn copy_does_not_move() {
    let s = ValueSource::Derived;
    let _a = s;
    let _b = s;
    assert_eq!(s.as_str(), "derived");
}
