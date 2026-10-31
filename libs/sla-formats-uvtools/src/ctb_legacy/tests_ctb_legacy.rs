/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

use std::io::Cursor;

use sc_print_primitives::{AaLevel, Mirror, PositiveFiniteMmPerMin, PositiveFiniteSec};
use sc_sla_formats::layer_images::PLACEHOLDER_GREY;
use sc_sla_formats::{CtbVariant, MotionStage, PreviewImage, PrintPreviews, PrintProgram};

use super::*;
use crate::read::{CtbFile, read_ctb};
use crate::test_fixture::{
    PIXELS_X, PIXELS_Y, assert_no_dark_layer, expected_images, gapped_stack, mm, program, raster,
    stack,
};

const LEGACY: [CtbVariant; 3] = [CtbVariant::V2, CtbVariant::V3, CtbVariant::V4];

fn written_program(variant: CtbVariant, program: &PrintProgram) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    write_file(variant, program, None, &stack(6), &mut out).unwrap();
    out.into_inner()
}

fn written(variant: CtbVariant) -> CtbFile {
    let program = program("Mars 2 Pro", raster(AaLevel::X4, Mirror::default()), 6);
    read_ctb(&written_program(variant, &program)).unwrap()
}

/// What a grey reads back as through the 7-bit codec.
fn seven_bit(grey: u8) -> u8 {
    match grey >> 1 {
        0 => 0,
        code => (code << 1) | 1,
    }
}

#[test]
fn every_version_reads_back_as_the_slicer_rasterised_it() {
    for variant in LEGACY {
        for aa in [AaLevel::Off, AaLevel::X4] {
            let raster = raster(aa, Mirror { x: true, y: false });
            let program = program("Mars 2 Pro", raster.clone(), 6);
            let file = read_ctb(&written_program(variant, &program)).unwrap();
            let expected: Vec<Vec<u8>> = expected_images(&stack(6), &raster)
                .into_iter()
                .map(|image| image.into_iter().map(seven_bit).collect())
                .collect();
            assert_eq!(file.layers, expected, "{variant:?} {aa:?}");
        }
    }
}

#[test]
fn each_version_opens_with_its_magic_and_number() {
    for (variant, magic, version) in [
        (CtbVariant::V2, MAGIC_CTB, 2),
        (CtbVariant::V3, MAGIC_CTB, 3),
        (CtbVariant::V4, MAGIC_CTB_V4, 4),
    ] {
        let header = written(variant).header;
        assert_eq!((header.magic, header.version), (magic, version));
        assert_eq!(
            (header.resolution_x, header.resolution_y),
            (PIXELS_X, PIXELS_Y)
        );
        assert_eq!(
            (header.display_width_mm, header.display_height_mm),
            (10.0, 7.5)
        );
        assert_eq!(header.layer_count, 6);
        assert_ne!(header.layer_key, 0, "the layers are XORed, as UVtools does");
    }
}

#[test]
fn version_5_is_not_this_writer_s() {
    let program = program("Mars 2 Pro", raster(AaLevel::X4, Mirror::default()), 6);
    let mut out = Cursor::new(Vec::new());
    let err = write_file(CtbVariant::PlainV5, &program, None, &stack(6), &mut out).unwrap_err();
    assert!(matches!(err, WriteFailure::Unencodable(_)), "{err:?}");
}

#[test]
fn the_same_print_gives_the_same_bytes() {
    let program = program("Mars 2 Pro", raster(AaLevel::X4, Mirror::default()), 6);
    assert_eq!(
        written_program(CtbVariant::V3, &program),
        written_program(CtbVariant::V3, &program)
    );
}

