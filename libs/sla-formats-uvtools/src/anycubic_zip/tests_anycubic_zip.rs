/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

use std::io::Cursor;

use sc_print_primitives::{AaLevel, Mirror, PanelRotation};
use sc_sla_formats::AnycubicZipVariant;
use sc_sla_formats::layer_images::PLACEHOLDER_GREY;

use super::*;
use crate::read::read_anycubic_zip;
use crate::test_fixture::{
    DARK_LAYERS, PIXELS_X, PIXELS_Y, assert_no_dark_layer, expected_images, gapped_stack, program,
    raster, stack, turned_ccw,
};

fn written(variant: AnycubicZipVariant, aa: AaLevel) -> Vec<u8> {
    let program = program("Photon Mono M7 Pro", raster(aa, Mirror::default()), 6);
    let mut out = Cursor::new(Vec::new());
    write_file(variant, &program, None, &stack(6), &mut out).unwrap();
    out.into_inner()
}

#[test]
fn the_archive_holds_the_entries_uvtools_writes_in_its_order() {
    let file =
        read_anycubic_zip(Cursor::new(written(AnycubicZipVariant::Pwsz, AaLevel::Off))).unwrap();
    let mut expected: Vec<String> = (0..3).map(preview_entry).collect();
    expected.extend(
        [
            SETTINGS_FILE,
            LAYERS_FILE,
            "lcd_function.json",
            "print_info.json",
            "software_info.conf",
        ]
        .map(str::to_owned),
    );
    expected.extend((0..6).map(layer_entry));
    expected.extend(["calc_layer_volumes.data", SCENE_FILE].map(str::to_owned));
    assert_eq!(file.entries, expected);
}

#[test]
fn every_layer_reads_back_as_the_slicer_rasterised_it_at_16_greys() {
    for aa in [AaLevel::Off, AaLevel::X8] {
        let raster = raster(aa, Mirror { x: true, y: false });
        let program = program("Photon Mono M7 Pro", raster.clone(), 6);
        let stack = stack(6);
        let mut out = Cursor::new(Vec::new());
        write_file(AnycubicZipVariant::Pwsz, &program, None, &stack, &mut out).unwrap();

        let file = read_anycubic_zip(Cursor::new(out.into_inner())).unwrap();
        let quantised: Vec<Vec<u8>> = expected_images(&stack, &raster)
            .into_iter()
            .map(|image| image.into_iter().map(|grey| (grey >> 4) * 17).collect())
            .collect();
        assert_eq!(file.layers, quantised, "{aa:?}");
    }
}

#[test]
fn a_quarter_turned_panel_is_named_and_stored_turned() {
    let plate = raster(AaLevel::Off, Mirror::default());
    let turned = plate.clone().with_panel_rotation(PanelRotation::Ccw90);
    let program = program("Photon Mono M7 Pro", turned, 6);
    let stack = stack(6);
    let mut out = Cursor::new(Vec::new());
    write_file(AnycubicZipVariant::Pwsz, &program, None, &stack, &mut out).unwrap();

    let file = read_anycubic_zip(Cursor::new(out.into_inner())).unwrap();
    let machine = &file.settings["machine_type"];
    assert_eq!(machine["res_x"], PIXELS_Y);
    assert_eq!(machine["res_y"], PIXELS_X);
    assert_eq!(machine["print_xsize"], 7.5);
    assert_eq!(machine["print_ysize"], 10);
    let expected: Vec<Vec<u8>> = expected_images(&stack, &plate)
        .iter()
        .map(|image| turned_ccw(image, PIXELS_X, PIXELS_Y))
        .collect();
    assert_eq!(file.layers, expected);
    // The scene's bounds are measured on the panel.
    let [left, top, right, bottom] = file.scene_layers[5].bounds_mm;
    assert!(left >= -3.75 && right <= 3.75 && left < right);
    assert!(top >= -5.0 && bottom <= 5.0 && top < bottom);
}

#[test]
fn the_machine_block_names_the_panel_and_the_variant() {
    for variant in [
        AnycubicZipVariant::Pwsz,
        AnycubicZipVariant::Pm7,
        AnycubicZipVariant::Pm7m,
    ] {
        let file = read_anycubic_zip(Cursor::new(written(variant, AaLevel::X4))).unwrap();
        let machine = &file.settings["machine_type"];
        assert_eq!(machine["name"], "Anycubic Photon Mono M7 Pro");
        assert_eq!(machine["key_suffix"], variant.extension());
        assert_eq!(machine["res_x"], PIXELS_X);
        assert_eq!(machine["res_y"], PIXELS_Y);
        assert_eq!(machine["xy_pixel"], 250);
        assert_eq!(machine["print_xsize"], 10);
        assert_eq!(machine["print_ysize"], 7.5);
        assert_eq!(machine["raster_antialiasing"], 4);
        assert_eq!(
            file.settings["machine_extern"]["picture"],
            "AnycubicPhotonMonoM7Pro.png"
        );
    }
}

