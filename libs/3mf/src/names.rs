/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Well-known 3MF and OPC identifier strings: XML namespaces,
//! relationship types, and content types.

/// XML namespace of the 3MF core specification (v1.x).
pub const CORE_NS: &str = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02";

/// XML namespace of the 3MF slice extension.
pub const SLICE_NS: &str = "http://schemas.microsoft.com/3dmanufacturing/slice/2015/07";

/// XML namespace of the 3MF production extension. Only detected (to
/// refuse split-part documents cleanly); not implemented.
pub const PRODUCTION_NS: &str = "http://schemas.microsoft.com/3dmanufacturing/production/2015/06";

/// XML namespace of OPC relationship documents.
pub const RELATIONSHIPS_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";

/// XML namespace of the OPC content-types document.
pub const CONTENT_TYPES_NS: &str = "http://schemas.openxmlformats.org/package/2006/content-types";

/// Relationship type marking the primary 3D Model part (StartPart).
pub const START_PART_REL_TYPE: &str =
    "http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel";

/// Relationship type for package / object thumbnails.
pub const THUMBNAIL_REL_TYPE: &str =
    "http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail";

/// Relationship type for custom parts that consumers should retain
/// when rewriting a package.
pub const MUST_PRESERVE_REL_TYPE: &str =
    "http://schemas.openxmlformats.org/package/2006/relationships/mustpreserve";

/// Content type of a 3D Model part.
pub const MODEL_PART_CONTENT_TYPE: &str = "application/vnd.ms-package.3dmanufacturing-3dmodel+xml";

/// Content type of OPC relationship parts.
pub const RELATIONSHIPS_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-package.relationships+xml";

/// Conventional name of the primary 3D Model part.
pub const DEFAULT_MODEL_PART_NAME: &str = "/3D/3dmodel.model";
