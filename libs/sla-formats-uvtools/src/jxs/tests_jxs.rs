/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

use std::io::Cursor;

use sc_print_primitives::{AaLevel, Mirror};

use super::*;
use crate::test_fixture::{PIXELS_X, program, raster, stack};

#[test]
fn the_gcode_turns_into_layers_delays_and_lines() {
    let gcode = "; comment\nG21;Set units to be mm\n;LAYER_START:0\nM6054 \"0.png\";Show image\n\
        G0 Z5.05 F60;Z Lift (1)\nG4 P02500;Sync movement\nG4 P0;Wait before cure\n\
        M106 S255;Turn LED ON\nG4 P2500;Cure time/delay\nM106 S0;Turn LED OFF\n;<Slice> Blank\n";
    let (actions, footer) = actions(gcode, ";<Slice> Blank");
    let kinds: Vec<String> = actions
        .iter()
        .map(|action| match action {
            Action::Gcode(line) => format!("gcode {line}"),
            Action::Delay(ms) => format!("delay {ms}"),
            Action::LayerNumber(n) => format!("layerno {n}"),
            Action::Slice(path) => format!("slice {path}"),
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "gcode G21",
            "layerno 1",
            "slice {PWD}/00000.png",
            "gcode G0 Z5.05 F60",
            "delay 2500",
            "gcode M106 S255",
            "delay 2500",
            "gcode M106 S0",
            "slice <BLANK>",
        ]
    );
    assert_eq!(footer, "G0 Z5.05 F60");
}

#[test]
fn a_panel_not_three_pixels_a_pixel_is_refused() {
    if PIXELS_X % 3 == 0 {
        return;
    }
    let program = program("GKone", raster(AaLevel::Off, Mirror::default()), 2);
    let mut out = Cursor::new(Vec::new());
    assert!(matches!(
        write_file(&program, &stack(2), &mut out),
        Err(WriteFailure::Unencodable(_))
    ));
}
