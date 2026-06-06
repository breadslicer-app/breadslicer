/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Per-pixel anti-aliasing factor for raster output.
//!
//! [`AaLevel`] enumerates the supported anti-aliasing factors: off
//! (no AA) and 2x / 4x / 8x super-sampling. The canonical lowercase
//! spellings `off`, `x2`, `x4`, `x8` are the only accepted forms for
//! [`FromStr`] and (under the `serde` feature) TOML serialisation.
//! [`UnknownAaLevel`] reports the rejected input.

use std::fmt;
use std::str::FromStr;

/// Per-pixel anti-aliasing factor. `Off` means no anti-aliasing; the
/// `X2` / `X4` / `X8` variants specify the super-sampling factor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
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
            "unknown AA level \"{}\"; expected one of: off, x2, x4, x8",
            self.got,
        )
    }
}

impl std::error::Error for UnknownAaLevel {}

impl FromStr for AaLevel {
    type Err = UnknownAaLevel;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "off" => Ok(Self::Off),
            "x2" => Ok(Self::X2),
            "x4" => Ok(Self::X4),
            "x8" => Ok(Self::X8),
            _ => Err(UnknownAaLevel { got: s.to_owned() }),
        }
    }
}
