/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Per-pixel anti-aliasing factor for raster output.
//!
//! [`AaLevel`] enumerates the supported anti-aliasing factors: off
//! (no AA) and 2x / 4x / 8x super-sampling. Parsing via [`FromStr`]
//! accepts numeric (`0`, `2`, `4`, `8`) and canonical (`off`, `x2`,
//! `x4`, `x8`) spellings, case-insensitively. [`UnknownAaLevel`]
//! reports the rejected input.

use std::fmt;
use std::str::FromStr;

/// Per-pixel anti-aliasing factor. `Off` means no anti-aliasing; the
/// `X2` / `X4` / `X8` variants specify the super-sampling factor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AaLevel {
    Off,
    X2,
    X4,
    X8,
}

/// Returned when [`AaLevel::from_str`] receives an unknown name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownAaLevel {
    pub got: String,
}

impl fmt::Display for UnknownAaLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown AA level \"{}\"; expected one of: 0, off, 2, x2, 4, x4, 8, x8",
            self.got,
        )
    }
}

impl std::error::Error for UnknownAaLevel {}

impl FromStr for AaLevel {
    type Err = UnknownAaLevel;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "0" | "off" => Ok(Self::Off),
            "2" | "x2" => Ok(Self::X2),
            "4" | "x4" => Ok(Self::X4),
            "8" | "x8" => Ok(Self::X8),
            _ => Err(UnknownAaLevel { got: s.to_owned() }),
        }
    }
}
