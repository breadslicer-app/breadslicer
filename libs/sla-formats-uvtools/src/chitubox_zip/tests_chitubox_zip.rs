/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

use std::io::Cursor;

use sc_print_primitives::{AaLevel, Mirror};

use super::*;
use crate::read::{ImageArchive, read_image_archive};
use crate::test_fixture::{expected_images, program, raster, stack};

fn written(aa: AaLevel, layers: usize) -> (ImageArchive, Vec<Vec<u8>>) {
    let raster = raster(aa, Mirror { x: true, y: false });
    let program = program("Phrozen Sonic", raster.clone(), layers);
    let stack = stack(layers);
    let mut out = Cursor::new(Vec::new());
    write_file(&program, None, &stack, &mut out).unwrap();
    let archive = read_image_archive(Cursor::new(out.into_inner())).unwrap();
    (archive, expected_images(&stack, &raster))
}

#[test]
fn the_archive_holds_the_gcode_previews_and_every_layer() {
    let (archive, _) = written(AaLevel::Off, 3);
    assert_eq!(
        archive.entries,
        [
            "run.gcode",
            "preview.png",
            "preview_cropping.png",
            "1.png",
            "2.png",
            "3.png"
        ]
    );
}

#[test]
fn every_layer_reads_back_as_the_slicer_rasterised_it_at_every_grey() {
    for aa in [AaLevel::Off, AaLevel::X8] {
        let (archive, expected) = written(aa, 6);
        for (index, image) in expected.iter().enumerate() {
            let (_, pixels) = archive.grey(&format!("{}.png", index + 1)).unwrap();
            assert_eq!(&pixels, image, "{aa:?} layer {index}");
        }
    }
}

#[test]
fn the_gcode_states_the_settings_and_shows_each_layer() {
    let (archive, _) = written(AaLevel::Off, 6);
    let gcode = archive.text(GCODE_FILE).unwrap();
    assert!(gcode.contains(";machineType:Phrozen Sonic\n"), "{gcode}");
    assert!(gcode.contains(";totalLayer:6\n"));
    assert!(gcode.contains(";bottomLayerCount:2\n"));
    assert!(gcode.contains(";mirror:1\n"));
    for index in 1..=6 {
        assert!(gcode.contains(&format!("M6054 \"{index}.png\";Show image\n")));
    }
    assert!(gcode.contains("G90;Absolute positioning\n"));
    assert!(gcode.contains("M106 S255;Turn LED ON\n"));
    assert!(gcode.trim_end().ends_with(";<Completed>"));
}
