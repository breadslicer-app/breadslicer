/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The Elegoo GOO container family.
//!
//! Revisions share the extension, the file magic and the header
//! prefix. Byte order, layer indexing and chunk grammar are each
//! revision's own, so each revision is its own module.

pub mod v3_0;
