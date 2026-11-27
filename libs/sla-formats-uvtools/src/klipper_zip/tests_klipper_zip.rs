/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

use std::io::Cursor;

use sc_print_primitives::{AaLevel, Mirror};

use super::*;
use crate::read::read_image_archive;
use crate::test_fixture::{expected_images, program, raster, stack};

#[test]
fn the_archive_holds_the_layers_the_marker_the_gcode_and_the_previews() {
    let raster = raster(AaLevel::X4, Mirror::default());
    let program = program("IBEE", raster.clone(), 5);
    let stack = stack(5);
    let mut out = Cursor::new(Vec::new());
    write_file(&program, None, &stack, &mut out).unwrap();
    let archive = read_image_archive(Cursor::new(out.into_inner())).unwrap();
    let mut expected: Vec<String> = (1..=5).map(|i| format!("{i}.png")).collect();
    expected.extend(
        [
            ".klipper",
            "breadslicer.gcode",
            "preview.png",
            "preview_cropping.png",
        ]
        .map(str::to_owned),
    );
    assert_eq!(archive.entries, expected);
    for (index, image) in expected_images(&stack, &raster).iter().enumerate() {
        assert_eq!(
            &archive.grey(&format!("{}.png", index + 1)).unwrap().1,
            image
        );
    }
    let gcode = archive.text("breadslicer.gcode").unwrap();
    assert!(gcode.contains("M1451 F\"1.png\";Show image\n"), "{gcode}");
    assert!(gcode.contains("BEFORE_LAYER_CHANGE LAYER=5 "));
    assert!(gcode.contains("PRINT_START LAYERS=5 "));
    assert!(gcode.contains("; thumbnail begin 32x32 "));
    assert!(gcode.contains("PRINT_END\n"));
    assert!(gcode.contains(";LayerCount:5\n"));
}
