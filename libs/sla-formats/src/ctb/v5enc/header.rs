/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The 48 plaintext bytes the file opens with.
//!
//! They say nothing about the print. What they carry is where the
//! settings block is and how long it is, so a machine can decrypt
//! it, and where the signature sits, so it can check that whoever
//! wrote the file held the key.

use std::io::{self, Write};

use super::layout::{Layout, SETTINGS_SIZE, SIGNATURE_SIZE};
use crate::ctb::wire::{write_u16, write_u32};

/// Opens every encrypted file of this family. The plain revisions
/// carry their own.
const MAGIC: u32 = 0x12FD_0107;

/// Which layout the rest of the file follows. The encrypted
/// revisions differ from each other by more than their fixed
/// blocks, so a reader branches on this.
const VERSION: u32 = 5;

/// Three slots nobody has explained, the same in every file read.
const UNIDENTIFIED_PAIR: u16 = 1;
const UNIDENTIFIED_WORD: u32 = 42;

/// Write the plaintext header, stating where `layout` put the
/// settings block and where the layers left room for the signature.
///
/// # Errors
///
/// Propagates `out`'s failures.
pub fn write_header(
    layout: &Layout,
    signature_offset: u32,
    out: &mut impl Write,
) -> io::Result<()> {
    write_u32(out, MAGIC)?;
    write_u32(out, SETTINGS_SIZE)?;
    write_u32(out, layout.settings)?;
    write_u32(out, 0)?;
    write_u32(out, VERSION)?;
    write_u32(out, SIGNATURE_SIZE)?;
    write_u32(out, signature_offset)?;
    write_u32(out, 0)?;
    write_u16(out, UNIDENTIFIED_PAIR)?;
    write_u16(out, UNIDENTIFIED_PAIR)?;
    write_u32(out, 0)?;
    write_u32(out, UNIDENTIFIED_WORD)?;
    write_u32(out, 0)
}
