/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Identifier strings of a project package: part names and prefixes,
//! relationship types, and content types.

/// Name of the project part inside the package.
pub const PROJECT_PART_NAME: &str = "/Metadata/soulcrafted-project.toml";

/// Name of the slice model part inside the package. A second 3D-model part
/// carrying only the slice stack, standalone from the scene.
pub const SLICE_PART_NAME: &str = "/3D/slices.model";

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
