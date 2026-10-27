/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

use sc_print_primitives::{AaLevel, Mirror};
use sc_sla_formats::{
    AnycubicZipVariant, CxdlpVersion, GooVersion, PrinterFileFormat, write_printer_file,
};

use crate::read::{read_anycubic_zip, read_cxdlp};
use crate::test_fixture::{program, raster, stack};

const PORTED: [PrinterFileFormat; 5] = [
    PrinterFileFormat::Cxdlp(CxdlpVersion::V2),
    PrinterFileFormat::Cxdlp(CxdlpVersion::V3),
    PrinterFileFormat::AnycubicZip(AnycubicZipVariant::Pwsz),
    PrinterFileFormat::AnycubicZip(AnycubicZipVariant::Pm7),
    PrinterFileFormat::AnycubicZip(AnycubicZipVariant::Pm7m),
];

#[test]
fn installing_makes_the_ported_formats_writable_with_their_previews() {
    crate::install();
    crate::install();
    for format in PORTED {
        assert!(format.has_writer(), "{format}");
        assert!(format.preview_sizes().is_some(), "{format}");
    }
    assert!(!PrinterFileFormat::Goo(GooVersion::V5_1).has_writer());
}

#[test]
fn the_dispatcher_writes_every_ported_format_to_disk() {
    crate::install();
    let dir = tempfile::tempdir().unwrap();
    for format in PORTED {
        let machine = match format {
            PrinterFileFormat::Cxdlp(_) => "Halot One",
            _ => "Photon Mono M7 Pro",
        };
        let program = program(machine, raster(AaLevel::X4, Mirror::default()), 4);
        let path = dir.path().join(format!("plate.{}", format.extension()));
        write_printer_file(&stack(4), &path, format, &program, None).unwrap();

        let file = std::fs::File::open(&path).unwrap();
        match format {
            PrinterFileFormat::Cxdlp(_) => {
                let read = read_cxdlp(&std::fs::read(&path).unwrap()).unwrap();
                assert!(read.checksum_matches);
                assert_eq!(read.layers.len(), 4);
            }
            _ => {
                let read = read_anycubic_zip(file).unwrap();
                assert_eq!(read.layers.len(), 4);
            }
        }
    }
}
