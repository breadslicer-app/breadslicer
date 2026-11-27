/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

use super::*;

#[test]
fn base64_pads_as_the_standard_says() {
    assert_eq!(base64(b""), "");
    assert_eq!(base64(b"f"), "Zg==");
    assert_eq!(base64(b"fo"), "Zm8=");
    assert_eq!(base64(b"foo"), "Zm9v");
    assert_eq!(base64(b"foobar"), "Zm9vYmFy");
}

#[test]
fn numbers_read_as_dotnet_writes_them() {
    assert_eq!(num(5.0), "5");
    assert_eq!(num(0.05), "0.05");
    assert_eq!(num(2500.0), "2500");
    assert_eq!(num(1.25), "1.25");
}

#[test]
fn a_command_fills_its_slots_and_carries_its_comment() {
    let dialect = Dialect::default();
    let line = dialect
        .move_g0
        .line(true, true, Some("Z Lift (1)"), &["5.05", "60"]);
    assert_eq!(line, "G0 Z5.05 F60;Z Lift (1)");
    let line = dialect.move_g0.line(false, true, None, &["5.05", "60"]);
    assert_eq!(line, "G0 Z5.05 F60;");
    let line = dialect.clear_image.line(true, true, None, &[]);
    assert_eq!(line, ";<Slice> Blank");
}

fn layer(z: f32) -> GLayer {
    let mut layer = GLayer {
        position_z: z,
        exposure_sec: 2.5,
        lift_mm: 5.0,
        lift_mm_min: 60.0,
        retract_mm_min: 150.0,
        pwm: 255,
        ..GLayer::default()
    };
    layer.retract_mm = layer.lift_total_mm();
    layer
}

#[test]
fn an_absolute_layer_lifts_retracts_and_exposes() {
    let layers = [layer(0.05), layer(0.1)];
    let job = Job {
        layers: &layers,
        machine_z_mm: 100.0,
        lift_mm_min: 60.0,
        maximum_mm_min: 150.0,
        thumbnails: Vec::new(),
    };
    let mut builder = Builder::new(Dialect::default());
    builder.rebuild(&job, "", &mut NoHooks);
    let text = builder.text();
    let block = ";LAYER_START:1\n;PositionZ:0.1mm\nM6054 \"2.png\";Show image\n\
        G0 Z5.1 F60;Z Lift (1)\nG0 Z0.1 F150;Retract (1)\nG4 P0;Wait before cure\n\
        M106 S255;Turn LED ON\nG4 P2500;Cure time/delay\nM106 S0;Turn LED OFF\n\
        ;<Slice> Blank\n;LAYER_END\n";
    assert!(text.contains(block), "{text}");
}

#[test]
fn a_relative_layer_moves_by_the_distances() {
    let layers = [layer(0.05), layer(0.1)];
    let job = Job {
        layers: &layers,
        machine_z_mm: 100.0,
        lift_mm_min: 60.0,
        maximum_mm_min: 150.0,
        thumbnails: Vec::new(),
    };
    let dialect = Dialect {
        positioning: Positioning::Relative,
        ..Dialect::default()
    };
    let mut builder = Builder::new(dialect);
    builder.rebuild(&job, "", &mut NoHooks);
    let text = builder.text();
    assert!(
        text.contains("G0 Z5.05 F60;Z Lift (1)\nG0 Z-5 F150;Retract (1)\n"),
        "{text}"
    );
}
