/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Identifiers of a project package: part names and prefixes, relationship
//! types, and content types.

use sc_3mf::package::static_part_name::StaticPartName;

/// Name of the project part inside the package.
pub const PROJECT_PART_NAME: StaticPartName =
    match StaticPartName::new("/Metadata/soulcrafted-project.toml") {
        Ok(name) => name,
        Err(_) => panic!("malformed part name"),
    };

/// Name of the slice model part inside the package. A second 3D-model part
/// carrying only the slice stack, standalone from the scene.
pub const SLICE_PART_NAME: StaticPartName = match StaticPartName::new("/3D/slices.model") {
    Ok(name) => name,
    Err(_) => panic!("malformed part name"),
};

/// How the name of a numbered slice part starts: `/3D/slices-1.model` and on.
/// Slices too large for 1 part go there, and [`SLICE_PART_NAME`] then holds a
/// slice stack of references to them.
pub(super) const SLICE_PART_NUMBERED_PREFIX: &str = "/3D/slices-";

/// How the name of a numbered slice part ends.
pub(super) const SLICE_PART_NUMBERED_SUFFIX: &str = ".model";

/// Relationship type linking the package root to the slice part. An opaque OPC
/// identifier following the schemas.<domain> shape of the core 3MF relationship
/// types; it is not meant to resolve.
pub const SLICE_REL_TYPE: &str = "http://schemas.soulcrafted.it/slicer/2026/07/slices";

/// Content type of the project part.
pub const PROJECT_PART_CONTENT_TYPE: &str = "application/vnd.soulcrafted.project+toml";

/// Relationship type linking the package root to the project part. An opaque
/// OPC identifier following the schemas.<domain> shape of the core 3MF
/// relationship types; it is not meant to resolve.
pub const PROJECT_REL_TYPE: &str = "http://schemas.soulcrafted.it/slicer/2026/07/project";

/// Content type of embedded profile parts.
pub const PROFILE_PART_CONTENT_TYPE: &str = "application/toml";

/// Directory prefix of every embedded profile part.
pub const PROFILES_PART_PREFIX: &str = "/Metadata/profiles/";

/// Directory prefix of embedded printer profile parts.
pub const PRINTER_PROFILE_PART_PREFIX: &str = "/Metadata/profiles/printers/";

/// Directory prefix of embedded material profile parts.
pub const MATERIAL_PROFILE_PART_PREFIX: &str = "/Metadata/profiles/materials/";

/// Directory prefix of embedded material extension parts.
pub const MATERIAL_EXTENSION_PART_PREFIX: &str = "/Metadata/profiles/materials/extensions/";
