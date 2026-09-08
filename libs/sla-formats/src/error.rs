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

use sc_3mf::error::{ModelWriteError, PackageWriteError};
use sc_scene::SceneError;
use sc_scene::node::NodeId;

use crate::format::Format;
use crate::goo::v3_0::file::WriteFailure;
use crate::preview::PreviewSizeMismatch;
use crate::printer_file_format::PrinterFileFormat;

#[derive(Debug)]
pub enum WriterError {
    /// The format's writer has not yet landed. Every variant of
    /// [`Format`] starts here and is replaced by a working writer
    /// in a follow-on commit.
    NotImplemented { format: Format },
    /// The printer format's writer has not yet landed.
    PrinterFormatNotImplemented { format: PrinterFileFormat },
    /// The print program and the contour stack handed to
    /// [`write_printer_file`](crate::write_printer_file) describe
    /// different numbers of layers.
    ProgramLayerCountMismatch {
        program_layers: usize,
        stack_layers: usize,
    },
    /// A preview handed to
    /// [`write_printer_file`](crate::write_printer_file) is not the
    /// size the chosen container fixes.
    PreviewSizeMismatch(PreviewSizeMismatch),
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
    /// The format encodes per-model data that a merged
    /// [`ContourStack`](sc_sl1cer::contour::ContourStack) no longer
    /// carries, so [`write_slice`](crate::write_slice) cannot
    /// produce it; callers must go through
    /// [`write_scene_slices`](crate::write_scene_slices).
    SceneContextRequired { format: Format },
    /// The per-leaf slices list is empty: there is nothing to
    /// package.
    NoModelsToWrite,
    /// One leaf's contour stack does not span the layer schedule's
    /// grid, so layers cannot be paired with their slab tops.
    LayerGridMismatch {
        leaf: NodeId,
        schedule_layers: usize,
        stack_layers: usize,
    },
    /// Baking a leaf's mesh into world space failed.
    SceneBakeFailed(SceneError),
    /// Serializing a 3MF model document failed.
    ModelSerializationFailed(ModelWriteError),
    /// Assembling or compressing the 3MF package failed.
    PackageWriteFailed(PackageWriteError),
}

impl fmt::Display for WriterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotImplemented { format } => {
                write!(f, "writer for format \"{format}\" is not yet implemented")
            }
            Self::PrinterFormatNotImplemented { format } => write!(
                f,
                "writer for printer format \"{format}\" is not yet implemented",
            ),
            Self::ProgramLayerCountMismatch {
                program_layers,
                stack_layers,
            } => write!(
                f,
                "the print program holds {program_layers} layer(s) and the contour stack {stack_layers}",
            ),
            Self::PreviewSizeMismatch(err) => err.fmt(f),
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
            Self::SceneContextRequired { format } => write!(
                f,
                "format \"{format}\" stores per-model slice stacks; it cannot be written from a merged contour stack alone",
            ),
            Self::NoModelsToWrite => {
                write!(f, "the per-leaf slices list is empty; nothing to write")
            }
            Self::LayerGridMismatch {
                leaf,
                schedule_layers,
                stack_layers,
            } => write!(
                f,
                "leaf #{} carries {stack_layers} layer(s) but the layer schedule defines {schedule_layers}",
                leaf.get(),
            ),
            Self::SceneBakeFailed(err) => {
                write!(f, "baking a leaf mesh into world space failed: {err}")
            }
            Self::ModelSerializationFailed(err) => {
                write!(f, "serializing a 3MF model document failed: {err}")
            }
            Self::PackageWriteFailed(err) => {
                write!(f, "writing the 3MF package failed: {err}")
            }
        }
    }
}

impl std::error::Error for WriterError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::SceneBakeFailed(err) => Some(err),
            Self::ModelSerializationFailed(err) => Some(err),
            Self::PackageWriteFailed(err) => Some(err),
            Self::PreviewSizeMismatch(err) => Some(err),
            Self::NotImplemented { .. }
            | Self::PrinterFormatNotImplemented { .. }
            | Self::ProgramLayerCountMismatch { .. }
            | Self::OutputPathInvalid { .. }
            | Self::OutputPathExistsAndIsNotADirectory { .. }
            | Self::OutputPathExistsAndIsNotEmpty { .. }
            | Self::EncodingFailed { .. }
            | Self::SceneContextRequired { .. }
            | Self::NoModelsToWrite
            | Self::LayerGridMismatch { .. } => None,
        }
    }
}

