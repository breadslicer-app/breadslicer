/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The converter's in-flight profile representation.
//!
//! A source-format reader produces [`ConvertedProfile`] values and the
//! writer consumes them. Each carries one of our own domain profile
//! types, so by the time a reader hands one over every invariant it
//! carries (envelope agreement, non-empty strings, slug grammar, ...)
//! has already been enforced by constructing the domain type.

use std::path::PathBuf;

use sc_print_profiles::{MaterialProfile, PrinterProfile};

/// A converted printer: its domain profile plus, optionally, the source
/// image to copy into the library. `profile.identity.image_asset_path`
/// is left unset here; the writer fills it in when (and only when) it
/// copies the image, so the profile never carries a source path.
#[derive(Clone, Debug, PartialEq)]
pub struct ConvertedPrinter {
    pub profile: PrinterProfile,
    pub image_source: Option<PathBuf>,
}

/// A single profile a reader has converted into our domain model.
#[derive(Clone, Debug, PartialEq)]
pub enum ConvertedProfile {
    Printer(ConvertedPrinter),
    Material(MaterialProfile),
}
