/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_print_primitives::{
    AaLevel, BuildVolume, LightPwm, MassG, Mirror, NonEmptyString, NonNegativeFiniteSec,
    PixelResolution, PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec, VolumeMm3,
};
use sc_sl1cer::profile::{LayerSchedule, PackingMode, SliceProfile};

use super::header::{FILE_MAGIC, HEADER_SIZE, SizedPreviews, write_header};
use crate::preview::{PreviewImage, PrintPreviews};
use crate::print_program::{CreationTime, LayerProgram, MotionStage, PrintProgram};
use crate::raster_setup::RasterSetup;

const PLATE_X_MM: f64 = 143.43;
const PLATE_Y_MM: f64 = 89.6;
const PLATE_Z_MM: f64 = 150.0;
const RESOLUTION_X: u32 = 4098;
const RESOLUTION_Y: u32 = 2560;
const LAYER_HEIGHT_MM: f64 = 0.05;
/// Exactly representable in the `f32` the field holds, so a test can
/// assert equality rather than a tolerance.
const VOLUME_MM3: f64 = 11_701.25;
/// Likewise exactly representable in `f32`.
const MASS_G: f64 = 13.75;
/// Fractional, since the header field counts whole seconds.
const PRINT_TIME_SEC: f64 = 7_883.5;

fn mm(value: f64) -> PositiveFiniteMm {
    PositiveFiniteMm::new(value).unwrap()
}

fn sec(value: f64) -> PositiveFiniteSec {
    PositiveFiniteSec::new(value).unwrap()
}

fn wait(value: f64) -> NonNegativeFiniteSec {
    NonNegativeFiniteSec::new(value).unwrap()
}

fn speed(value: f64) -> PositiveFiniteMmPerMin {
    PositiveFiniteMmPerMin::new(value).unwrap()
}

fn raster(aa: AaLevel) -> RasterSetup {
    let profile = SliceProfile::new(
        BuildVolume::new(PLATE_X_MM, PLATE_Y_MM, PLATE_Z_MM).unwrap(),
        PixelResolution::new(RESOLUTION_X, RESOLUTION_Y).unwrap(),
        LayerSchedule::uniform(LAYER_HEIGHT_MM, LAYER_HEIGHT_MM, 3).unwrap(),
        Mirror { x: true, y: false },
        aa,
        PackingMode::None,
    );
    RasterSetup::from_profile(&profile)
}

/// A layer whose every slot holds a distinct value, so a test can
/// tell which record and which slot a header field came from.
fn layer(exposure_sec: f64, waits: [f64; 3], lift_speed: f64, pwm: LightPwm) -> LayerProgram {
    LayerProgram {
        slab_top_mm: mm(LAYER_HEIGHT_MM),
        exposure_sec: sec(exposure_sec),
        wait_before_cure_sec: wait(waits[0]),
        wait_after_cure_sec: wait(waits[1]),
        wait_after_lift_sec: wait(waits[2]),
        lift: MotionStage {
            distance_mm: mm(5.0),
            speed_mm_min: speed(lift_speed),
        },
        retract: MotionStage {
            distance_mm: mm(6.0),
            speed_mm_min: speed(150.0),
        },
        second_lift: None,
        second_retract: None,
        light_pwm: pwm,
    }
}

fn program(layers: Vec<LayerProgram>) -> PrintProgram {
    PrintProgram {
        raster: raster(AaLevel::Off),
        machine_name: NonEmptyString::new("Mars 5").unwrap(),
        material_name: NonEmptyString::new("Blu").unwrap(),
        layers,
        layer_height_mm: mm(LAYER_HEIGHT_MM),
        bottom_layer_count: 8,
        transition_layer_count: 5,
        volume_mm3: VolumeMm3::new(VOLUME_MM3).unwrap(),
        mass_g: MassG::new(MASS_G).unwrap(),
        print_time_sec: wait(PRINT_TIME_SEC),
        created_at: CreationTime {
            rendered: NonEmptyString::new("2026-09-04 17:31:02").unwrap(),
            unix_sec: 1_788_708_662,
        },
        slicer_version: "0.0.12",
    }
}

