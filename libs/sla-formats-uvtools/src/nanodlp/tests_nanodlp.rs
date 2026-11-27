/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

use std::io::Cursor;

use sc_print_primitives::{AaLevel, Mirror};

use super::*;
use crate::test_fixture::{PIXELS_X, program, raster, stack};

#[test]
fn the_manifests_are_json_and_state_the_program() {
    let program = program("Athena 8K", raster(AaLevel::X4, Mirror::default()), 3);
    let machine = machine(&program);
    let bounds = vec![Some([2, 3, 10, 12]); 3];
    let model = boundary(&program, &bounds);
    for text in [
        meta(),
        slicer(&program, &machine, &model),
        plate(&program, &model),
        profile(&program, &machine),
        info(&stack(3), &bounds),
    ] {
        serde_json::from_str::<serde_json::Value>(&text)
            .unwrap_or_else(|err| panic!("{err}: {text}"));
    }
    let profile: serde_json::Value = serde_json::from_str(&profile(&program, &machine)).unwrap();
    assert_eq!(profile["CureTime"], 2.5);
    assert_eq!(profile["SupportLayerNumber"], 2);
    assert_eq!(profile["Depth"], 50);
    assert!(
        profile["ShieldAfterLayer"]
            .as_str()
            .unwrap()
            .contains(&format!("{0}u0026{0}u0026", char::from(92))),
        "the template keeps its escapes as UVtools states them"
    );
    let info: serde_json::Value = serde_json::from_str(&info(&stack(3), &bounds)).unwrap();
    assert_eq!(info.as_array().unwrap().len(), 3);
    assert_eq!(info[0]["MinX"], 2);
}

#[test]
fn a_panel_not_three_pixels_a_pixel_is_refused() {
    if PIXELS_X % 3 == 0 {
        return;
    }
    let program = program("Athena 8K", raster(AaLevel::Off, Mirror::default()), 2);
    let mut out = Cursor::new(Vec::new());
    assert!(matches!(
        write_file(&program, None, &stack(2), &mut out),
        Err(WriteFailure::Unencodable(_))
    ));
}
