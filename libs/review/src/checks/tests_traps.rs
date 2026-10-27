/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Resin-trap and suction-cup tests (#83).

use super::*;
use crate::test_support::{FixtureBuilder, Shape};

#[test]
fn a_solid_block_has_no_trap() {
    // 40 x 30 pixels (4 x 3 mm) on every layer of an 8-layer plate.
    let fixture = FixtureBuilder::new(8)
        .model(.., Shape::rect(40.0, 30.0, 80.0, 60.0))
        .build();
    let findings = fixture.check(|ctx| run(ctx, &TrapOptions::default()));
    assert!(findings.is_empty(), "{findings:?}");
}
