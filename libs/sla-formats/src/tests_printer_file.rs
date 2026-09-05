/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::fs;

use sc_print_primitives::{
    AaLevel, BuildVolume, LightPwm, Mirror, NonEmptyString, NonNegativeFiniteSec, PixelResolution,
    PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec,
};
use sc_sl1cer::contour::{ClosedRing, ContourLayer, ContourStack, Polygon};
use sc_sl1cer::geom2d::Point2D;
use sc_sl1cer::profile::{LayerSchedule, PackingMode, SliceProfile};
use tempfile::tempdir;

use crate::error::WriterError;
use crate::print_program::{LayerProgram, MotionStage, PrintProgram};
use crate::printer_file_format::{CtbVariant, GooVersion, PrinterFileFormat};
use crate::raster_setup::RasterSetup;
use crate::write::write_printer_file;

const GOO_V3_0: PrinterFileFormat = PrinterFileFormat::Goo(GooVersion::V3_0);
const HEADER_SIZE: usize = 195_477;
const FILE_MAGIC: [u8; 8] = [0x07, 0x00, 0x00, 0x00, 0x44, 0x4C, 0x50, 0x00];

fn mm(value: f64) -> PositiveFiniteMm {
    PositiveFiniteMm::new(value).unwrap()
}

fn raster() -> RasterSetup {
    let profile = SliceProfile::new(
        BuildVolume::new(10.0, 10.0, 100.0).unwrap(),
        PixelResolution::new(20, 20).unwrap(),
        LayerSchedule::uniform(0.1, 0.1, 2).unwrap(),
        Mirror::default(),
        AaLevel::Off,
        PackingMode::None,
    );
    RasterSetup::from_profile(&profile)
}

fn layer(slab_top_mm: f64) -> LayerProgram {
    LayerProgram {
        slab_top_mm: mm(slab_top_mm),
        exposure_sec: PositiveFiniteSec::new(2.5).unwrap(),
        wait_before_cure_sec: NonNegativeFiniteSec::ZERO,
        wait_after_cure_sec: NonNegativeFiniteSec::ZERO,
        wait_after_lift_sec: NonNegativeFiniteSec::ZERO,
        lift: MotionStage {
            distance_mm: mm(5.0),
            speed_mm_min: PositiveFiniteMmPerMin::new(90.0).unwrap(),
        },
        retract: MotionStage {
            distance_mm: mm(5.0),
            speed_mm_min: PositiveFiniteMmPerMin::new(150.0).unwrap(),
        },
        second_lift: None,
        second_retract: None,
        light_pwm: LightPwm::FULL,
    }
}

fn program(layer_count: usize) -> PrintProgram {
    PrintProgram {
        raster: raster(),
        machine_name: NonEmptyString::new("Mars 5").unwrap(),
        material_name: NonEmptyString::new("Blu").unwrap(),
        layers: (1..=layer_count)
            .map(|index| layer(0.1 * index as f64))
            .collect(),
        layer_height_mm: mm(0.1),
        bottom_layer_count: 1,
        transition_layer_count: 0,
        created_at: NonEmptyString::new("2026-09-04 21:07:00").unwrap(),
        slicer_version: "0.0.12",
    }
}

fn stack(layer_count: usize) -> ContourStack {
    let layers = (0..layer_count)
        .map(|index| {
            let ring = ClosedRing::new(vec![
                Point2D::new(-2.0, -2.0),
                Point2D::new(2.0, -2.0),
                Point2D::new(2.0, 2.0),
                Point2D::new(-2.0, 2.0),
            ]);
            let z_mm = 0.1f64.mul_add(index as f64, 0.05);
            ContourLayer::new(z_mm, vec![Polygon::new_unchecked(ring, Vec::new())])
        })
        .collect();
    ContourStack::new_unchecked(layers)
}

#[test]
fn goo_v3_0_writes_a_file_bounded_by_the_magic() {
    let dir = tempdir().unwrap();
    let out = dir.path().join("job.goo");
    write_printer_file(&stack(2), &out, GOO_V3_0, &program(2)).unwrap();
    let bytes = fs::read(&out).unwrap();

    assert_eq!(&bytes[0..4], b"V3.0");
    assert_eq!(&bytes[4..12], &FILE_MAGIC);
    assert_eq!(&bytes[bytes.len() - 8..], &FILE_MAGIC);
    assert!(bytes.len() > HEADER_SIZE + 11);
}

#[test]
fn goo_v3_0_overwrites_a_file_that_is_already_there() {
    let dir = tempdir().unwrap();
    let out = dir.path().join("job.goo");
    fs::write(&out, b"stale").unwrap();
    write_printer_file(&stack(1), &out, GOO_V3_0, &program(1)).unwrap();

    assert_eq!(&fs::read(&out).unwrap()[0..4], b"V3.0");
}

#[test]
fn a_format_without_a_writer_is_refused_and_writes_nothing() {
    let dir = tempdir().unwrap();
    for format in [
        PrinterFileFormat::Goo(GooVersion::V5_1),
        PrinterFileFormat::Ctb(CtbVariant::PlainV5),
        PrinterFileFormat::Ctb(CtbVariant::EncryptedV5),
    ] {
        let out = dir.path().join(format!("{format}.out"));
        let err = write_printer_file(&stack(1), &out, format, &program(1)).unwrap_err();

        assert!(
            matches!(err, WriterError::PrinterFormatNotImplemented { format: refused } if refused == format),
            "{format}: {err}",
        );
        assert!(!out.exists(), "{format} left a file behind");
    }
}

#[test]
fn a_program_that_does_not_match_the_stack_leaves_the_target_alone() {
    let dir = tempdir().unwrap();
    let out = dir.path().join("job.goo");
    fs::write(&out, b"precious").unwrap();
    let err = write_printer_file(&stack(3), &out, GOO_V3_0, &program(2)).unwrap_err();

    assert!(
        matches!(
            err,
            WriterError::ProgramLayerCountMismatch {
                program_layers: 2,
                stack_layers: 3,
            },
        ),
        "{err}",
    );
    assert_eq!(fs::read(&out).unwrap(), b"precious");
}

#[test]
fn a_directory_as_the_output_path_is_refused() {
    let dir = tempdir().unwrap();
    let err = write_printer_file(&stack(1), dir.path(), GOO_V3_0, &program(1)).unwrap_err();

    assert!(
        matches!(err, WriterError::OutputPathInvalid { .. }),
        "{err}"
    );
}

#[test]
fn the_refusal_names_the_format_a_profile_would_declare() {
    let dir = tempdir().unwrap();
    let err = write_printer_file(&stack(1), dir.path(), GOO_V3_0, &program(1)).unwrap_err();

    assert!(err.to_string().contains("goo@v3.0"), "{err}");
}

#[test]
fn an_empty_program_writes_a_header_and_a_footer() {
    let dir = tempdir().unwrap();
    let out = dir.path().join("empty.goo");
    write_printer_file(&stack(0), &out, GOO_V3_0, &program(0)).unwrap();

    assert_eq!(fs::metadata(&out).unwrap().len() as usize, HEADER_SIZE + 11);
}
