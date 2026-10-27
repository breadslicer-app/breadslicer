/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

use sc_print_primitives::{AaLevel, Mirror};
use sc_sl1cer::raster::rle::RleRun;
use sc_sla_formats::{CxdlpVersion, PreviewImage, PrintPreviews, WriteFailure};

use super::*;
use crate::read::read_cxdlp;
use crate::test_fixture::{PIXELS_X, PIXELS_Y, expected_images, program, raster, stack};

fn written(version: CxdlpVersion, aa: AaLevel, machine: &str) -> Vec<u8> {
    let program = program(machine, raster(aa, Mirror::default()), 6);
    let mut out = Vec::new();
    write_file(version, &program, None, &stack(6), &mut out).unwrap();
    out
}

#[test]
fn the_crc_starts_from_zero_and_is_never_inverted() {
    assert_eq!(crc32_zero_start(0, b"123456789"), 0x2dfd_2d88);
    // Continuing over a split gives the same as one pass.
    assert_eq!(
        crc32_zero_start(crc32_zero_start(0, b"1234"), b"56789"),
        0x2dfd_2d88,
    );
}

#[test]
fn the_header_matches_the_bytes_uvtools_writes() {
    let bytes = written(CxdlpVersion::V3, AaLevel::Off, "Halot One");
    let mut expected = vec![0, 0, 0, 9];
    expected.extend_from_slice(b"CXSW3DV2\0");
    expected.extend_from_slice(&[0, 3, 0, 0, 0, 6]);
    expected.extend_from_slice(b"CL-60\0");
    expected.extend_from_slice(&6_u16.to_be_bytes());
    expected.extend_from_slice(&(PIXELS_X as u16).to_be_bytes());
    expected.extend_from_slice(&(PIXELS_Y as u16).to_be_bytes());
    expected.extend_from_slice(&[0; 64]);
    assert_eq!(&bytes[..expected.len()], expected.as_slice());
}

#[test]
fn every_layer_reads_back_as_the_slicer_rasterised_it() {
    for aa in [AaLevel::Off, AaLevel::X4] {
        for version in [CxdlpVersion::V2, CxdlpVersion::V3] {
            let raster = raster(aa, Mirror { x: true, y: false });
            let program = program("Halot One", raster.clone(), 6);
            let stack = stack(6);
            let mut out = Vec::new();
            write_file(version, &program, None, &stack, &mut out).unwrap();

            let file = read_cxdlp(&out).unwrap();
            assert!(file.checksum_matches, "{version:?} {aa:?}");
            assert_eq!(file.layers, expected_images(&stack, &raster), "{aa:?}");
        }
    }
}

#[test]
fn anti_aliased_layers_keep_their_greys() {
    let file = read_cxdlp(&written(CxdlpVersion::V3, AaLevel::X4, "Halot One")).unwrap();
    let greys: std::collections::BTreeSet<u8> = file.layers.iter().flatten().copied().collect();
    assert!(
        greys.len() > 3,
        "an anti-aliased slanted edge carries greys: {greys:?}"
    );
}

#[test]
fn the_settings_are_stored_as_uvtools_rounds_them() {
    let file = read_cxdlp(&written(CxdlpVersion::V3, AaLevel::Off, "Halot One")).unwrap();
    assert_eq!(file.version, 3);
    assert_eq!(file.printer_model, "CL-60");
    assert_eq!(
        (file.resolution_x, file.resolution_y),
        (PIXELS_X as u16, PIXELS_Y as u16)
    );
    assert_eq!(file.display_width, "10");
    assert_eq!(file.display_height, "7.5");
    assert_eq!(file.layer_height, "0.05");
    assert_eq!(file.exposure_tenths, 25);
    // 1.5 s rounds half away from zero.
    assert_eq!(file.wait_before_cure_sec, 2);
    assert_eq!(file.bottom_exposure_sec, 35);
    assert_eq!(file.bottom_layer_count, 2);
    assert_eq!(file.bottom_lift_mm, 7);
    assert_eq!(file.bottom_lift_mm_s, 1);
    assert_eq!(file.lift_mm, 6);
    // 90 mm/min is 1.5 mm/s, stored as 2.
    assert_eq!(file.lift_mm_s, 2);
    // 150 mm/min is 2.5 mm/s, stored as 3.
    assert_eq!(file.retract_mm_s, 3);
    assert_eq!((file.bottom_light_pwm, file.light_pwm), (255, 255));
    assert_eq!(
        file.software_name.as_deref(),
        Some("Breadslicer v0.0.1-test")
    );
    assert_eq!(file.material_name.as_deref(), Some("Validation Resin"));
    assert_eq!(file.largest_outlines.len(), 6);
    assert!(file.largest_outlines.iter().all(|&area| area > 10_000));
}

