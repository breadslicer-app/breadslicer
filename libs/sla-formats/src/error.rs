/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Typed errors for the public writer API.
//!
//! [`WriterError`] aggregates every failure mode of
//! [`write_slice`](crate::write_slice): an output path that does
//! not match the format's directory-vs-file expectation, a writer
//! that has not yet landed, or an I/O failure under the hood.

use std::fmt;
use std::io;
use std::path::PathBuf;

use crate::format::Format;

#[derive(Debug)]
pub enum WriterError {
    /// The format's writer has not yet landed. Every variant of
    /// [`Format`] starts here and is replaced by a working writer
    /// in a follow-on commit.
    NotImplemented { format: Format },
    /// An underlying filesystem operation failed.
    Io(io::Error),
    /// The output path is unsuitable for the chosen format and the
    /// mismatch does not fit a more specific variant.
    OutputPathInvalid { reason: String },
    /// A directory-target format was pointed at an existing path
    /// that is not a directory.
    OutputPathExistsAndIsNotADirectory { path: PathBuf },
    /// A directory-target format was pointed at an existing
    /// directory that already contains files; we refuse to clobber.
    OutputPathExistsAndIsNotEmpty { path: PathBuf },
    /// The format's encoder rejected our request for reasons other
    /// than I/O (bad parameter, malformed text chunk, ...).
    EncodingFailed { format: Format, reason: String },
}

impl fmt::Display for WriterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotImplemented { format } => {
                write!(f, "writer for format \"{format}\" is not yet implemented")
            }
            Self::Io(err) => write!(f, "filesystem operation failed: {err}"),
            Self::OutputPathInvalid { reason } => {
                write!(f, "output path is invalid: {reason}")
            }
            Self::OutputPathExistsAndIsNotADirectory { path } => write!(
                f,
                "output path \"{}\" exists and is not a directory",
                path.display(),
            ),
            Self::OutputPathExistsAndIsNotEmpty { path } => write!(
                f,
                "output directory \"{}\" exists and is not empty",
                path.display(),
            ),
            Self::EncodingFailed { format, reason } => write!(
                f,
                "encoder for format \"{format}\" rejected the request: {reason}",
            ),
        }
    }
}

impl std::error::Error for WriterError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::NotImplemented { .. }
            | Self::OutputPathInvalid { .. }
            | Self::OutputPathExistsAndIsNotADirectory { .. }
            | Self::OutputPathExistsAndIsNotEmpty { .. }
            | Self::EncodingFailed { .. } => None,
        }
    }
}

impl From<io::Error> for WriterError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

#[cfg(test)]
mod writer_error_tests {
    use std::error::Error as _;

    use super::*;

    #[test]
    fn display_not_implemented_names_the_format() {
        let msg = format!(
            "{}",
            WriterError::NotImplemented {
                format: Format::SvgPerLayer,
            },
        );
        assert!(msg.contains("svg-per-layer"), "msg: {msg}");
        assert!(msg.contains("not yet"), "msg: {msg}");
    }

    #[test]
    fn display_io_renders_inner_message() {
        let io_err = io::Error::other("disk full");
        let msg = format!("{}", WriterError::Io(io_err));
        assert!(msg.contains("disk full"), "msg: {msg}");
    }

    #[test]
    fn display_output_path_invalid_renders_reason() {
        let msg = format!(
            "{}",
            WriterError::OutputPathInvalid {
                reason: "expected a regular file path".into(),
            },
        );
        assert!(msg.contains("regular file path"), "msg: {msg}");
    }

    #[test]
    fn display_output_path_not_a_directory_renders_path() {
        let msg = format!(
            "{}",
            WriterError::OutputPathExistsAndIsNotADirectory {
                path: PathBuf::from("/tmp/file.svg"),
            },
        );
        assert!(msg.contains("/tmp/file.svg"), "msg: {msg}");
        assert!(msg.contains("not a directory"), "msg: {msg}");
    }

    #[test]
    fn display_output_path_not_empty_renders_path() {
        let msg = format!(
            "{}",
            WriterError::OutputPathExistsAndIsNotEmpty {
                path: PathBuf::from("/tmp/already_there"),
            },
        );
        assert!(msg.contains("/tmp/already_there"), "msg: {msg}");
        assert!(msg.contains("not empty"), "msg: {msg}");
    }

    #[test]
    fn from_io_error_routes_to_io_variant() {
        let io_err = io::Error::other("nope");
        let err: WriterError = io_err.into();
        assert!(matches!(err, WriterError::Io(_)));
    }

    #[test]
    fn source_returns_inner_for_io() {
        let err = WriterError::Io(io::Error::other("nope"));
        assert!(err.source().is_some());
    }

    #[test]
    fn source_returns_none_for_leaf_variants() {
        assert!(
            WriterError::NotImplemented {
                format: Format::SvgCombined,
            }
            .source()
            .is_none(),
        );
        assert!(
            WriterError::OutputPathInvalid { reason: "x".into() }
                .source()
                .is_none(),
        );
        assert!(
            WriterError::OutputPathExistsAndIsNotADirectory {
                path: PathBuf::from("/x"),
            }
            .source()
            .is_none(),
        );
        assert!(
            WriterError::OutputPathExistsAndIsNotEmpty {
                path: PathBuf::from("/x"),
            }
            .source()
            .is_none(),
        );
        assert!(
            WriterError::EncodingFailed {
                format: Format::PngPerLayer,
                reason: "x".into(),
            }
            .source()
            .is_none(),
        );
    }

    #[test]
    fn display_encoding_failed_names_the_format_and_reason() {
        let msg = format!(
            "{}",
            WriterError::EncodingFailed {
                format: Format::PngPerLayer,
                reason: "image data length mismatch".into(),
            },
        );
        assert!(msg.contains("png-per-layer"), "msg: {msg}");
        assert!(msg.contains("image data length mismatch"), "msg: {msg}");
    }
}
