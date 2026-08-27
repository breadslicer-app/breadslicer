/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 EntoMind Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use crate::staged::Staged;

#[test]
fn provisional_reports_its_value_and_is_not_final() {
    let s = Staged::Provisional(1.5_f64);
    assert_eq!(*s.value(), 1.5);
    assert!(!s.is_final());
}

#[test]
fn final_reports_its_value_and_is_final() {
    let s = Staged::Final(2.5_f64);
    assert_eq!(*s.value(), 2.5);
    assert!(s.is_final());
}

#[test]
fn into_inner_unwraps_either_variant() {
    assert_eq!(Staged::Provisional(3).into_inner(), 3);
    assert_eq!(Staged::Final(4).into_inner(), 4);
}
