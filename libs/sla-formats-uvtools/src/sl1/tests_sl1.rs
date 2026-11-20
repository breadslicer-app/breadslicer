/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

use std::io::Cursor;

use sc_print_primitives::{AaLevel, Mirror};
use sc_sla_formats::Sl1Variant;
use sc_sla_formats::layer_images::PLACEHOLDER_GREY;

use super::*;
use crate::read::{Sl1File, read_sl1};
use crate::test_fixture::{
    PIXELS_X, PIXELS_Y, assert_no_dark_layer, expected_images, gapped_stack, program, raster, stack,
};

fn written(variant: Sl1Variant, aa: AaLevel, layers: usize) -> Sl1File {
    let raster = raster(aa, Mirror { x: true, y: false });
    let program = program("Original Prusa SL1S SPEED", raster, layers);
    let mut out = Cursor::new(Vec::new());
    write_file(variant, &program, None, &stack(layers), &mut out).unwrap();
    read_sl1(Cursor::new(out.into_inner())).unwrap()
}

#[test]
fn the_archive_holds_thumbnails_layers_and_both_ini_files() {
    let file = written(Sl1Variant::Sl1s, AaLevel::Off, 4);
    let mut expected = vec![
        "thumbnail/thumbnail400x400.png".to_owned(),
        "thumbnail/thumbnail800x480.png".to_owned(),
    ];
    expected.extend((0..4).map(layer_entry));
    expected.extend([CONFIG_FILE, PRUSASLICER_FILE].map(str::to_owned));
    assert_eq!(file.entries, expected);
    assert_eq!(layer_entry(12), "breadslicer00012.png");
}

#[test]
fn every_layer_reads_back_as_the_slicer_rasterised_it_at_every_grey() {
    for variant in Sl1Variant::ALL {
        for aa in [AaLevel::Off, AaLevel::X8] {
            let raster = raster(aa, Mirror { x: true, y: false });
            let program = program("Original Prusa SL1", raster.clone(), 6);
            let stack = stack(6);
            let mut out = Cursor::new(Vec::new());
            write_file(variant, &program, None, &stack, &mut out).unwrap();

            let file = read_sl1(Cursor::new(out.into_inner())).unwrap();
            assert_eq!(
                file.layers,
                expected_images(&stack, &raster),
                "{variant:?} {aa:?}"
            );
            assert!(
                file.layer_sizes
                    .iter()
                    .all(|&size| size == (PIXELS_X, PIXELS_Y))
            );
        }
    }
}

#[test]
fn the_config_states_the_program() {
    let file = written(Sl1Variant::Sl1s, AaLevel::Off, 6);
    let value = |key| file.config_value(key).unwrap_or_else(|| panic!("{key}"));
    assert_eq!(value("action"), "print");
    assert_eq!(value("jobDir"), JOB_NAME);
    assert_eq!(value("expTime"), "2.5");
    assert_eq!(value("expTimeFirst"), "35");
    assert_eq!(value("layerHeight"), "0.05");
    assert_eq!(value("numFade"), "2");
    assert_eq!(value("materialName"), "Validation Resin");
    assert_eq!(value("printTime"), "321.5");
    assert_eq!(value("printerModel"), "SL1S");
    assert_eq!(value("printerProfile"), "Original Prusa SL1S SPEED");
    assert_eq!(value("usedMaterial"), "0.0125");
    assert_eq!(value("fileCreationTimestamp"), "2026-10-27 at 10:00:00 UTC");
    let slow: u32 = value("numSlow").parse().unwrap();
    let fast: u32 = value("numFast").parse().unwrap();
    assert_eq!(slow + fast, 6);
}

#[test]
fn the_prusaslicer_settings_name_the_machine_and_agree_with_the_config() {
    for (variant, model) in [(Sl1Variant::Sl1, "SL1"), (Sl1Variant::Sl1s, "SL1S")] {
        let file = written(variant, AaLevel::Off, 3);
        let value = |key| {
            file.prusaslicer_value(key)
                .unwrap_or_else(|| panic!("{key}"))
        };
        assert_eq!(value("printer_model"), model);
        assert_eq!(value("printer_technology"), "SLA");
        assert_eq!(value("display_pixels_x"), PIXELS_X.to_string());
        assert_eq!(value("display_pixels_y"), PIXELS_Y.to_string());
        assert_eq!(value("display_mirror_x"), "1");
        assert_eq!(value("display_mirror_y"), "0");
        // UVtools reads both files into one set of settings: each height
        // must be the config's, or the first layer moves.
        assert_eq!(
            value("layer_height"),
            file.config_value("layerHeight").unwrap()
        );
        assert_eq!(value("initial_layer_height"), value("layer_height"));
        assert_eq!(
            value("exposure_time"),
            file.config_value("expTime").unwrap()
        );
        assert_eq!(
            value("initial_exposure_time"),
            file.config_value("expTimeFirst").unwrap()
        );
    }
}

#[test]
fn a_layer_lit_past_the_area_fill_tilts_slowly() {
    let width = 10;
    let height = 10;
    let half = [
        RleRun {
            value: 255,
            length: 46,
        },
        RleRun {
            value: 0,
            length: 54,
        },
    ];
    assert!(encode(&half, width, height, 45.0).slow);
    assert!(!encode(&half, width, height, 50.0).slow);
    let grey = [RleRun {
        value: 100,
        length: 100,
    }];
    // 100 of 255 lights 39 %: fast, every pixel counted by its grey.
    assert!(!encode(&grey, width, height, 45.0).slow);
}

#[test]
fn no_layer_is_written_dark() {
    let raster = raster(AaLevel::X4, Mirror::default());
    let stack = gapped_stack();
    let program = program("Original Prusa SL1", raster, stack.layers().len());
    let mut out = Cursor::new(Vec::new());
    write_file(Sl1Variant::Sl1, &program, None, &stack, &mut out).unwrap();
    let file = read_sl1(Cursor::new(out.into_inner())).unwrap();
    assert_no_dark_layer(&file.layers, PLACEHOLDER_GREY);
}

#[test]
fn the_utc_date_of_a_moment_is_its_civil_date() {
    assert_eq!(creation_timestamp(0), "1970-01-01 at 00:00:00 UTC");
    assert_eq!(
        creation_timestamp(951_782_400),
        "2000-02-29 at 00:00:00 UTC"
    );
    assert_eq!(
        creation_timestamp(1_707_421_970),
        "2024-02-08 at 19:52:50 UTC"
    );
}
