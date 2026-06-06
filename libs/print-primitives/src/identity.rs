/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Profile identifier newtypes for printers and materials.
//!
//! [`PrinterId`] and [`MaterialId`] are slug-shaped string newtypes
//! sharing the pattern `^[a-z0-9][a-z0-9-]*$` (lowercase ASCII
//! alphanumerics and hyphens, must start with an alphanumeric). They
//! are distinct types so the compiler refuses a material id where a
//! printer id is expected, even though both wrap a validated slug.
//! Construction failures are reported through [`InvalidProfileId`].

use std::fmt;
use std::str::FromStr;

/// Returned by [`PrinterId::new`], [`MaterialId::new`] and their
/// [`TryFrom<String>`] / [`FromStr`] impls when the supplied slug
/// violates the shared id pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InvalidProfileId {
    Empty,
    LeadingHyphen { value: String },
    InvalidCharAt { value: String, index: usize },
}

impl fmt::Display for InvalidProfileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "profile id must not be empty"),
            Self::LeadingHyphen { value } => {
                write!(f, "profile id {value:?} must not start with a hyphen")
            }
            Self::InvalidCharAt { value, index } => write!(
                f,
                "profile id {value:?} has an invalid character at byte {index}; allowed characters are [a-z0-9-]"
            ),
        }
    }
}

impl std::error::Error for InvalidProfileId {}

fn validate(value: &str) -> Result<(), InvalidProfileId> {
    let bytes = value.as_bytes();
    let Some(&first) = bytes.first() else {
        return Err(InvalidProfileId::Empty);
    };
    if first == b'-' {
        return Err(InvalidProfileId::LeadingHyphen {
            value: value.to_owned(),
        });
    }
    for (i, &b) in bytes.iter().enumerate() {
        if !is_valid_id_byte(b) {
            return Err(InvalidProfileId::InvalidCharAt {
                value: value.to_owned(),
                index: i,
            });
        }
    }
    Ok(())
}

const fn is_valid_id_byte(b: u8) -> bool {
    b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'
}

/// A printer profile identifier (slug).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(try_from = "String", into = "String"))]
pub struct PrinterId(String);

impl PrinterId {
    /// Construct from a borrowed slug. Fails with [`InvalidProfileId`]
    /// when the slug violates the shared id pattern.
    pub fn new(value: &str) -> Result<Self, InvalidProfileId> {
        validate(value)?;
        Ok(Self(value.to_owned()))
    }

    /// The underlying slug as a plain string slice.
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for PrinterId {
    type Error = InvalidProfileId;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        validate(&value)?;
        Ok(Self(value))
    }
}

impl From<PrinterId> for String {
    fn from(id: PrinterId) -> Self {
        id.0
    }
}

impl FromStr for PrinterId {
    type Err = InvalidProfileId;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl fmt::Display for PrinterId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A material profile identifier (slug).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(try_from = "String", into = "String"))]
pub struct MaterialId(String);

impl MaterialId {
    /// Construct from a borrowed slug. Fails with [`InvalidProfileId`]
    /// when the slug violates the shared id pattern.
    pub fn new(value: &str) -> Result<Self, InvalidProfileId> {
        validate(value)?;
        Ok(Self(value.to_owned()))
    }

    /// The underlying slug as a plain string slice.
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for MaterialId {
    type Error = InvalidProfileId;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        validate(&value)?;
        Ok(Self(value))
    }
}

impl From<MaterialId> for String {
    fn from(id: MaterialId) -> Self {
        id.0
    }
}

impl FromStr for MaterialId {
    type Err = InvalidProfileId;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl fmt::Display for MaterialId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
