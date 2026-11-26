/*
 * SPDX-FileCopyrightText: 2020-2026 PTRTECH, Tiago Conceição (sn4k3) and UVtools contributors
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 *
 * Part of the Rust port of UVtools file formats for Breadslicer. The formats
 * written here are ported from UVtools (https://github.com/sn4k3/UVtools),
 * v7.1.1, commit 8b5c91fd41b60bd6265ba1c1f2b2b64636b873e8, and modified for
 * Breadslicer; each module names the UVtools file it ports. See NOTICE.
 *
 * This program is free software: you can redistribute it and/or modify it
 * under the terms of the GNU Affero General Public License as published by the
 * Free Software Foundation, either version 3 of the License, or (at your
 * option) any later version. It is distributed WITHOUT ANY WARRANTY; see the
 * GNU Affero General Public License (LICENSE.md) for details.
 */

//! Printer-file writers ported from UVtools.
//!
//! `sc-sla-formats` names these formats and stays free of their code, since it
//! is published under a different license. This crate encodes them and plugs
//! into it at run time: call [`install`] once at start-up, and from then on
//! [`sc_sla_formats::PrinterFileFormat::has_writer`] reports them writable and
//! [`sc_sla_formats::write_printer_file`] writes them.
//!
//! - [`cxdlp`]: Creality CXDLP, versions 2 and 3 (Halot One, One Pro, One
//!   Plus, ...).
//! - [`anycubic_zip`]: the Anycubic ZIP family (`.pwsz` for the Photon Mono M7
//!   Pro, `.pm7` for the M7, `.pm7m` for the M7 Max, `.pm4u`, `.pp1`,
//!   `.pp1m`).
//! - [`anycubic_binary`]: the Anycubic binary family (`.pwmx`, `.pwma`, `.pm3`,
//!   `.pws` and the rest Photon Workshop wrote before the ZIP).
//! - [`ctb_legacy`]: ChiTuBox CTB versions 2 to 4 (version 5 is written by
//!   `sc-sla-formats` itself), and version 4 for the UniFormation GKtwo
//!   (`.gktwo.ctb`).
//! - [`sl1`]: Prusa's SL1 archive (`.sl1` for the SL1, `.sl1s` for the SL1S).
//! - [`cxdlpv4`]: Creality CXDLP version 4 (Halot Mage, Mage S, Play, R6 and
//!   the Halot X1 family).
//!
//! [`read`] decodes them back, for tests and for checking a written file.

pub mod anycubic_binary;
pub mod anycubic_zip;
pub mod ctb_legacy;
pub mod cxdlp;
pub mod cxdlpv4;
mod layer_stats;
mod program;
mod pw0;
mod pws;
pub mod read;
mod resample;
pub mod sl1;

use sc_sl1cer::contour::ContourStack;
use sc_sla_formats::{
    CtbVariant, OtherFormat, PreviewSizes, PrintPreviews, PrintProgram, PrinterFileFormat,
    PrinterFileSink, PrinterFileWriter, RasterSetup, WriteFailure,
};

/// The writer this crate installs.
#[derive(Debug)]
struct UvtoolsWriters;

static WRITERS: UvtoolsWriters = UvtoolsWriters;

impl PrinterFileWriter for UvtoolsWriters {
    fn writes(&self, format: PrinterFileFormat) -> bool {
        matches!(
            format,
            PrinterFileFormat::Ctb(CtbVariant::V2 | CtbVariant::V3 | CtbVariant::V4)
                | PrinterFileFormat::Cxdlp(_)
                | PrinterFileFormat::AnycubicZip(_)
                | PrinterFileFormat::AnycubicBinary(_)
                | PrinterFileFormat::Sl1(_)
                | PrinterFileFormat::Other(OtherFormat::Cxdlpv4 | OtherFormat::GktwoCtb)
        )
    }

