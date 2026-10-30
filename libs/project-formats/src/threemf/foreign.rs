/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Detection of other slicers' project data inside a 3MF package.
//!
//! Each known dialect is recognized by the part name(s) it always
//! writes. Detection is the whole feature: none of the foreign data
//! is parsed, the reader only reports that it exists (and is about
//! to be ignored). Real extractors for specific dialects would slot
//! in next to the payload extraction in the reader.

use sc_3mf::package::Package;

use crate::messages::ForeignDialect;

/// Part name PrusaSlicer writes its resolved config dump to.
pub(crate) const PRUSA_MARKER_PART: &str = "/Metadata/Slic3r_PE.config";

/// Part name Bambu Studio and OrcaSlicer write their project
/// settings to.
pub(crate) const BAMBU_ORCA_MARKER_PART: &str = "/Metadata/project_settings.config";

/// Directory prefix Cura project files keep their container stacks
/// under.
pub(crate) const CURA_FOLDER_PREFIX: &str = "/Cura/";

/// Returns the foreign dialects whose marker parts appear in the
/// package, each reported once, in part order. Parts that reading skipped for
/// having no content type count, after the others: Bambu Studio and Orca store
/// their marker part that way.
pub(crate) fn detect_foreign_dialects(package: &Package) -> Vec<ForeignDialect> {
    let mut dialects = Vec::new();
    for name in package.part_names().chain(package.untyped_parts()) {
        let detected = match name.as_str() {
            PRUSA_MARKER_PART => Some(ForeignDialect::PrusaSlicer),
            BAMBU_ORCA_MARKER_PART => Some(ForeignDialect::BambuStudioOrca),
            other if other.starts_with(CURA_FOLDER_PREFIX) => Some(ForeignDialect::Cura),
            _ => None,
        };
        if let Some(dialect) = detected
            && !dialects.contains(&dialect)
        {
            dialects.push(dialect);
        }
    }
    dialects
}