impl From<io::Error> for WriterError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<WriteFailure> for WriterError {
    fn from(err: WriteFailure) -> Self {
        match err {
            WriteFailure::LayerCountMismatch {
                program_layers,
                stack_layers,
            } => Self::ProgramLayerCountMismatch {
                program_layers,
                stack_layers,
            },
            WriteFailure::PreviewSize(err) => Self::PreviewSizeMismatch(err),
            WriteFailure::Io(err) => Self::Io(err),
        }
    }
}

impl From<ModelWriteError> for WriterError {
    fn from(err: ModelWriteError) -> Self {
        Self::ModelSerializationFailed(err)
    }
}

impl From<PackageWriteError> for WriterError {
    fn from(err: PackageWriteError) -> Self {
        Self::PackageWriteFailed(err)
    }
}

#[cfg(test)]
mod writer_error_tests {
    use std::error::Error as _;

    use crate::printer_file_format::{CtbVariant, GooVersion};

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
    fn display_printer_format_not_implemented_names_the_revision() {
        let msg = format!(
            "{}",
            WriterError::PrinterFormatNotImplemented {
                format: PrinterFileFormat::Ctb(CtbVariant::PlainV5),
            },
        );
        assert!(msg.contains("ctb@v5"), "msg: {msg}");
        assert!(msg.contains("not yet"), "msg: {msg}");
    }

    #[test]
    fn display_program_layer_count_mismatch_reports_both_counts() {
        let msg = format!(
            "{}",
            WriterError::ProgramLayerCountMismatch {
                program_layers: 4,
                stack_layers: 7,
            },
        );
        assert!(msg.contains("4 layer(s)"), "msg: {msg}");
        assert!(msg.contains("stack 7"), "msg: {msg}");
    }

    #[test]
    fn a_goo_write_failure_keeps_its_shape() {
        let mismatch = WriteFailure::LayerCountMismatch {
            program_layers: 4,
            stack_layers: 7,
        };
        assert!(matches!(
            WriterError::from(mismatch),
            WriterError::ProgramLayerCountMismatch {
                program_layers: 4,
                stack_layers: 7,
            },
        ));
        assert!(matches!(
            WriterError::from(WriteFailure::Io(io::Error::other("disk full"))),
            WriterError::Io(_),
        ));
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
        assert!(
            WriterError::PrinterFormatNotImplemented {
                format: PrinterFileFormat::Goo(GooVersion::V5_1),
            }
            .source()
            .is_none(),
        );
        assert!(
            WriterError::ProgramLayerCountMismatch {
                program_layers: 1,
                stack_layers: 0,
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

    fn mint_node_id() -> NodeId {
        let mut scene = sc_scene::Scene::new();
        scene.add_group(None, sc_mesh_core::transform::Transform3D::Identity)
    }

    #[test]
    fn display_scene_context_required_names_the_format() {
        let msg = format!(
            "{}",
            WriterError::SceneContextRequired {
                format: Format::ThreeMf,
            },
        );
        assert!(msg.contains("3mf"), "msg: {msg}");
        assert!(msg.contains("merged contour stack"), "msg: {msg}");
    }

    #[test]
    fn display_no_models_to_write_mentions_the_empty_list() {
        let msg = format!("{}", WriterError::NoModelsToWrite);
        assert!(msg.contains("empty"), "msg: {msg}");
    }

    #[test]
    fn display_layer_grid_mismatch_reports_both_counts_and_the_leaf() {
        let leaf = mint_node_id();
        let msg = format!(
            "{}",
            WriterError::LayerGridMismatch {
                leaf,
                schedule_layers: 10,
                stack_layers: 7,
            },
        );
        assert!(msg.contains(&format!("#{}", leaf.get())), "msg: {msg}");
        assert!(msg.contains("7 layer(s)"), "msg: {msg}");
        assert!(msg.contains("defines 10"), "msg: {msg}");
    }

    #[test]
    fn source_returns_inner_for_wrapped_3mf_and_scene_errors() {
        let bake = WriterError::SceneBakeFailed(SceneError::DanglingNodeId { id: mint_node_id() });
        assert!(bake.source().is_some());
    }

    #[test]
    fn source_returns_none_for_new_leaf_variants() {
        assert!(
            WriterError::SceneContextRequired {
                format: Format::ThreeMf,
            }
            .source()
            .is_none(),
        );
        assert!(WriterError::NoModelsToWrite.source().is_none());
        assert!(
            WriterError::LayerGridMismatch {
                leaf: mint_node_id(),
                schedule_layers: 2,
                stack_layers: 1,
            }
            .source()
            .is_none(),
        );
    }
}
