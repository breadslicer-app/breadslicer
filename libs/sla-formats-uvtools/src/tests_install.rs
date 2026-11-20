/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

use sc_print_primitives::{AaLevel, Mirror};
use sc_sla_formats::{
    AnycubicBinaryVariant, AnycubicZipVariant, CtbVariant, CxdlpVersion, GooVersion,
    PrinterFileFormat, Sl1Variant, write_printer_file,
};

use crate::read::{read_anycubic_binary, read_anycubic_zip, read_ctb, read_cxdlp, read_sl1};
use crate::test_fixture::{program, ramp_exposure, ramped_program, raster, stack};

fn ported() -> Vec<PrinterFileFormat> {
    let mut formats = vec![
        PrinterFileFormat::Ctb(CtbVariant::V2),
        PrinterFileFormat::Ctb(CtbVariant::V3),
        PrinterFileFormat::Ctb(CtbVariant::V4),
        PrinterFileFormat::Cxdlp(CxdlpVersion::V2),
        PrinterFileFormat::Cxdlp(CxdlpVersion::V3),
    ];
    formats.extend(AnycubicZipVariant::ALL.map(PrinterFileFormat::AnycubicZip));
    formats.extend(AnycubicBinaryVariant::ALL.map(PrinterFileFormat::AnycubicBinary));
    formats.extend(Sl1Variant::ALL.map(PrinterFileFormat::Sl1));
    formats
}

/// #96: every format that keeps the transition ramp reads it back as the
/// slicer made it; the ones that cannot say so.
#[test]
fn the_exposure_ramp_reads_back_from_every_format_that_keeps_it() {
    crate::install();
    let dir = tempfile::tempdir().unwrap();
    let layers = 8;
    let near = |got: f32, want: f64| (f64::from(got) - want).abs() < 0.011;
    for format in ported() {
        let machine = match format {
            PrinterFileFormat::Cxdlp(_) => "Halot One",
            _ => "Photon Mono M7 Pro",
        };
        let program = ramped_program(machine, raster(AaLevel::Off, Mirror::default()), layers);
        let path = dir.path().join(format!("ramp.{}", format.extension()));
        write_printer_file(&stack(layers), &path, format, &program, None).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let keeps = format.keeps_exposure_ramp();
        match format {
            PrinterFileFormat::Cxdlp(_) => {
                assert!(!keeps, "{format}");
                let read = read_cxdlp(&bytes).unwrap();
                assert_eq!(read.bottom_exposure_sec, 35, "{format}");
                assert_eq!(read.exposure_tenths, 25, "{format}");
            }
            PrinterFileFormat::Ctb(variant) => {
                assert_eq!(keeps, variant != CtbVariant::V2, "{format}");
                let read = read_ctb(&bytes).unwrap();
                for (index, layer) in read.layer_definitions.iter().enumerate() {
                    assert!(
                        near(layer.exposure_sec, ramp_exposure(index)),
                        "{format} layer {index}: {}",
                        layer.exposure_sec
                    );
                }
            }
            PrinterFileFormat::AnycubicZip(_) => {
                assert!(keeps, "{format}");
                let read = read_anycubic_zip(std::io::Cursor::new(bytes)).unwrap();
                let text = read.settings.to_string();
                assert!(
                    text.contains("\"transition_layercount\":3"),
                    "{format}: {text}"
                );
            }
            PrinterFileFormat::AnycubicBinary(variant) => {
                assert!(keeps, "{format}");
                let read = read_anycubic_binary(&bytes, variant).unwrap();
                for (index, layer) in read.layer_definitions.iter().enumerate() {
                    assert!(
                        near(layer.exposure_sec, ramp_exposure(index)),
                        "{format} layer {index}: {}",
                        layer.exposure_sec
                    );
                }
            }
            PrinterFileFormat::Sl1(_) => {
                assert!(!keeps, "{format}");
                let read = read_sl1(std::io::Cursor::new(bytes)).unwrap();
                assert_eq!(read.config_value("expTimeFirst"), Some("35"), "{format}");
                assert_eq!(read.config_value("expTime"), Some("2.5"), "{format}");
            }
            other => unreachable!("{other} is not ported"),
        }
    }
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
            PrinterFileFormat::Ctb(_) => read_ctb(&bytes).unwrap().layers.len(),
            PrinterFileFormat::AnycubicBinary(variant) => {
                read_anycubic_binary(&bytes, variant).unwrap().layers.len()
            }
            PrinterFileFormat::Sl1(_) => {
                read_sl1(std::io::Cursor::new(bytes)).unwrap().layers.len()
            }
            other => unreachable!("{other} is not ported"),
        };
        assert_eq!(layers, 4, "{format}");
    }
}
