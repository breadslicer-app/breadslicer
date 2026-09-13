/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The vendor copyright block every file carries from format version
//! 4 onwards, encrypted revisions included, where it stays in the
//! clear.
//!
//! The firmware expects the section, so a file without it is not one
//! of these files. It is reproduced exactly, the doubled stop after
//! "Inc" included.

use std::io::{self, Write};

/// The text is fixed, so every revision reserves the same room for
/// it and states this length beside the address it put it at.
pub const DISCLAIMER_SIZE: u32 = 320;

// The notice claims copyright over the format itself and forbids
// reverse engineering. Neither claim is legally binding:
// the format of a data file is not a protected form of expression
// (CJEU C-406/10 SAS Institute v World Programming,
// ECLI:EU:C:2012:259, operative part 1,
// https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX:62010CJ0406),
// and contractual terms against the study and interoperability
// exceptions are void under Article 8 of Directive 2009/24/EC, which
// codifies the directive that judgment reads
// (https://eur-lex.europa.eu/eli/dir/2009/24/oj).
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