#[test]
fn version_2_has_no_version_3_block_and_an_xor_checksum() {
    let file = read_cxdlp(&written(CxdlpVersion::V2, AaLevel::Off, "Halot One")).unwrap();
    assert_eq!(file.version, 2);
    assert_eq!(file.software_name, None);
    assert!(file.checksum_matches);
}

#[test]
fn the_previews_are_packed_as_rgb565() {
    let small = PreviewImage::new(116, 116, [255, 0, 0].repeat(116 * 116)).unwrap();
    let large = PreviewImage::new(290, 290, [0, 0, 255].repeat(290 * 290)).unwrap();
    let previews = PrintPreviews { small, large };
    let program = program("Halot One", raster(AaLevel::Off, Mirror::default()), 2);
    let mut out = Vec::new();
    write_file(
        CxdlpVersion::V3,
        &program,
        Some(&previews),
        &stack(2),
        &mut out,
    )
    .unwrap();

    let header = 4 + 9 + 2 + 4 + 6 + 2 + 2 + 2 + 64;
    assert_eq!(&out[header..header + 2], &[0xf8, 0x00]);
    let large_start = header + 116 * 116 * 2 + 2;
    assert_eq!(&out[large_start..large_start + 2], &[0x00, 0x1f]);
    let third = large_start + 290 * 290 * 2 + 2;
    assert_eq!(&out[third..third + 2], &[0x00, 0x1f]);
}

#[test]
fn a_preview_of_the_wrong_size_is_refused_before_writing() {
    let previews = PrintPreviews {
        small: PreviewImage::black(PreviewSize {
            width: 10,
            height: 10,
        }),
        large: PreviewImage::black(PREVIEW_SIZES.large),
    };
    let program = program("Halot One", raster(AaLevel::Off, Mirror::default()), 2);
    let mut out = Vec::new();
    let err = write_file(
        CxdlpVersion::V3,
        &program,
        Some(&previews),
        &stack(2),
        &mut out,
    )
    .unwrap_err();
    assert!(matches!(err, WriteFailure::PreviewSize(_)));
    assert!(out.is_empty());
}

#[test]
fn the_model_code_comes_from_the_name_or_the_panel() {
    assert_eq!(printer_model("CL-89", (1, 1)).as_deref(), Some("CL-89"));
    assert_eq!(
        printer_model("Creality HALOT-ONE-PLUS CL-79", (1, 1)).as_deref(),
        Some("CL-79"),
    );
    assert_eq!(
        printer_model("Halot One Pro", (1, 1)).as_deref(),
        Some("CL-70")
    );
    assert_eq!(
        printer_model("Some Halot", (1620, 2560)).as_deref(),
        Some("CL-60")
    );
    assert_eq!(printer_model("Some Printer", (1000, 1000)), None);
}

#[test]
fn a_machine_with_no_model_code_is_refused() {
    let program = program("Mystery", raster(AaLevel::Off, Mirror::default()), 2);
    let mut out = Vec::new();
    let err = write_file(CxdlpVersion::V3, &program, None, &stack(2), &mut out).unwrap_err();
    assert!(matches!(err, WriteFailure::Unencodable(_)), "{err}");
    assert!(out.is_empty());
}

#[test]
fn a_column_run_is_one_line_per_grey_change() {
    // A 3 x 4 frame; column 1 lit in rows 0..=2 with a grey change at row 2.
    let runs = [
        RleRun {
            value: 0,
            length: 1,
        },
        RleRun {
            value: 255,
            length: 1,
        },
        RleRun {
            value: 0,
            length: 2,
        },
        RleRun {
            value: 255,
            length: 1,
        },
        RleRun {
            value: 0,
            length: 2,
        },
        RleRun {
            value: 100,
            length: 1,
        },
        RleRun {
            value: 0,
            length: 4,
        },
    ];
    let record = encode_layer(&runs, 3, 42);
    assert_eq!(&record[..4], &42_u32.to_be_bytes());
    assert_eq!(&record[4..8], &2_u32.to_be_bytes());
    let mut first = Vec::new();
    push_line(&mut first, 0, 1, 1, 255);
    push_line(&mut first, 2, 2, 1, 100);
    assert_eq!(&record[8..20], first.as_slice());
    assert_eq!(&record[20..], &PAGE_BREAK);
}