/// A bottom layer and a normal layer whose slots differ throughout.
fn two_layer_program() -> PrintProgram {
    program(vec![
        layer(30.0, [0.1, 0.2, 0.3], 90.0, LightPwm::new(50.0).unwrap()),
        layer(2.5, [0.4, 0.5, 0.6], 120.0, LightPwm::FULL),
    ])
}

fn header_of(program: &PrintProgram) -> Vec<u8> {
    let mut out = Vec::new();
    write_header(program, None, &mut out).unwrap();
    out
}

const SMALL_PREVIEW_AT: usize = 194;
const SMALL_PREVIEW_BYTES: usize = 116 * 116 * 2;
const LARGE_PREVIEW_AT: usize = SMALL_PREVIEW_AT + SMALL_PREVIEW_BYTES + 2;
const LARGE_PREVIEW_BYTES: usize = 290 * 290 * 2;

/// An image of one colour, at the size this container fixes for the
/// slot it goes in.
fn filled(width: u32, height: u32, rgb: [u8; 3]) -> PreviewImage {
    let pixels = (width * height) as usize;
    PreviewImage::new(width, height, rgb.repeat(pixels)).unwrap()
}

fn previews(small: [u8; 3], large: [u8; 3]) -> PrintPreviews {
    PrintPreviews {
        small: filled(116, 116, small),
        large: filled(290, 290, large),
    }
}

