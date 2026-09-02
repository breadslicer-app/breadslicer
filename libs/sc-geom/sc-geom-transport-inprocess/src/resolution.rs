/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 EntoMind Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Constraint-to-actual resolution trail, following the pattern of
//! `sc_print_profiles::resolve()`'s `warnings`: every specs/settings field an
//! algorithm resolves gets one [`ResolutionRecord`] naming where the
//! final value came from, in resolution order. A field the caller
//! specified outright, one that fell back to a default, one derived
//! from something else, and one a hard/soft constraint moved are all
//! different [`FieldSource`]s, so the driver can render "why did I get
//! this" without re-deriving it.

/// Where a resolved field's final value came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldSource {
    /// The caller supplied this value directly and nothing changed it.
    Requested,
    /// No input specified this value; an algorithm- or crate-level
    /// default was used.
    Default,
    /// The value was computed from other inputs (e.g. an
    /// auto-detected anchor point) rather than being a stored default.
    Derived,
    /// A constraint moved the value away from what was requested or
    /// derived. `hard` distinguishes a constraint that cannot be
    /// relaxed (the value is now the constrained value, full stop)
    /// from one that was a preference the algorithm honoured when it
    /// could.
    ConstraintClamped { hard: bool },
}

/// One entry in a job's resolution trail: which field, where its
/// final value came from, and (when it changed) what it was before.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolutionRecord {
    pub field: &'static str,
    pub source: FieldSource,
    pub prior: Option<String>,
    pub final_value: String,
}

impl ResolutionRecord {
    pub fn new(field: &'static str, source: FieldSource, final_value: impl Into<String>) -> Self {
        Self {
            field,
            source,
            prior: None,
            final_value: final_value.into(),
        }
    }

    pub fn with_prior(mut self, prior: impl Into<String>) -> Self {
        self.prior = Some(prior.into());
        self
    }
}
