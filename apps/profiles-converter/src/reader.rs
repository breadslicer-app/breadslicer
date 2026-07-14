/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The source-format port.
//!
//! [`ProfileReader`] is the seam every source format plugs into: given
//! the input paths, it produces the [`ConvertedProfile`] values they
//! contain. Each source format is an adapter implementing this trait;
//! the binary picks one via `--format` and hands the results to the
//! writer. The core downstream of this port is format-agnostic.

use std::path::Path;

use crate::profile::ConvertedProfile;

/// A reader for one source profile format.
pub trait ProfileReader {
    /// Everything that can go wrong turning this format's inputs into
    /// domain profiles.
    type Error: std::error::Error + 'static;

    /// Read every input path and return the profiles they contain. A
    /// path may be a single file or a directory the reader walks.
    fn read(&self, inputs: &[&Path]) -> Result<Vec<ConvertedProfile>, Self::Error>;
}