fn header_with(program: &PrintProgram, previews: &PrintPreviews) -> Vec<u8> {
    let sized = SizedPreviews::check(previews).unwrap();
    let mut out = Vec::new();
    write_header(program, Some(sized), &mut out).unwrap();
    out
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn f32_at(bytes: &[u8], offset: usize) -> f32 {
    f32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn text_at(bytes: &[u8], offset: usize, width: usize) -> String {
    let slot = &bytes[offset..offset + width];
    let end = slot.iter().position(|byte| *byte == 0).unwrap_or(width);
    String::from_utf8(slot[..end].to_vec()).unwrap()
}

#[test]
fn the_header_is_195_477_bytes() {
    assert_eq!(header_of(&two_layer_program()).len(), 195_477);
    assert_eq!(HEADER_SIZE, 195_477);
}

#[test]
fn the_version_string_and_magic_open_the_file() {
    let bytes = header_of(&two_layer_program());
    assert_eq!(&bytes[0..4], b"V3.0");
    assert_eq!(&bytes[4..12], &FILE_MAGIC);
}

#[test]
fn the_previews_are_black_and_delimited_at_their_offsets() {
    let bytes = header_of(&two_layer_program());
    assert!(bytes[194..27_106].iter().all(|byte| *byte == 0));
    assert_eq!(&bytes[27_106..27_108], &[0x0D, 0x0A]);
    assert!(bytes[27_108..195_308].iter().all(|byte| *byte == 0));
    assert_eq!(&bytes[195_308..195_310], &[0x0D, 0x0A]);
}

#[test]
fn the_names_land_in_their_fixed_width_slots() {
    let bytes = header_of(&two_layer_program());
    assert_eq!(text_at(&bytes, 12, 32), "SoulCrafted Slicer");
    assert_eq!(text_at(&bytes, 44, 24), "0.0.12");
    assert_eq!(text_at(&bytes, 68, 24), "2026-09-04 17:31:02");
    assert_eq!(text_at(&bytes, 92, 32), "Mars 5");
    assert_eq!(text_at(&bytes, 156, 32), "Blu");
}

#[test]
fn the_printer_type_repeats_the_printer_name() {
    let bytes = header_of(&two_layer_program());
    assert_eq!(text_at(&bytes, 124, 32), text_at(&bytes, 92, 32));
}

#[test]
fn a_name_longer_than_its_slot_keeps_a_terminator() {
    let mut long = two_layer_program();
    long.material_name = NonEmptyString::new(&"R".repeat(64)).unwrap();
    let bytes = header_of(&long);

    assert_eq!(text_at(&bytes, 156, 32).len(), 31);
    assert_eq!(bytes[156 + 31], 0);
}

#[test]
fn the_anti_aliasing_field_records_the_super_sampling_factor() {
    for (aa, factor) in [
        (AaLevel::Off, 1),
        (AaLevel::X2, 2),
        (AaLevel::X4, 4),
        (AaLevel::X8, 8),
    ] {
        let mut sampled = two_layer_program();
        sampled.raster = raster(aa);
        assert_eq!(u16_at(&header_of(&sampled), 188), factor, "{aa:?}");
    }
}

#[test]
fn the_grey_and_blur_levels_are_zero() {
    let bytes = header_of(&two_layer_program());
    assert_eq!(u16_at(&bytes, 190), 0);
    assert_eq!(u16_at(&bytes, 192), 0);
}

#[test]
fn the_layer_count_and_resolution_come_from_the_program() {
    let bytes = header_of(&two_layer_program());
    assert_eq!(u32_at(&bytes, 195_310), 2);
    assert_eq!(u16_at(&bytes, 195_314), 4098);
    assert_eq!(u16_at(&bytes, 195_316), 2560);
}

#[test]
fn the_mirror_flags_are_written_one_byte_each() {
    let bytes = header_of(&two_layer_program());
    assert_eq!(bytes[195_318], 1);
    assert_eq!(bytes[195_319], 0);
}

#[test]
fn the_plate_size_and_layer_height_are_big_endian_floats() {
    let bytes = header_of(&two_layer_program());
    assert_eq!(&bytes[195_320..195_324], &(PLATE_X_MM as f32).to_be_bytes());
    assert_eq!(f32_at(&bytes, 195_324), PLATE_Y_MM as f32);
    assert_eq!(f32_at(&bytes, 195_328), PLATE_Z_MM as f32);
    assert_eq!(f32_at(&bytes, 195_332), LAYER_HEIGHT_MM as f32);
}

#[test]
fn a_float_is_not_written_little_endian() {
    let bytes = header_of(&two_layer_program());
    let little = f32::from_le_bytes(bytes[195_320..195_324].try_into().unwrap());
    assert_ne!(little, PLATE_X_MM as f32);
}

#[test]
fn the_exposures_come_from_the_first_and_last_records() {
    let bytes = header_of(&two_layer_program());
    assert_eq!(f32_at(&bytes, 195_336), 2.5);
    assert_eq!(f32_at(&bytes, 195_369), 30.0);
}

#[test]
fn the_delay_mode_selects_the_wait_times() {
    let bytes = header_of(&two_layer_program());
    assert_eq!(bytes[195_340], 1);
    assert_eq!(f32_at(&bytes, 195_341), 0.0);
}

#[test]
fn the_waits_follow_the_slot_names_the_header_uses() {
    let bytes = header_of(&two_layer_program());
    // The bottom record holds before-cure 0.1, after-cure 0.2 and
    // after-lift 0.3; the header orders them before lift, after lift,
    // after retract.
    assert_eq!(f32_at(&bytes, 195_345), 0.2);
    assert_eq!(f32_at(&bytes, 195_349), 0.3);
    assert_eq!(f32_at(&bytes, 195_353), 0.1);
    assert_eq!(f32_at(&bytes, 195_357), 0.5);
    assert_eq!(f32_at(&bytes, 195_361), 0.6);
    assert_eq!(f32_at(&bytes, 195_365), 0.4);
}

#[test]
fn the_bottom_layer_count_comes_from_the_program() {
    assert_eq!(u32_at(&header_of(&two_layer_program()), 195_373), 8);
}

#[test]
fn the_motion_rows_come_from_the_first_and_last_records() {
    let bytes = header_of(&two_layer_program());
    assert_eq!(f32_at(&bytes, 195_377), 5.0);
    assert_eq!(f32_at(&bytes, 195_381), 90.0);
    assert_eq!(f32_at(&bytes, 195_385), 5.0);
    assert_eq!(f32_at(&bytes, 195_389), 120.0);
    assert_eq!(f32_at(&bytes, 195_393), 6.0);
    assert_eq!(f32_at(&bytes, 195_397), 150.0);
    assert_eq!(f32_at(&bytes, 195_401), 6.0);
    assert_eq!(f32_at(&bytes, 195_405), 150.0);
}

#[test]
fn a_layer_without_a_second_stage_zeroes_those_slots() {
    let bytes = header_of(&two_layer_program());
    for offset in (195_409..195_441).step_by(4) {
        assert_eq!(f32_at(&bytes, offset), 0.0, "offset {offset}");
    }
}

#[test]
fn a_second_stage_reaches_its_slots() {
    let staged = MotionStage {
        distance_mm: mm(2.0),
        speed_mm_min: speed(60.0),
    };
    let mut program = two_layer_program();
    program.layers[0].second_lift = Some(staged);
    program.layers[0].second_retract = Some(staged);
    let bytes = header_of(&program);

    assert_eq!(f32_at(&bytes, 195_409), 2.0);
    assert_eq!(f32_at(&bytes, 195_413), 60.0);
    assert_eq!(f32_at(&bytes, 195_425), 2.0);
    assert_eq!(f32_at(&bytes, 195_429), 60.0);
}

#[test]
fn without_previews_both_images_are_black() {
    let bytes = header_of(&two_layer_program());

    let small = &bytes[SMALL_PREVIEW_AT..SMALL_PREVIEW_AT + SMALL_PREVIEW_BYTES];
    let large = &bytes[LARGE_PREVIEW_AT..LARGE_PREVIEW_AT + LARGE_PREVIEW_BYTES];
    assert!(small.iter().all(|byte| *byte == 0));
    assert!(large.iter().all(|byte| *byte == 0));
}

#[test]
fn a_supplied_preview_fills_its_slot_as_rgb565() {
    let bytes = header_with(&two_layer_program(), &previews([0xFF, 0, 0], [0, 0xFF, 0]));

    let small = &bytes[SMALL_PREVIEW_AT..SMALL_PREVIEW_AT + SMALL_PREVIEW_BYTES];
    let large = &bytes[LARGE_PREVIEW_AT..LARGE_PREVIEW_AT + LARGE_PREVIEW_BYTES];
    assert!(
        small
            .as_chunks::<2>()
            .0
            .iter()
            .all(|pixel| *pixel == [0xF8, 0x00])
    );
    assert!(
        large
            .as_chunks::<2>()
            .0
            .iter()
            .all(|pixel| *pixel == [0x07, 0xE0])
    );
}

#[test]
fn the_channels_drop_the_bits_rgb565_has_no_room_for() {
    // 0x12 keeps its top 5 bits, 0x34 its top 6 and 0x56 its top 5,
    // which pack to 0b00010_001101_01010.
    let bytes = header_with(
        &two_layer_program(),
        &previews([0x12, 0x34, 0x56], [0, 0, 0]),
    );

    assert_eq!(&bytes[SMALL_PREVIEW_AT..SMALL_PREVIEW_AT + 2], [0x11, 0xAA],);
}

#[test]
fn the_delimiters_still_close_both_images() {
    let bytes = header_with(&two_layer_program(), &previews([0xFF, 0, 0], [0, 0xFF, 0]));

    assert_eq!(
        &bytes[SMALL_PREVIEW_AT + SMALL_PREVIEW_BYTES..LARGE_PREVIEW_AT],
        [0x0D, 0x0A],
    );
    let after_large = LARGE_PREVIEW_AT + LARGE_PREVIEW_BYTES;
    assert_eq!(&bytes[after_large..after_large + 2], [0x0D, 0x0A]);
}

#[test]
fn a_preview_of_another_size_is_refused_for_its_slot() {
    let wrong = PrintPreviews {
        small: filled(116, 116, [0, 0, 0]),
        large: filled(290, 289, [0, 0, 0]),
    };

    let err = SizedPreviews::check(&wrong).unwrap_err();
    assert_eq!(err.expected_width, 290);
    assert_eq!(err.expected_height, 290);
    assert_eq!(err.actual_height, 289);
}

#[test]
fn the_light_pwm_is_scaled_to_the_byte_range() {
    let bytes = header_of(&two_layer_program());
    assert_eq!(u16_at(&bytes, 195_441), 128);
    assert_eq!(u16_at(&bytes, 195_443), 255);
}

#[test]
fn the_display_only_totals_are_zero() {
    let bytes = header_of(&two_layer_program());
    assert_eq!(bytes[195_445], 0);
    assert_eq!(f32_at(&bytes, 195_458), 0.0);
    assert_eq!(text_at(&bytes, 195_462, 8), "");
}

#[test]
fn the_print_time_comes_from_the_program_truncated_to_whole_seconds() {
    let bytes = header_of(&two_layer_program());
    assert_eq!(u32_at(&bytes, 195_446), 7_883);
}

#[test]
fn a_print_time_beyond_the_field_reports_its_largest_value() {
    let mut long = two_layer_program();
    long.print_time_sec = wait(1e12);
    assert_eq!(u32_at(&header_of(&long), 195_446), u32::MAX);
}

#[test]
fn the_total_volume_comes_from_the_program() {
    let bytes = header_of(&two_layer_program());
    assert_eq!(f32_at(&bytes, 195_450), VOLUME_MM3 as f32);
}

#[test]
fn an_empty_plate_reports_no_volume() {
    let mut empty = two_layer_program();
    empty.volume_mm3 = VolumeMm3::ZERO;
    assert_eq!(f32_at(&header_of(&empty), 195_450), 0.0);
}

#[test]
fn the_total_weight_comes_from_the_program() {
    let bytes = header_of(&two_layer_program());
    assert_eq!(f32_at(&bytes, 195_454), MASS_G as f32);
}

#[test]
fn an_empty_plate_reports_no_weight() {
    let mut empty = two_layer_program();
    empty.mass_g = MassG::ZERO;
    assert_eq!(f32_at(&header_of(&empty), 195_454), 0.0);
}

#[test]
fn the_layer_content_offset_is_the_header_size() {
    let bytes = header_of(&two_layer_program());
    assert_eq!(u32_at(&bytes, 195_470), HEADER_SIZE);
    assert_eq!(u32_at(&bytes, 195_470) as usize, bytes.len());
}

#[test]
fn the_grey_scale_level_declares_the_whole_byte_range() {
    assert_eq!(header_of(&two_layer_program())[195_474], 1);
}

#[test]
fn the_transition_count_closes_the_header() {
    assert_eq!(u16_at(&header_of(&two_layer_program()), 195_475), 5);
}

#[test]
fn a_program_with_no_layers_still_writes_a_whole_header() {
    let bytes = header_of(&program(Vec::new()));
    assert_eq!(bytes.len(), 195_477);
    assert_eq!(u32_at(&bytes, 195_310), 0);
    assert_eq!(f32_at(&bytes, 195_336), 0.0);
    assert_eq!(f32_at(&bytes, 195_369), 0.0);
}

#[test]
fn a_single_layer_program_fills_both_rows_from_it() {
    let only = layer(20.0, [0.7, 0.8, 0.9], 100.0, LightPwm::FULL);
    let bytes = header_of(&program(vec![only]));

    assert_eq!(f32_at(&bytes, 195_336), 20.0);
    assert_eq!(f32_at(&bytes, 195_369), 20.0);
    assert_eq!(f32_at(&bytes, 195_381), 100.0);
    assert_eq!(f32_at(&bytes, 195_389), 100.0);
}
