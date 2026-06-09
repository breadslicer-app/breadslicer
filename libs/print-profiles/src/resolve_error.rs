/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Fatal failure modes the resolver can surface.
//!
//! [`ResolveError`] is the error type returned from the resolver
//! entry point. Two variants today: a constraint rejection when a
//! resolved value falls below a printer-enforced minimum (e.g. a
//! lift speed below the safety floor), and a missing required
//! field when every resolution layer fell through without
//! supplying a value the resolved configuration requires.

use crate::resolved_field::ResolvedField;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ResolveError {
    RejectedBelowMinimum {
        field: ResolvedField,
        value: f64,
        minimum: f64,
    },
    MissingRequiredField {
        field: ResolvedField,
    },
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RejectedBelowMinimum {
                field,
                value,
                minimum,
            } => write!(
                f,
                "field '{field}' value {value} is below the printer minimum {minimum}",
            ),
            Self::MissingRequiredField { field } => write!(
                f,
                "required field '{field}' has no value after all resolution layers",
            ),
        }
    }
}

impl std::error::Error for ResolveError {}
