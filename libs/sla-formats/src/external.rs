/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Printer-file writers that live outside this crate.
//!
//! Some formats are encoded by code under another license than this crate's,
//! which therefore cannot depend on it. Such a writer crate depends on this one
//! instead and installs itself at start-up with
//! [`install_printer_file_writer`]: from then on
//! [`PrinterFileFormat::has_writer`] reports its formats writable, their
//! preview sizes are its own, and
//! [`write_printer_file`](crate::write_printer_file) hands it the file to
//! write. Without the install those formats are named and refused, as before.

use std::io::{Seek, Write};
use std::sync::RwLock;

use sc_sl1cer::contour::ContourStack;

use crate::error::WriteFailure;
use crate::preview::{PreviewSizes, PrintPreviews};
use crate::print_program::PrintProgram;
use crate::printer_file_format::PrinterFileFormat;

/// An output a printer file can be written to: a stream that can also go back,
/// which an archive needs to patch its directory.
pub trait PrinterFileSink: Write + Seek {}

impl<T: Write + Seek> PrinterFileSink for T {}

/// A writer for printer-file formats this crate names but does not encode.
pub trait PrinterFileWriter: Send + Sync {
    /// Reports whether this writer encodes `format`.
    fn writes(&self, format: PrinterFileFormat) -> bool;

    /// Reports the sizes `format` carries its 2 previews at, as
    /// [`PrinterFileFormat::preview_sizes`] does for the built-in formats.
    fn preview_sizes(&self, format: PrinterFileFormat) -> Option<PreviewSizes>;

    /// Writes `stack` as a file of `format` running the schedule in `program`.
    ///
    /// The caller has already checked that the program and the stack hold the
    /// same number of layers.
    fn write(
        &self,
        format: PrinterFileFormat,
        program: &PrintProgram,
        previews: Option<&PrintPreviews>,
        stack: &ContourStack,
        out: &mut dyn PrinterFileSink,
    ) -> Result<(), WriteFailure>;
}

static WRITERS: RwLock<Vec<&'static dyn PrinterFileWriter>> = RwLock::new(Vec::new());

/// Installs `writer` for the formats it reports writing, for the rest of the
/// process. Installing the same writer again changes nothing. A format this
/// crate encodes itself keeps its own writer.
pub fn install_printer_file_writer(writer: &'static dyn PrinterFileWriter) {
    let mut writers = WRITERS
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !writers
        .iter()
        .any(|installed| std::ptr::addr_eq(*installed, writer))
    {
        writers.push(writer);
    }
}

/// Finds the installed writer for `format`, the first installed when several
/// write it.
pub(crate) fn writer_for(format: PrinterFileFormat) -> Option<&'static dyn PrinterFileWriter> {
    let writers = WRITERS
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    writers.iter().copied().find(|writer| writer.writes(format))
}