#[test]
fn without_waits_of_their_own_versions_2_and_3_fold_the_wait_into_the_light_off_delay() {
    // Normal layers: 6 mm up at 90 mm/min and down at 150, then 1.5 s.
    let normal = 1.5 + 6.0 / 1.5 + 6.0 / 2.5;
    // Bottom layers: 7 mm up at 60 and down at 150, then 3 s.
    let bottom = 3.0 + 7.0 + 7.0 / 2.5;
    for variant in [CtbVariant::V2, CtbVariant::V3] {
        let file = written(variant);
        assert_eq!(file.header.light_off_sec, normal, "{variant:?}");
        assert_eq!(file.print_parameters.light_off_sec, normal);
        assert_eq!(file.print_parameters.bottom_light_off_sec, bottom);
        assert_eq!(file.layer_definitions[0].light_off_sec, bottom);
        assert_eq!(file.layer_definitions[5].light_off_sec, normal);
    }
    let file = written(CtbVariant::V4);
    assert_eq!(file.header.light_off_sec, 0.0);
    assert!(
        file.layer_definitions
            .iter()
            .all(|layer| layer.light_off_sec == 0.0)
    );
    let v4 = file.print_parameters_v4.unwrap();
    assert_eq!(v4.wait_before_cure_sec, 1.5);
    assert_eq!(v4.wait_after_cure_sec, 0.5);
    assert_eq!(v4.wait_after_lift_sec, 0.25);
}

#[test]
fn the_print_parameters_carry_the_motion() {
    let file = written(CtbVariant::V2);
    let parameters = file.print_parameters;
    assert_eq!(parameters.bottom_lift_mm, 7.0);
    assert_eq!(parameters.bottom_lift_mm_min, 60.0);
    assert_eq!(parameters.lift_mm, 6.0);
    assert_eq!(parameters.lift_mm_min, 90.0);
    assert_eq!(parameters.retract_mm_min, 150.0);
    assert_eq!(parameters.volume_ml, 0.013);
    assert_eq!(parameters.weight_g, 0.015);
    assert_eq!(parameters.bottom_layer_count, 2);
    assert_eq!(parameters.last_padding, V2_PADDING_MARK);
    assert_eq!(written(CtbVariant::V3).print_parameters.last_padding, 0);
}

#[test]
fn version_2_stores_images_straight_after_the_table() {
    let file = written(CtbVariant::V2);
    assert!(
        file.layer_definitions
            .iter()
            .all(|layer| layer.motion.is_none() && layer.table_size == TABLE_RECORD_BYTES)
    );
    let first = file.layer_definitions[0];
    let table_start = first.address - u64::from(TABLE_RECORD_BYTES) * 6;
    assert!(table_start > u64::from(HEADER_BYTES));
    assert_eq!(first.position_z_mm, 0.05);
    assert_eq!(first.exposure_sec, 35.0);
}

#[test]
fn version_3_adds_a_single_stage_motion_record_and_version_4_the_second_stage() {
    let file = written(CtbVariant::V3);
    for layer in &file.layer_definitions {
        assert_eq!(layer.table_size, LAYER_RECORD_BYTES);
        let motion = layer.motion.unwrap();
        assert_eq!(motion[10], 255.0, "full light");
        assert_eq!(
            &motion[2..4],
            &[0.0, 0.0],
            "no second stage before version 4"
        );
        assert_eq!(
            &motion[7..10],
            &[0.0, 0.0, 0.0],
            "no waits before version 4"
        );
    }
    assert_eq!(file.layer_definitions[0].motion.unwrap()[0..2], [7.0, 60.0]);
    assert_eq!(file.layer_definitions[5].motion.unwrap()[4], 150.0);

    let mut program = program("Mars 2 Pro", raster(AaLevel::X4, Mirror::default()), 6);
    for layer in &mut program.layers {
        layer.second_lift = Some(MotionStage {
            distance_mm: mm(4.0),
            speed_mm_min: PositiveFiniteMmPerMin::new(180.0).unwrap(),
        });
        layer.second_retract = Some(MotionStage {
            distance_mm: mm(3.0),
            speed_mm_min: PositiveFiniteMmPerMin::new(240.0).unwrap(),
        });
    }
    let file = read_ctb(&written_program(CtbVariant::V4, &program)).unwrap();
    let motion = file.layer_definitions[5].motion.unwrap();
    assert_eq!(&motion[0..4], &[10.0, 90.0, 4.0, 180.0]);
    assert_eq!(&motion[4..7], &[150.0, 3.0, 240.0]);
    assert_eq!(&motion[7..10], &[0.5, 0.25, 1.5]);
    assert_eq!(file.slicer_info.second_lift_mm, 4.0);
    assert_eq!(file.slicer_info.second_retract_mm, 3.0);
    assert_eq!(file.print_parameters.lift_mm, 10.0);
}

