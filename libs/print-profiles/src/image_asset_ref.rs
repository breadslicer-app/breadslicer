/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Library-relative references to preview images.
//!
//! [`ImageAssetRef`] is the value stored in a printer profile's
//! `image_asset_path` field: a `/`-separated relative path such as
//! `images/elegoo-saturn-4-goo.png`, resolved against the root of the
//! library the profile was loaded from. Construction refuses empty
//! strings, absolute paths, and any `.` / `..` / empty component, so a
//! stored reference can never point outside its library.

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// A `/`-separated relative path to a preview image inside a profile
/// library. Every component is a plain name.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct ImageAssetRef(String);

/// Returned by [`ImageAssetRef::new`] and its [`TryFrom<String>`] /
/// [`FromStr`] impls when the input cannot address a file inside a
/// library root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NonLibraryRelativeImageRefError {
    /// The input has zero bytes.
    Empty,
    /// The input starts with `/` or `\`, or carries a drive prefix
    /// such as `C:`.
    Absolute,
    /// A component is empty (`a//b`), `.`, or `..`.
    NonNormalComponent,
}

impl fmt::Display for NonLibraryRelativeImageRefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("image reference must not be empty"),
            Self::Absolute => f.write_str("image reference must be a relative path"),
            Self::NonNormalComponent => f.write_str(
                "image reference components must be plain names (no `.`, `..`, or empty segments)",
            ),
        }
    }
}

impl std::error::Error for NonLibraryRelativeImageRefError {}

impl ImageAssetRef {
    /// Construct from a borrowed string. Fails with
    /// [`NonLibraryRelativeImageRefError`] when the input is empty,
    /// absolute, or has a `.` / `..` / empty component.
    pub fn new(value: &str) -> Result<Self, NonLibraryRelativeImageRefError> {
        validate(value)?;
        Ok(Self(value.to_owned()))
    }

    /// Build the reference for a file directly under an images
    /// subdirectory of the library, `<subdir>/<file_name>`. Crate
    /// internal: the caller guarantees both parts are plain names, so
    /// the result needs no validation.
    pub(crate) fn under_subdir(subdir: &str, file_name: &str) -> Self {
        Self(format!("{subdir}/{file_name}"))
    }

    /// The reference as stored in the profile file.
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The on-disk location of the image inside `library_root`.
    pub fn resolve_in(&self, library_root: &Path) -> PathBuf {
        self.0
            .split('/')
            .fold(library_root.to_path_buf(), |path, component| {
                path.join(component)
            })
    }
}

fn validate(value: &str) -> Result<(), NonLibraryRelativeImageRefError> {
    if value.is_empty() {
        return Err(NonLibraryRelativeImageRefError::Empty);
    }
    if value.starts_with('/') || value.starts_with('\\') || has_drive_prefix(value) {
        return Err(NonLibraryRelativeImageRefError::Absolute);
    }
    if value
        .split('/')
        .any(|component| matches!(component, "" | "." | ".."))
    {
        return Err(NonLibraryRelativeImageRefError::NonNormalComponent);
    }
    Ok(())
}

/// `C:` style prefixes, which resolve as absolute on Windows.
fn has_drive_prefix(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(
        (chars.next(), chars.next()),
        (Some(letter), Some(':')) if letter.is_ascii_alphabetic()
    )
}

impl TryFrom<String> for ImageAssetRef {
    type Error = NonLibraryRelativeImageRefError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        validate(&value)?;
        Ok(Self(value))
    }
}

impl From<ImageAssetRef> for String {
    fn from(value: ImageAssetRef) -> Self {
        value.0
    }
}

impl FromStr for ImageAssetRef {
    type Err = NonLibraryRelativeImageRefError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl fmt::Display for ImageAssetRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
