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
fn the_archive_holds_its_metadata_its_layers_and_the_run() {
    let raster = raster(AaLevel::Off, Mirror { x: true, y: false });
    let program = program("Inkspire", raster.clone(), 3);
    let stack = stack(3);
    let mut out = Cursor::new(Vec::new());
    write_file(&program, None, &stack, &mut out).unwrap();
    let archive = read_image_archive(Cursor::new(out.into_inner())).unwrap();
    assert_eq!(
        archive.entries,
        [
            "ResinMetadata",
            "UserSettingsData",
            "ZCodeMetadata",
            "Preview.png",
            "ResinSlicesData/Slice00000.png",
            "ResinSlicesData/Slice00001.png",
            "ResinSlicesData/Slice00002.png",
            GCODE_FILE,
        ]
    );
    for (index, image) in expected_images(&stack, &raster).iter().enumerate() {
        assert_eq!(&archive.grey(&layer_entry(index)).unwrap().1, image);
    }
    for name in ["ResinMetadata", "UserSettingsData", "ZCodeMetadata"] {
        let value: serde_json::Value = serde_json::from_str(archive.text(name).unwrap()).unwrap();
        assert!(value.is_object(), "{name}");
    }
    let run = archive.text(GCODE_FILE).unwrap();
    assert!(run.starts_with("<Slice> 0\n"), "{run}");
    assert_eq!(run.matches("<Delay_model>").count(), 3);
    assert!(run.ends_with("M18\n"));
}
