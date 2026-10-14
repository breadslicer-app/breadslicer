/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Parsers for attribute values of the spec types `ST_Number` and
//! `ST_ResourceIndex`.

use crate::error::ModelParseError;

use super::resource_ids::ResourceIndex;

pub(super) fn parse_f64(
    element: &'static str,
    attribute: &'static str,
    value: &str,
) -> Result<f64, ModelParseError> {
    // ST_Number is a decimal pattern with no NaN or infinity spelling; Rust's
    // f64 parser accepts both, so filter them out.
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|number| number.is_finite())
        .ok_or_else(|| ModelParseError::InvalidNumber {
            element,
            attribute,
            value: value.to_string(),
        })
}

pub(super) fn parse_index(
    element: &'static str,
    attribute: &'static str,
    value: &str,
) -> Result<ResourceIndex, ModelParseError> {
    value
        .trim()
        .parse::<u32>()
        .ok()
        .and_then(ResourceIndex::new)
        .ok_or_else(|| ModelParseError::InvalidNumber {
            element,
            attribute,
            value: value.to_string(),
        })
}
