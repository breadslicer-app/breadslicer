/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

use sc_print_primitives::{AaLevel, Mirror};
use sc_sla_formats::{
    AnycubicBinaryVariant, AnycubicZipVariant, CxdlpVersion, GooVersion, PrinterFileFormat,
    write_printer_file,
};

use crate::read::{read_anycubic_binary, read_anycubic_zip, read_cxdlp};
use crate::test_fixture::{program, raster, stack};

fn ported() -> Vec<PrinterFileFormat> {
    let mut formats = vec![
        PrinterFileFormat::Cxdlp(CxdlpVersion::V2),
        PrinterFileFormat::Cxdlp(CxdlpVersion::V3),
    ];
    formats.extend(AnycubicZipVariant::ALL.map(PrinterFileFormat::AnycubicZip));
    formats.extend(AnycubicBinaryVariant::ALL.map(PrinterFileFormat::AnycubicBinary));
    formats
}

#[test]
fn installing_makes_the_ported_formats_writable_with_their_previews() {
    crate::install();
    crate::install();
    for format in ported() {
        assert!(format.has_writer(), "{format}");
        assert!(format.preview_sizes().is_some(), "{format}");
    }
    assert!(!PrinterFileFormat::Goo(GooVersion::V5_1).has_writer());
}

#[test]
fn the_dispatcher_writes_every_ported_format_to_disk() {
    crate::install();
    let dir = tempfile::tempdir().unwrap();
    for format in ported() {
        let machine = match format {
            PrinterFileFormat::Cxdlp(_) => "Halot One",
            _ => "Photon Mono M7 Pro",
        };
        let program = program(machine, raster(AaLevel::X4, Mirror::default()), 4);
        let path = dir.path().join(format!("plate.{}", format.extension()));
        write_printer_file(&stack(4), &path, format, &program, None).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        let layers = match format {
            PrinterFileFormat::Cxdlp(_) => {
                let read = read_cxdlp(&bytes).unwrap();
                assert!(read.checksum_matches);
                read.layers.len()
            }
            PrinterFileFormat::AnycubicZip(_) => read_anycubic_zip(std::io::Cursor::new(bytes))
                .unwrap()
                .layers
                .len(),
            PrinterFileFormat::AnycubicBinary(variant) => {
                read_anycubic_binary(&bytes, variant).unwrap().layers.len()
            }
            other => unreachable!("{other} is not ported"),
        };
        assert_eq!(layers, 4, "{format}");
    }
}
