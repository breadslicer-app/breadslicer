/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Error types for project reading and writing.
//!
//! The dispatch-level enums ([`ReadProjectError`], [`WriteProjectError`]) carry
//! 1 variant per supported format, mirroring how `sc-mesh-formats` shapes its
//! loader errors; the per-format enums hold the failure modes of that
//! container.

use std::fmt;

use sc_3mf::error::{MalformedPartNameError, ModelWriteError, PackageReadError, PackageWriteError};
use sc_mesh_formats::error::{LoadThreeMfError, WriteThreeMfError};

use crate::threemf::slice_mapping::SliceMappingError;

/// Returned by [`crate::write_project`].
#[derive(Debug)]
pub enum WriteProjectError {
    ThreeMf(WriteThreeMfProjectError),
    Bread(WriteBreadProjectError),
}

impl fmt::Display for WriteProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ThreeMf(e) => write!(f, "{e}"),
            Self::Bread(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for WriteProjectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ThreeMf(e) => Some(e),
            Self::Bread(e) => Some(e),
        }
    }
}

/// Returned by [`crate::read_project`].
#[derive(Debug)]
pub enum ReadProjectError {
    ThreeMf(ReadThreeMfProjectError),
    Bread(ReadBreadProjectError),
}

impl fmt::Display for ReadProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ThreeMf(e) => write!(f, "{e}"),
            Self::Bread(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ReadProjectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ThreeMf(e) => Some(e),
            Self::Bread(e) => Some(e),
        }
    }
}

/// Failure modes of writing a project as a 3MF package.
#[derive(Debug)]
pub enum WriteThreeMfProjectError {
    /// The scene graph could not be converted to a 3MF model document.
    Scene(WriteThreeMfError),
    /// The model XML could not be serialized.
    Model(ModelWriteError),
    /// The merged slices could not be mapped to a 3MF slice stack.
    SliceMapping(SliceMappingError),
    /// The project payload could not be serialized to TOML.
    Payload(toml_edit::ser::Error),
    /// A part could not be added or the container could not be serialized.
    Package(PackageWriteError),
    /// A part name built from the project payload is not a valid part name. The
    /// id grammars keep these names valid, so only a change of a grammar
    /// produces it.
    MalformedPartName {
        name: String,
        source: MalformedPartNameError,
    },
    /// Writing to the caller's writer failed.
    Io(std::io::Error),
}

impl fmt::Display for WriteThreeMfProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scene(e) => write!(f, "3MF project writer: {e}"),
            Self::Model(e) => write!(f, "3MF project writer: {e}"),
            Self::SliceMapping(e) => write!(f, "3MF project writer: {e}"),
            Self::Payload(e) => write!(
                f,
                "3MF project writer: failed to serialize the project part: {e}",
            ),
            Self::Package(e) => write!(f, "3MF project writer: {e}"),
            Self::MalformedPartName { name, source } => write!(
                f,
                "3MF project writer: \"{name}\" is not a valid part name: {source}",
            ),
            Self::Io(e) => write!(f, "3MF project writer I/O error: {e}"),
        }
    }
}

impl std::error::Error for WriteThreeMfProjectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Scene(e) => Some(e),
            Self::Model(e) => Some(e),
            Self::SliceMapping(e) => Some(e),
            Self::Payload(e) => Some(e),
            Self::Package(e) => Some(e),
            Self::MalformedPartName { source, .. } => Some(source),
            Self::Io(e) => Some(e),
        }
    }
}

impl From<WriteThreeMfError> for WriteThreeMfProjectError {
    fn from(e: WriteThreeMfError) -> Self {
        Self::Scene(e)
    }
}

impl From<ModelWriteError> for WriteThreeMfProjectError {
    fn from(e: ModelWriteError) -> Self {
        Self::Model(e)
    }
}

impl From<SliceMappingError> for WriteThreeMfProjectError {
    fn from(e: SliceMappingError) -> Self {
        Self::SliceMapping(e)
    }
}

impl From<toml_edit::ser::Error> for WriteThreeMfProjectError {
    fn from(e: toml_edit::ser::Error) -> Self {
        Self::Payload(e)
    }
}

