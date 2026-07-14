/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The converter's in-flight profile representation.
//!
//! A source-format reader produces [`ConvertedProfile`] values and the
//! writer consumes them. Each is one of our own domain profile types,
//! so by the time a reader hands one over every invariant it carries
//! (envelope agreement, non-empty strings, slug grammar, ...) has
//! already been enforced by constructing the domain type.

use sc_print_profiles::{MaterialProfile, PrinterProfile};

/// A single profile a reader has converted into our domain model.
#[derive(Clone, Debug, PartialEq)]
pub enum ConvertedProfile {
    Printer(PrinterProfile),
    Material(MaterialProfile),
}