#[test]
fn the_slicer_info_names_the_machine_and_the_revision() {
    for variant in LEGACY {
        let info = written(variant).slicer_info;
        assert_eq!(info.machine_name, "Mars 2 Pro");
        assert_eq!(info.anti_alias_flag, 0x0F);
        assert_eq!(info.anti_alias_level, 4);
        assert_eq!(info.software_version, 0x0109_0000);
        // Every layer runs its group's settings.
        assert_eq!(info.per_layer_settings, 0);
    }
    assert_eq!(
        written(CtbVariant::V2)
            .slicer_info
            .print_parameters_v4_address,
        0
    );
    assert_ne!(
        written(CtbVariant::V4)
            .slicer_info
            .print_parameters_v4_address,
        0
    );
}

#[test]
fn a_layer_off_its_group_flags_per_layer_settings_from_version_3() {
    let mut program = program("Mars 2 Pro", raster(AaLevel::Off, Mirror::default()), 6);
    program.layers[4].exposure_sec = PositiveFiniteSec::new(3.0).unwrap();
    for (variant, flag) in [
        (CtbVariant::V2, 0),
        (CtbVariant::V3, 0x30),
        (CtbVariant::V4, 0x40),
    ] {
        let file = read_ctb(&written_program(variant, &program)).unwrap();
        assert_eq!(file.slicer_info.per_layer_settings, flag, "{variant:?}");
        assert_eq!(file.slicer_info.anti_alias_flag, 0x07);
        assert_eq!(file.layer_definitions[4].exposure_sec, 3.0);
    }
}

#[test]
fn version_4_carries_the_disclaimer_and_its_block() {
    let v4 = written(CtbVariant::V4).print_parameters_v4.unwrap();
    assert_eq!(v4.disclaimer.len() as u32, DISCLAIMER_SIZE);
    assert!(v4.disclaimer.starts_with("Layout and record format"));
    assert_eq!(v4.last_layer_index, 5);
    assert_eq!(v4.bottom_retract_mm_min, 150.0);
}

#[test]
fn the_previews_are_the_ctb_sizes() {
    let file = written(CtbVariant::V3);
    assert_eq!(file.previews, [(400, 300), (200, 125)]);

    let previews = PrintPreviews {
        small: PreviewImage::black(PREVIEW_SIZES.large),
        large: PreviewImage::black(PREVIEW_SIZES.large),
    };
    let program = program("Mars 2 Pro", raster(AaLevel::X4, Mirror::default()), 6);
    let mut out = Cursor::new(Vec::new());
    let err = write_file(
        CtbVariant::V2,
        &program,
        Some(&previews),
        &stack(6),
        &mut out,
    )
    .unwrap_err();
    assert!(matches!(err, WriteFailure::PreviewSize(_)), "{err:?}");
    assert!(out.into_inner().is_empty());
}

#[test]
fn a_layer_without_contours_is_written_with_one_placeholder_pixel() {
    // #309: a machine can stall on a dark layer, so a gap and a dark top
    // each carry one pixel.
    for variant in LEGACY {
        let program = program("Mars 2 Pro", raster(AaLevel::Off, Mirror::default()), 6);
        let mut out = Cursor::new(Vec::new());
        write_file(variant, &program, None, &gapped_stack(), &mut out).unwrap();
        let file = read_ctb(&out.into_inner()).unwrap();
        assert_no_dark_layer(&file.layers, seven_bit(PLACEHOLDER_GREY));
    }
}