#[test]
fn the_active_resin_carries_the_print_settings() {
    let file =
        read_anycubic_zip(Cursor::new(written(AnycubicZipVariant::Pwsz, AaLevel::Off))).unwrap();
    let resin = &file.settings["machine_extern"]["user_resins"][0];
    assert_eq!(resin["property"]["name"], "default_resin");
    let slice = &resin["slicepara"];
    assert_eq!(slice["bott_layers"], 2);
    assert_eq!(slice["bott_time"], 35);
    assert_eq!(slice["bott_time_dual"], 35);
    assert_eq!(slice["exposure_time"], 2.5);
    assert_eq!(slice["off_time"], 1.5);
    assert_eq!(slice["bott_off_time"], 3);
    assert_eq!(slice["wait_before_lift"], 0.5);
    assert_eq!(slice["bott_wait_before_lift"], 1);
    assert_eq!(slice["wait_after_lift"], 0.25);
    assert_eq!(slice["zthick"], 0.05);
    assert_eq!(slice["zup_height"], 6);
    assert_eq!(slice["zup_speed"], 1.5);
    assert_eq!(slice["zdown_speed"], 2.5);
    assert_eq!(slice["use_indivi_layerpara"], 0);
    let stages = &resin["slice_extpara"]["multi_state_paras"];
    assert_eq!(stages["bott_0"]["height"], 7);
    assert_eq!(stages["bott_0"]["up_speed"], 1);
    assert_eq!(stages["normal_0"]["down_speed"], 2.5);
    assert_eq!(stages["normal_1"]["height"], 0);
    assert_eq!(resin["slice_extpara"]["multi_state_used"], 0);
    // The factory resin keeps Anycubic's own defaults.
    let factory = &file.settings["machine_extern"]["factory_resins"][0]["slicepara"];
    assert_eq!(factory["bott_time"], 30);
}

#[test]
fn whole_numbers_are_written_without_a_fraction() {
    let program = program(
        "Photon Mono M7 Pro",
        raster(AaLevel::Off, Mirror::default()),
        2,
    );
    let json = String::from_utf8(
        json_bytes(&settings_manifest(AnycubicZipVariant::Pwsz, &program)).unwrap(),
    )
    .unwrap();
    assert!(json.contains("\"bott_time\": 35,"), "{json}");
    assert!(json.contains("\"price\": 25,"), "{json}");
    assert!(json.contains("\"currency\": \"€\""), "{json}");
    assert!(!json.contains(".0,"), "{json}");
}

#[test]
fn the_layer_list_and_the_scene_follow_the_program() {
    let file =
        read_anycubic_zip(Cursor::new(written(AnycubicZipVariant::Pwsz, AaLevel::Off))).unwrap();
    let paras = file.layers_manifest["paras"].as_array().unwrap();
    assert_eq!(file.layers_manifest["count"], 6);
    assert_eq!(paras[0]["exposure_time"], 35);
    assert_eq!(paras[0]["zup_height"], 7);
    assert_eq!(paras[0]["zup_speed"], 1);
    assert_eq!(paras[5]["exposure_time"], 2.5);
    assert_eq!(paras[5]["layer_index"], 5);
    assert_eq!(paras[5]["layer_minheight"], 0.25);
    assert_eq!(paras[5]["layer_thickness"], 0.05);

    assert_eq!(file.scene_layers.len(), 6);
    let last = file.scene_layers[5];
    assert!((last.top_mm - 0.3).abs() < 1e-6);
    assert_eq!(last.outline_count, 2);
    assert!(last.area_mm2 > 10.0 && last.area_mm2 < 30.0, "{last:?}");
    // Every lit pixel sits inside the plate.
    let [left, top, right, bottom] = last.bounds_mm;
    assert!(left >= -5.0 && right <= 5.0 && left < right);
    assert!(top >= -3.75 && bottom <= 3.75 && top < bottom);
}

#[test]
fn a_layer_without_contours_is_written_with_one_placeholder_pixel() {
    // #309: the M7 Pro stalled on a dark last layer. A gap and a dark top
    // each carry one pixel, and the scene describes it as one object.
    let raster = raster(AaLevel::Off, Mirror::default());
    let program = program("Photon Mono M7 Pro", raster.clone(), 6);
    let stack = gapped_stack();
    let mut out = Cursor::new(Vec::new());
    write_file(AnycubicZipVariant::Pwsz, &program, None, &stack, &mut out).unwrap();

    let file = read_anycubic_zip(Cursor::new(out.into_inner())).unwrap();
    assert_no_dark_layer(&file.layers, (PLACEHOLDER_GREY >> 4) * 17);
    let quantised: Vec<Vec<u8>> = expected_images(&stack, &raster)
        .into_iter()
        .map(|image| image.into_iter().map(|grey| (grey >> 4) * 17).collect())
        .collect();
    assert_eq!(file.layers, quantised);
    for index in DARK_LAYERS {
        let layer = &file.scene_layers[index];
        assert_eq!(layer.outline_count, 1, "layer {index}");
        let [left, top, right, bottom] = layer.bounds_mm;
        assert!(-5.0 < left && left < right && right < 5.0, "layer {index}");
        assert!(
            -3.75 < top && top < bottom && bottom < 3.75,
            "layer {index}"
        );
    }
}
