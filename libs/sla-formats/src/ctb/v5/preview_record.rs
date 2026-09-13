/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The 32-byte record that locates one preview image.
//!
//! Each record carries its own width and height, so the container
//! fixes neither and a reader must take both from here rather than
//! assume a size.

use std::io::{self, Write};

use crate::ctb::wire::{write_u32, write_zeros};
use crate::preview::PreviewImage;

const RESERVED_BYTES: usize = 16;

/// Write the record for `image`, whose encoded `blob` starts at
/// `blob_offset` in the file.
///
/// # Errors
///
/// Propagates `out`'s failures, and rejects a blob longer than the
/// record's length field can count.
pub fn write_preview_record(
    image: &PreviewImage,
    blob: &[u8],
    blob_offset: u32,
    out: &mut impl Write,
) -> io::Result<()> {
    let blob_length = u32::try_from(blob.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "preview image longer than its length field can count",
        )
    })?;

    write_u32(out, image.width())?;
    write_u32(out, image.height())?;
    write_u32(out, blob_offset)?;
    write_u32(out, blob_length)?;
    write_zeros(out, RESERVED_BYTES)
}
