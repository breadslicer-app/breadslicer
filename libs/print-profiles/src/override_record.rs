/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Diagnostic record attributing a resolved field's final value to
//! the resolver pipeline layer that supplied it.
//!
//! Collected into a vector on the resolved configuration: one
//! record per emission point. `prior_value` is the value an earlier
//! layer had set for the same field (if any), letting downstream
//! tooling reconstruct the full provenance chain. Both values are
//! stored as `f64` regardless of the field's typed unit; the
//! [`ResolvedField`] tag identifies the unit.

use crate::resolved_field::ResolvedField;
use crate::value_source::ValueSource;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverrideRecord {
    pub field: ResolvedField,
    pub source: ValueSource,
    pub prior_value: Option<f64>,
    pub final_value: f64,
}

impl std::fmt::Display for OverrideRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.prior_value {
            Some(prior) => write!(
                f,
                "{}: {} ({}, was {})",
                self.field, self.final_value, self.source, prior,
            ),
            None => write!(f, "{}: {} ({})", self.field, self.final_value, self.source,),
        }
    }
}
