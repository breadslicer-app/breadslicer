/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

use std::io::Cursor;

use sc_print_primitives::{AaLevel, Mirror, PanelRotation, PositiveFiniteMmPerMin};
use sc_sla_formats::{
    AnycubicBinaryVariant, MotionStage, PreviewImage, PrintPreviews, PrintProgram,
};

use super::*;
use crate::pws;
use crate::read::{AnycubicBinaryFile, read_anycubic_binary};
use crate::test_fixture::{
    PIXELS_X, PIXELS_Y, expected_images, mm, program, raster, stack, turned_ccw,
};

fn written_program(variant: AnycubicBinaryVariant, program: &PrintProgram) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    write_file(variant, program, None, &stack(6), &mut out).unwrap();
    out.into_inner()
}

fn written(variant: AnycubicBinaryVariant, aa: AaLevel) -> AnycubicBinaryFile {
    let program = program("Photon Mono X", raster(aa, Mirror::default()), 6);
    read_anycubic_binary(&written_program(variant, &program), variant).unwrap()
}

/// What a layer image reads back as through a variant's layer codec.
fn quantised(variant: AnycubicBinaryVariant, aa: AaLevel, image: Vec<u8>) -> Vec<u8> {
    image
        .into_iter()
        .map(|grey| {
            if uses_bit_planes(variant) {
                pws::quantise(grey, aa.factor())
            } else {
                (grey >> 4) * 17
            }
        })
        .collect()
}

#[test]
fn every_variant_reads_back_as_the_slicer_rasterised_it() {
    for variant in AnycubicBinaryVariant::ALL {
        for aa in [AaLevel::Off, AaLevel::X4] {
            let raster = raster(aa, Mirror { x: true, y: false });
            let program = program("Photon Mono X", raster.clone(), 6);
            let file = read_anycubic_binary(&written_program(variant, &program), variant).unwrap();
            let expected: Vec<Vec<u8>> = expected_images(&stack(6), &raster)
                .into_iter()
                .map(|image| quantised(variant, aa, image))
                .collect();
            assert_eq!(file.layers, expected, "{variant:?} {aa:?}");
        }
    }
}

#[test]
fn each_variant_carries_the_tables_of_its_revision() {
    for variant in AnycubicBinaryVariant::ALL {
        let version = file_version(variant);
        let file = written(variant, AaLevel::X4);
        assert_eq!(file.version, version, "{variant:?}");
        assert_eq!(
            file.table_count,
            match version {
                VERSION_1 => 4,
                VERSION_516 => 8,
                VERSION_517 => 9,
                _ => 11,
            },
            "{variant:?}"
        );
        assert_eq!(file.header.table_length, header_bytes(version));
        assert_eq!(file.grey_table.is_some(), version >= VERSION_515);
        assert_eq!(file.machine.is_some(), version >= VERSION_516);
        assert_eq!(file.software.is_some(), version >= VERSION_517);
        assert_eq!(file.model.is_some(), version >= VERSION_517);
        assert_eq!(file.sub_layers.is_some(), version >= VERSION_518);
        assert_eq!(
            file.preview2,
            (version >= VERSION_518).then_some((330, 190))
        );
        assert_eq!(file.preview, (224, 168));
    }
}

#[test]
fn the_revisions_follow_uvtools_extension_table() {
    use AnycubicBinaryVariant as V;
    assert_eq!(file_version(V::Pws), VERSION_1);
    assert_eq!(file_version(V::Pwmx), VERSION_516);
    assert_eq!(file_version(V::Pwma), VERSION_516);
    assert_eq!(file_version(V::Pwmb), VERSION_517);
    assert_eq!(file_version(V::Pm5), VERSION_517);
    assert_eq!(file_version(V::M5sp), VERSION_518);
    assert_eq!(file_version(V::Pm4n), VERSION_518);
    assert!(uses_bit_planes(V::Pws));
    assert!(!uses_bit_planes(V::Pw0));
}

#[test]
fn the_file_mark_opens_the_file_and_points_at_the_header() {
    let program = program("Photon S", raster(AaLevel::Off, Mirror::default()), 6);
    let bytes = written_program(AnycubicBinaryVariant::Pws, &program);
    assert_eq!(&bytes[..12], b"ANYCUBIC\0\0\0\0");
    assert_eq!(&bytes[12..16], &1_u32.to_le_bytes()); // revision
    assert_eq!(&bytes[16..20], &4_u32.to_le_bytes()); // tables
    assert_eq!(&bytes[20..24], &48_u32.to_le_bytes()); // the header's address
    assert_eq!(&bytes[48..60], b"HEADER\0\0\0\0\0\0");
    assert_eq!(&bytes[60..64], &80_u32.to_le_bytes());
}

