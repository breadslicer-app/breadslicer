/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The `.bread` project file (#145).
//!
//! A `.bread` file is a short header followed by chunks. Each chunk has a
//! four-byte tag, its own version, and a payload compressed with zstd and
//! guarded by a CRC-32. A reader skips the tags it does not know, so a newer
//! file still opens in an older build as far as that build understands it,
//! and a damaged chunk is reported instead of read.
//!
//! [`container`] reads and writes chunks without knowing what they hold; the
//! other modules are the codecs of the payloads, starting with the sliced
//! plate ([`slices`]).

pub mod container;
pub mod slices;

#[cfg(test)]
mod tests_container;
#[cfg(test)]
mod tests_slices;

pub use container::{BreadReader, BreadWriter, Chunk, ChunkTag, ReadBreadError};
pub use slices::{DecodeSlicesError, SLICES_TAG, SLICES_VERSION, decode_slices, encode_slices};
