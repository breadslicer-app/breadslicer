/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The vendor copyright block every file carries from format version
//! 4 onwards.
//!
//! The firmware expects the section, so a file without it is not one
//! of these files. It is reproduced exactly, the doubled stop after
//! "Inc" included.

use std::io::{self, Write};

const TEXT: &str = "Layout and record format for the ctb and cbddlp file types are the \
copyrighted programs or codes of CBD Technology (China) Inc..The \
Customer or User shall not in any manner reproduce, distribute, \
modify, decompile, disassemble, decrypt, extract, reverse engineer, \
lease, assign, or sublicense the said programs or codes.";

/// Write the disclaimer section.
///
/// # Errors
///
/// Propagates `out`'s failures.
pub fn write_disclaimer(out: &mut impl Write) -> io::Result<()> {
    out.write_all(TEXT.as_bytes())
}