    fn preview_sizes(&self, format: PrinterFileFormat) -> Option<PreviewSizes> {
        match format {
            PrinterFileFormat::Ctb(CtbVariant::V2 | CtbVariant::V3 | CtbVariant::V4)
            | PrinterFileFormat::Other(OtherFormat::GktwoCtb) => {
                Some(sc_sla_formats::ctb::PREVIEW_SIZES)
            }
            PrinterFileFormat::Cxdlp(_) => Some(cxdlp::PREVIEW_SIZES),
            PrinterFileFormat::AnycubicZip(_) => Some(anycubic_zip::PREVIEW_SIZES),
            PrinterFileFormat::AnycubicBinary(_) => Some(anycubic_binary::PREVIEW_SIZES),
            PrinterFileFormat::Sl1(_) => Some(sl1::PREVIEW_SIZES),
            PrinterFileFormat::Other(OtherFormat::Cxdlpv4) => Some(cxdlpv4::PREVIEW_SIZES),
            _ => None,
        }
    }

    fn stored_grey(&self, format: PrinterFileFormat, raster: &RasterSetup, grey: u8) -> u8 {
        match format {
            PrinterFileFormat::Ctb(CtbVariant::V2 | CtbVariant::V3 | CtbVariant::V4)
            | PrinterFileFormat::Other(OtherFormat::GktwoCtb) => {
                sc_sla_formats::ctb::rle7::stored_grey(grey)
            }
            PrinterFileFormat::AnycubicZip(_) => pw0::stored_grey(grey),
            PrinterFileFormat::AnycubicBinary(variant) => {
                if anycubic_binary::uses_bit_planes(variant) {
                    pws::quantise(grey, raster.aa().factor())
                } else {
                    pw0::stored_grey(grey)
                }
            }
            PrinterFileFormat::Other(OtherFormat::Cxdlpv4) => {
                let resolution = raster.panel_resolution();
                if cxdlpv4::is_pixel_pair_panel((resolution.x(), resolution.y())) {
                    cxdlpv4::pixel_pair_grey(grey)
                } else {
                    sc_sla_formats::ctb::rle7::stored_grey(grey)
                }
            }
            _ => grey,
        }
    }

    fn write(
        &self,
        format: PrinterFileFormat,
        program: &PrintProgram,
        previews: Option<&PrintPreviews>,
        stack: &ContourStack,
        out: &mut dyn PrinterFileSink,
    ) -> Result<(), WriteFailure> {
        match format {
            PrinterFileFormat::Ctb(
                variant @ (CtbVariant::V2 | CtbVariant::V3 | CtbVariant::V4),
            ) => ctb_legacy::write_file(variant, program, previews, stack, out),
            PrinterFileFormat::Cxdlp(version) => {
                cxdlp::write_file(version, program, previews, stack, out)
            }
            PrinterFileFormat::AnycubicZip(variant) => {
                anycubic_zip::write_file(variant, program, previews, stack, out)
            }
            PrinterFileFormat::AnycubicBinary(variant) => {
                anycubic_binary::write_file(variant, program, previews, stack, out)
            }
            PrinterFileFormat::Sl1(variant) => {
                sl1::write_file(variant, program, previews, stack, out)
            }
            PrinterFileFormat::Other(OtherFormat::Cxdlpv4) => {
                cxdlpv4::write_file(program, previews, stack, out)
            }
            PrinterFileFormat::Other(OtherFormat::GktwoCtb) => {
                ctb_legacy::write_gktwo(program, previews, stack, out)
            }
            other => Err(WriteFailure::Unencodable(format!(
                "\"{other}\" is not a format ported from UVtools"
            ))),
        }
    }
}

/// Installs this crate's writers into `sc-sla-formats`, for the rest of the
/// process. Calling it again changes nothing.
pub fn install() {
    sc_sla_formats::install_printer_file_writer(&WRITERS);
}

#[cfg(test)]
mod test_fixture;
#[cfg(test)]
mod tests_install;
