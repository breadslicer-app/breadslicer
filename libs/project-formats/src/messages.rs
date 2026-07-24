/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Typed warnings surfaced by project-file readers and by the
//! profile-reference classification. Plain data, like the warning
//! enums in `sc-mesh-formats`: presentation (and translation) is the
//! consuming application's job.

use sc_mesh_formats::messages::LoadThreeMfWarning;
use sc_print_primitives::{MaterialId, PrinterId};

/// Non-fatal observations from reading a project file. The scene
/// always arrives intact; these describe data the file carries that
/// is dropped, ignored, or not understood.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadProjectWarning {
    /// A warning from the mesh-level 3MF reader (dropped materials,
    /// ignored slice data, and so on).
    ThreeMf(LoadThreeMfWarning),
    /// The project part declares a payload version this build does
    /// not understand. The scene loads normally; the project data is
    /// ignored, and a re-save will not carry it.
    ProjectVersionUnsupported { version: i64 },
    /// A part under the embedded-profiles prefix does not match the
    /// expected naming, so it is ignored; a re-save will not carry
    /// it.
    UnrecognizedProfilePart { part_name: String },
    /// The package carries project data written by another slicer.
    /// It is detected so the user knows it is there, but never
    /// parsed, and a re-save will not carry it.
    ForeignProjectData { dialect: ForeignDialect },
}

/// Slicer families whose project data our 3MF reader recognizes by
/// marker part names. Detection only: none of their data is parsed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForeignDialect {
    PrusaSlicer,
    BambuStudioOrca,
    Cura,
}

/// Result of classifying a project's profile references against the
/// locally installed profiles (see
/// [`crate::ProjectPayload::profile_ref_warnings`]).
///
/// The `Missing` variants mean the reference resolves nowhere: not
/// installed and no embedded copy in the file. The `EmbeddedOnly`
/// variants are softer: not installed, but the file carries a copy
/// of the document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfileRefWarning {
    PrinterMissing { id: PrinterId },
    PrinterEmbeddedOnly { id: PrinterId },
    ResinMissing { id: MaterialId },
    ResinEmbeddedOnly { id: MaterialId },
}
