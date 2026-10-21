/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! `Display` / `source()` coverage for [`WriteThreeMfError`].

use std::error::Error;

use sc_3mf::error::MalformedPartNameError;

use crate::error::WriteThreeMfError;

#[test]
fn facet_count_exceeds_u32_display_names_the_object_the_count_and_the_cap() {
    let err = WriteThreeMfError::FacetCountExceedsU32 {
        object_id: 7,
        count: 5_000_000_000,
    };
    assert_eq!(
        err.to_string(),
        "3MF writer: object 7 has 5000000000 declared facet normals, more than the 4294967295 \
         a declared-facet-normals part can hold",
    );
}

#[test]
fn facet_count_exceeds_u32_has_no_source() {
    let err = WriteThreeMfError::FacetCountExceedsU32 {
        object_id: 7,
        count: 42,
    };
    assert!(err.source().is_none());
}

#[test]
fn malformed_part_name_display_and_source() {
    let err = WriteThreeMfError::MalformedPartName {
        name: "/Metadata//3.bin".to_string(),
        source: MalformedPartNameError::EmptySegment,
    };
    assert_eq!(
        err.to_string(),
        "3MF writer: \"/Metadata//3.bin\" is not a valid part name: part name contains an empty \
         segment",
    );
    assert_eq!(
        err.source().map(ToString::to_string),
        Some("part name contains an empty segment".to_string()),
    );
}
