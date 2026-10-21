/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Strings that carry the "not empty" invariant.
//!
//! [`NonEmptyString`] wraps a `String` and refuses construction when
//! the input has zero bytes. Whitespace-only inputs (e.g. `" "`) pass;
//! if a stricter "no leading/trailing whitespace" semantic is needed
//! later, it lives in a separate primitive.

use std::fmt;
use std::str::FromStr;

/// A `String` that is guaranteed to contain at least one byte.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(try_from = "String", into = "String"))]
pub struct NonEmptyString(String);

/// Returned by [`NonEmptyString::new`] and its [`TryFrom<String>`] /
/// [`FromStr`] impls when the input is the empty string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EmptyStringError;

impl fmt::Display for EmptyStringError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("value must not be empty")
    }
}

impl std::error::Error for EmptyStringError {}

impl NonEmptyString {
    /// Construct from a borrowed string slice. Fails with
    /// [`EmptyStringError`] when the input has zero bytes.
    pub fn new(value: &str) -> Result<Self, EmptyStringError> {
        if value.is_empty() {
            Err(EmptyStringError)
        } else {
            Ok(Self(value.to_owned()))
        }
    }

    /// The underlying string as a plain slice.
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for NonEmptyString {
    type Error = EmptyStringError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty() {
            Err(EmptyStringError)
        } else {
            Ok(Self(value))
        }
    }
}

impl From<NonEmptyString> for String {
    fn from(value: NonEmptyString) -> Self {
        value.0
    }
}

impl FromStr for NonEmptyString {
    type Err = EmptyStringError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl fmt::Display for NonEmptyString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