#[test]
fn the_header_carries_the_print_settings() {
    let header = written(AnycubicBinaryVariant::Pwmx, AaLevel::X4).header;
    assert_eq!(header.pixel_size_um, 250.0);
    assert_eq!(header.layer_height_mm, 0.05);
    assert_eq!(header.exposure_sec, 2.5);
    assert_eq!(header.wait_before_cure_sec, 1.5);
    assert_eq!(header.bottom_exposure_sec, 35.0);
    assert_eq!(header.bottom_layer_count, 2.0);
    assert_eq!(header.lift_mm, 6.0);
    assert_eq!(header.lift_mm_s, 1.5);
    assert_eq!(header.retract_mm_s, 2.5);
    assert_eq!(header.volume_ml, 0.013);
    assert_eq!(header.weight_g, 0.015);
    assert_eq!(header.anti_aliasing, 4);
    assert_eq!(
        (header.resolution_x, header.resolution_y),
        (PIXELS_X, PIXELS_Y)
    );
    assert_eq!(header.print_time_sec, 321);
    assert_eq!(header.per_layer_settings, 0);
    assert_eq!(header.transition_layer_count, 0);
    assert_eq!(header.advanced_mode, Some(0));
}

#[test]
fn the_layer_table_carries_each_layer_and_its_lit_pixels() {
    let aa = AaLevel::X4;
    let raster = raster(aa, Mirror::default());
    let file = read_anycubic_binary(
        &written_program(
            AnycubicBinaryVariant::Pm5s,
            &program("Photon Mono M5s", raster.clone(), 6),
        ),
        AnycubicBinaryVariant::Pm5s,
    )
    .unwrap();
    let images = expected_images(&stack(6), &raster);
    assert_eq!(file.layer_definitions.len(), 6);
    for (index, (record, image)) in file.layer_definitions.iter().zip(&images).enumerate() {
        let bottom = index < 2;
        assert_eq!(record.exposure_sec, if bottom { 35.0 } else { 2.5 });
        assert_eq!(record.layer_height_mm, 0.05);
        assert_eq!(record.lift_mm, if bottom { 7.0 } else { 6.0 });
        assert_eq!(record.lift_mm_s, if bottom { 1.0 } else { 1.5 });
        let lit = image.iter().filter(|&&grey| grey != 0).count() as u32;
        assert_eq!(record.lit_pixels, lit, "layer {index}");
    }
    // The second layer table points at the same images.
    let sub_layers = file.sub_layers.unwrap();
    for (record, sub) in file.layer_definitions.iter().zip(sub_layers) {
        assert_eq!((record.address, record.length, record.lit_pixels), sub);
    }
}

#[test]
fn the_machine_table_names_the_panel() {
    let machine = written(AnycubicBinaryVariant::Pwmx, AaLevel::X4)
        .machine
        .unwrap();
    assert_eq!(machine.table_length, 156);
    assert_eq!(machine.name, "Photon Mono X");
    assert_eq!(machine.layer_image_format, "pw0Img");
    assert_eq!(machine.property_fields, 1);
    assert_eq!(
        (machine.display_width_mm, machine.display_height_mm),
        (10.0, 7.5)
    );
    assert_eq!(machine.machine_z_mm, 100.0);
    assert_eq!(machine.max_file_version, VERSION_516);
    assert_eq!(machine.pixel_um, None);

    let machine = written(AnycubicBinaryVariant::M5sp, AaLevel::X4)
        .machine
        .unwrap();
    assert_eq!(machine.table_length, 224);
    assert_eq!(machine.property_fields, 15);
    assert_eq!(machine.max_file_version, VERSION_518);
    assert_eq!(machine.pixel_um, Some((250.0, 250.0)));
    assert_eq!(machine.resolution, Some((PIXELS_X as u16, PIXELS_Y as u16)));
}

#[test]
fn the_grey_table_maps_each_level_to_its_grey() {
    let greys = written(AnycubicBinaryVariant::Pwma, AaLevel::X4)
        .grey_table
        .unwrap();
    assert_eq!(&greys[..5], &[63, 127, 191, 255, 255]);
    assert_eq!(greys.len(), 16);
    let greys = written(AnycubicBinaryVariant::Pwma, AaLevel::Off)
        .grey_table
        .unwrap();
    assert!(greys.iter().all(|&grey| grey == 255));
}

#[test]
fn the_software_and_the_model_describe_the_print() {
    let file = written(AnycubicBinaryVariant::Pm3r, AaLevel::X4);
    assert_eq!(
        file.software,
        Some(("Breadslicer".to_owned(), "0.0.1-test".to_owned()))
    );
    let [min_x, min_y, min_z, max_x, max_y, max_z] = file.model.unwrap();
    assert!(
        min_x >= -5.0 && max_x <= 5.0 && min_x < max_x,
        "{min_x} {max_x}"
    );
    assert!(
        min_y >= -3.75 && max_y <= 3.75 && min_y < max_y,
        "{min_y} {max_y}"
    );
    assert_eq!(min_z, 0.0);
    assert_eq!(max_z, 0.3);
}

