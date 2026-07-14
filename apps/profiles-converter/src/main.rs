/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

//! Converts third-party printer and material profiles into the
//! SoulCrafted Slicer TOML format, writing them into a bundled
//! profile-library root. Run on demand to refresh the committed
//! collection; it is not part of the shipped binaries.

use anyhow::Result;

use sc_profiles_converter::cli::cli_parser;
use sc_profiles_converter::request::ConversionRequest;

fn main() -> Result<()> {
    let raw = cli_parser().run();
    let request = ConversionRequest::from_raw(raw)?;
    eprintln!("{}", request.summary());
    Ok(())
}