impl From<PackageWriteError> for WriteThreeMfProjectError {
    fn from(e: PackageWriteError) -> Self {
        Self::Package(e)
    }
}

impl From<std::io::Error> for WriteThreeMfProjectError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Failure modes of reading a project from a 3MF package.
#[derive(Debug)]
pub enum ReadThreeMfProjectError {
    /// The OPC container could not be read.
    Package(PackageReadError),
    /// The model part could not be loaded as a scene.
    Scene(LoadThreeMfError),
    /// The project part is not UTF-8 encoded.
    PayloadNotUtf8(std::str::Utf8Error),
    /// The project part is not valid TOML of the shape this build understands.
    PayloadParse(toml_edit::de::Error),
}

impl fmt::Display for ReadThreeMfProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Package(e) => write!(f, "3MF project reader: {e}"),
            Self::Scene(e) => write!(f, "3MF project reader: {e}"),
            Self::PayloadNotUtf8(e) => {
                write!(f, "3MF project reader: the project part is not UTF-8: {e}")
            }
            Self::PayloadParse(e) => write!(
                f,
                "3MF project reader: failed to parse the project part: {e}",
            ),
        }
    }
}

impl std::error::Error for ReadThreeMfProjectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Package(e) => Some(e),
            Self::Scene(e) => Some(e),
            Self::PayloadNotUtf8(e) => Some(e),
            Self::PayloadParse(e) => Some(e),
        }
    }
}

impl From<PackageReadError> for ReadThreeMfProjectError {
    fn from(e: PackageReadError) -> Self {
        Self::Package(e)
    }
}

impl From<LoadThreeMfError> for ReadThreeMfProjectError {
    fn from(e: LoadThreeMfError) -> Self {
        Self::Scene(e)
    }
}

/// Failure modes of writing a `.bread` project.
#[derive(Debug)]
pub enum WriteBreadProjectError {
    /// The settings document could not be serialized to TOML.
    Settings(toml_edit::ser::Error),
    /// An embedded profile document is not UTF-8 text, so the settings
    /// document cannot carry it.
    ProfileNotUtf8 { id: String },
    /// The container could not be written.
    Container(sc_bread_format::WriteBreadError),
}

impl fmt::Display for WriteBreadProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Settings(e) => write!(
                f,
                ".bread project writer: failed to serialize the project settings: {e}",
            ),
            Self::ProfileNotUtf8 { id } => write!(
                f,
                ".bread project writer: the embedded profile \"{id}\" is not UTF-8 text",
            ),
            Self::Container(e) => write!(f, ".bread project writer: {e}"),
        }
    }
}

impl std::error::Error for WriteBreadProjectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Settings(e) => Some(e),
            Self::ProfileNotUtf8 { .. } => None,
            Self::Container(e) => Some(e),
        }
    }
}

/// Failure modes of reading a `.bread` project.
#[derive(Debug)]
pub enum ReadBreadProjectError {
    /// The file could not be measured.
    Io(std::io::Error),
    /// The container or one of the chunks this build needs could not be read.
    Container(sc_bread_format::ReadProjectError),
    /// The settings document is not UTF-8 encoded.
    SettingsNotUtf8(std::str::Utf8Error),
    /// The settings document is not valid TOML of the shape this build reads.
    SettingsParse(toml_edit::de::Error),
}

impl fmt::Display for ReadBreadProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, ".bread project reader I/O error: {e}"),
            Self::Container(e) => write!(f, ".bread project reader: {e}"),
            Self::SettingsNotUtf8(e) => write!(
                f,
                ".bread project reader: the project settings are not UTF-8: {e}",
            ),
            Self::SettingsParse(e) => write!(
                f,
                ".bread project reader: failed to parse the project settings: {e}",
            ),
        }
    }
}

impl std::error::Error for ReadBreadProjectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Container(e) => Some(e),
            Self::SettingsNotUtf8(e) => Some(e),
            Self::SettingsParse(e) => Some(e),
        }
    }
}