fn stage(distance: f64, speed: f64) -> MotionStage {
    MotionStage {
        distance_mm: mm(distance),
        speed_mm_min: PositiveFiniteMmPerMin::new(speed).unwrap(),
    }
}

#[test]
fn a_two_stage_travel_fills_the_extra_table() {
    let mut program = program("Photon Mono X", raster(AaLevel::X4, Mirror::default()), 6);
    for (index, layer) in program.layers.iter_mut().enumerate() {
        let bottom = index < 2;
        layer.lift = stage(if bottom { 2.0 } else { 3.0 }, 60.0);
        layer.retract = stage(if bottom { 2.0 } else { 3.0 }, 90.0);
        layer.second_lift = Some(stage(if bottom { 4.0 } else { 5.0 }, 180.0));
        layer.second_retract = Some(stage(if bottom { 4.0 } else { 5.0 }, 360.0));
    }
    let file = read_anycubic_binary(
        &written_program(AnycubicBinaryVariant::Pwmx, &program),
        AnycubicBinaryVariant::Pwmx,
    )
    .unwrap();
    let [bottom, normal] = file.stages.unwrap();
    assert_eq!(bottom.lift_mm, 2.0);
    assert_eq!(bottom.lift_mm_s, 1.0);
    assert_eq!(bottom.retract_near_mm_s, 1.5);
    assert_eq!(bottom.second_lift_mm, 4.0);
    assert_eq!(bottom.second_lift_mm_s, 3.0);
    assert_eq!(bottom.retract_far_mm_s, 6.0);
    assert_eq!(normal.lift_mm, 3.0);
    assert_eq!(normal.second_lift_mm, 5.0);
    // The header holds the whole rise and the speed of the stretch it starts
    // the return on.
    assert_eq!(file.header.lift_mm, 8.0);
    assert_eq!(file.header.lift_mm_s, 1.0);
    assert_eq!(file.header.retract_mm_s, 6.0);
    assert_eq!(file.header.advanced_mode, Some(1));
    assert_eq!(file.layer_definitions[0].lift_mm, 6.0);
    assert_eq!(file.layer_definitions[5].lift_mm, 8.0);
}

#[test]
fn a_single_stage_travel_repeats_its_speeds() {
    let [bottom, normal] = written(AnycubicBinaryVariant::Pwmx, AaLevel::X4)
        .stages
        .unwrap();
    assert_eq!(bottom.lift_mm, 7.0);
    assert_eq!(bottom.second_lift_mm, 0.0);
    assert_eq!(bottom.second_lift_mm_s, bottom.lift_mm_s);
    assert_eq!(bottom.retract_far_mm_s, bottom.retract_near_mm_s);
    assert_eq!(normal.lift_mm, 6.0);
    assert_eq!(normal.retract_near_mm_s, 2.5);
}

#[test]
fn a_quarter_turned_panel_is_named_and_stored_turned() {
    let plate = raster(AaLevel::Off, Mirror::default());
    let turned = plate.clone().with_panel_rotation(PanelRotation::Ccw90);
    let variant = AnycubicBinaryVariant::Pm5s;
    let file = read_anycubic_binary(
        &written_program(variant, &program("Photon Mono M5s", turned, 6)),
        variant,
    )
    .unwrap();
    assert_eq!(
        (file.header.resolution_x, file.header.resolution_y),
        (PIXELS_Y, PIXELS_X)
    );
    let machine = file.machine.unwrap();
    assert_eq!(
        (machine.display_width_mm, machine.display_height_mm),
        (7.5, 10.0)
    );
    let expected: Vec<Vec<u8>> = expected_images(&stack(6), &plate)
        .iter()
        .map(|image| quantised(variant, AaLevel::Off, turned_ccw(image, PIXELS_X, PIXELS_Y)))
        .collect();
    assert_eq!(file.layers, expected);
}

#[test]
fn a_preview_of_the_wrong_size_is_refused_before_writing() {
    let previews = PrintPreviews {
        small: PreviewImage::black(PREVIEW_SIZES.small),
        large: PreviewImage::black(PREVIEW_SIZES.small),
    };
    let program = program("Photon Mono X", raster(AaLevel::X4, Mirror::default()), 6);
    let mut out = Cursor::new(Vec::new());
    let err = write_file(
        AnycubicBinaryVariant::Pwmx,
        &program,
        Some(&previews),
        &stack(6),
        &mut out,
    )
    .unwrap_err();
    assert!(matches!(err, WriteFailure::PreviewSize(_)), "{err:?}");
    assert!(out.into_inner().is_empty());
}
