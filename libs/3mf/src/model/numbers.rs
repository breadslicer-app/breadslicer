/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Parsers for attribute values of the spec types `ST_Number` and
//! `ST_ResourceIndex`.

use crate::error::ModelParseError;

use super::resource_ids::ResourceIndex;

/// Returns the `ST_Number` that `value` spells, if it spells one.
pub(super) fn number(value: &str) -> Option<f64> {
    // ST_Number is a decimal pattern with no NaN or infinity spelling; Rust's
    // f64 parser accepts both, so filter them out.
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|number| number.is_finite())
}

pub(super) fn parse_f64(
    element: &'static str,
    attribute: &'static str,
    value: &str,
) -> Result<f64, ModelParseError> {
    number(value).ok_or_else(|| ModelParseError::InvalidNumber {
        element,
        attribute,
        value: value.to_string(),
    })
}

/// Returns the `ST_ResourceIndex` that `value` spells, if it spells one.
pub(super) fn resource_index(value: &str) -> Option<ResourceIndex> {
    value
        .trim()
        .parse::<u32>()
        .ok()
        .and_then(ResourceIndex::new)
}

pub(super) fn parse_index(
    element: &'static str,
    attribute: &'static str,
    value: &str,
) -> Result<ResourceIndex, ModelParseError> {
    resource_index(value).ok_or_else(|| ModelParseError::InvalidNumber {
        element,
        attribute,
        value: value.to_string(),
    })
}
