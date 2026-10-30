/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::error::Error as _;

use sc_print_primitives::{
    AaLevel, BuildVolume, LightPwm, MassG, Mirror, NonEmptyString, NonNegativeFiniteSec,
    PixelResolution, PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec, VolumeMm3,
};
use sc_sl1cer::contour::{ClosedRing, ContourLayer, ContourStack, Polygon};
use sc_sl1cer::geom2d::Point2D;
use sc_sl1cer::profile::{LayerSchedule, PackingMode, SliceProfile};

use super::file::write_file;
use super::header::{FILE_MAGIC, HEADER_SIZE};
use super::record::RECORD_SIZE;
use super::test_decode::decode_chunks;
use crate::error::WriteFailure;
use crate::preview::{PreviewImage, PrintPreviews};
use crate::print_program::{CreationTime, LayerProgram, MotionStage, PrintProgram};
use crate::raster_setup::RasterSetup;

const PLATE_MM: f64 = 10.0;
const PLATE_Z_MM: f64 = 100.0;
const PIXELS_PER_SIDE: u32 = 20;
const LAYER_HEIGHT_MM: f64 = 0.1;
const FOOTER_SIZE: usize = 11;

fn mm(value: f64) -> PositiveFiniteMm {
    PositiveFiniteMm::new(value).unwrap()
}

fn raster() -> RasterSetup {
    let profile = SliceProfile::new(
        BuildVolume::new(PLATE_MM, PLATE_MM, PLATE_Z_MM).unwrap(),
        PixelResolution::new(PIXELS_PER_SIDE, PIXELS_PER_SIDE).unwrap(),
        LayerSchedule::uniform(LAYER_HEIGHT_MM, LAYER_HEIGHT_MM, 3).unwrap(),
        Mirror::default(),
        AaLevel::Off,
        PackingMode::None,
    );
    RasterSetup::from_profile(&profile)
}

fn layer(slab_top_mm: f64, exposure_sec: f64) -> LayerProgram {
    LayerProgram {
        slab_top_mm: mm(slab_top_mm),
        exposure_sec: PositiveFiniteSec::new(exposure_sec).unwrap(),
        wait_before_cure_sec: NonNegativeFiniteSec::ZERO,
        wait_after_cure_sec: NonNegativeFiniteSec::ZERO,
        wait_after_lift_sec: NonNegativeFiniteSec::ZERO,
        lift: MotionStage {
            distance_mm: mm(5.0),
            speed_mm_min: PositiveFiniteMmPerMin::new(90.0).unwrap(),
        },
        retract: MotionStage {
            distance_mm: mm(5.0),
            speed_mm_min: PositiveFiniteMmPerMin::new(150.0).unwrap(),
        },
        second_lift: None,
        second_retract: None,
        light_pwm: LightPwm::FULL,
    }
}

fn program(layers: Vec<LayerProgram>) -> PrintProgram {
    PrintProgram {
        raster: raster(),
        machine_name: NonEmptyString::new("Mars 5").unwrap(),
        material_name: NonEmptyString::new("Blu").unwrap(),
        layers,
        layer_height_mm: mm(LAYER_HEIGHT_MM),
        bottom_layer_count: 1,
        transition_layer_count: 0,
        volume_mm3: VolumeMm3::ZERO,
        mass_g: MassG::ZERO,
        print_time_sec: NonNegativeFiniteSec::ZERO,
        created_at: CreationTime {
            rendered: NonEmptyString::new("2026-09-04 20:41:00").unwrap(),
            unix_sec: 1_788_719_660,
        },
        slicer_version: "0.0.12",
    }
}

/// An axis-aligned rectangle in printer-frame millimetres.
fn rect_layer(z_mm: f64, x: (f64, f64), y: (f64, f64)) -> ContourLayer {
    let ring = ClosedRing::new(vec![
        Point2D::new(x.0, y.0),
        Point2D::new(x.1, y.0),
        Point2D::new(x.1, y.1),
        Point2D::new(x.0, y.1),
    ]);
    ContourLayer::new(z_mm, vec![Polygon::new_unchecked(ring, Vec::new())])
}

/// 3 layers whose rectangles differ in size, so their images differ
/// in length too.
fn three_layer_case() -> (PrintProgram, ContourStack) {
    let program = program(vec![layer(0.1, 30.0), layer(0.2, 2.5), layer(0.3, 2.5)]);
    let stack = ContourStack::new_unchecked(vec![
        rect_layer(0.05, (-4.0, 4.0), (-4.0, 4.0)),
        rect_layer(0.15, (-2.0, 2.0), (-2.0, 2.0)),
        rect_layer(0.25, (-1.0, 1.0), (-1.0, 1.0)),
    ]);
    (program, stack)
}

fn written(program: &PrintProgram, stack: &ContourStack) -> Vec<u8> {
    let mut out = Vec::new();
    write_file(program, None, stack, &mut out).unwrap();
    out
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn f32_at(bytes: &[u8], offset: usize) -> f32 {
    f32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

/// Walk the layer chain by the stride each record declares,
/// returning where each record starts and where the walk ends.
fn walk_records(bytes: &[u8], layer_count: usize) -> (Vec<usize>, usize) {
    let mut starts = Vec::with_capacity(layer_count);
    let mut cursor = HEADER_SIZE as usize;
    for _ in 0..layer_count {
        starts.push(cursor);
        let image_length = u32_at(bytes, cursor + 66) as usize;
        cursor += RECORD_SIZE + image_length + 2;
    }
    (starts, cursor)
}

fn image_of(bytes: &[u8], record_start: usize) -> &[u8] {
    let image_length = u32_at(bytes, record_start + 66) as usize;
    let image_start = record_start + RECORD_SIZE;
    &bytes[image_start..image_start + image_length]
}

/// Expand one layer image back into pixels, row 0 first.
fn pixels_of(image: &[u8]) -> Vec<u8> {
    let mut pixels = Vec::new();
    for (value, length) in decode_chunks(&image[1..image.len() - 1]) {
        pixels.extend(std::iter::repeat_n(value, length as usize));
    }
    pixels
}

fn checksum_of(body: &[u8]) -> u8 {
    !body.iter().fold(0_u8, |sum, byte| sum.wrapping_add(*byte))
}

#[test]
fn walking_the_record_strides_lands_on_the_footer() {
    let (program, stack) = three_layer_case();
    let bytes = written(&program, &stack);
    let (starts, end) = walk_records(&bytes, program.layers.len());

    assert_eq!(starts.len(), 3);
    assert_eq!(end, bytes.len() - FOOTER_SIZE);
}

#[test]
fn the_magic_opens_the_header_and_closes_the_file() {
    let (program, stack) = three_layer_case();
    let bytes = written(&program, &stack);

    assert_eq!(&bytes[4..12], &FILE_MAGIC);
    assert_eq!(
        &bytes[bytes.len() - FOOTER_SIZE..bytes.len() - 8],
        &[0, 0, 0]
    );
    assert_eq!(&bytes[bytes.len() - 8..], &FILE_MAGIC);
}

#[test]
fn every_layer_image_carries_a_checksum_over_its_body() {
    let (program, stack) = three_layer_case();
    let bytes = written(&program, &stack);
    let (starts, _) = walk_records(&bytes, program.layers.len());

    for (index, start) in starts.iter().enumerate() {
        let image = image_of(&bytes, *start);
        assert_eq!(image[0], 0x55, "layer {index}");
        assert_eq!(
            image[image.len() - 1],
            checksum_of(&image[1..image.len() - 1]),
            "layer {index}",
        );
    }
}

#[test]
fn every_layer_image_covers_the_whole_frame() {
    let (program, stack) = three_layer_case();
    let bytes = written(&program, &stack);
    let (starts, _) = walk_records(&bytes, program.layers.len());
    let frame = (PIXELS_PER_SIDE * PIXELS_PER_SIDE) as usize;

    for (index, start) in starts.iter().enumerate() {
        assert_eq!(
            pixels_of(image_of(&bytes, *start)).len(),
            frame,
            "layer {index}"
        );
    }
}

#[test]
fn a_record_carries_its_own_slab_top_and_exposure() {
    let (program, stack) = three_layer_case();
    let bytes = written(&program, &stack);
    let (starts, _) = walk_records(&bytes, program.layers.len());

    for (index, start) in starts.iter().enumerate() {
        let layer = &program.layers[index];
        assert_eq!(f32_at(&bytes, start + 6), layer.slab_top_mm.value() as f32);
        assert_eq!(
            f32_at(&bytes, start + 10),
            layer.exposure_sec.value() as f32
        );
    }
}

#[test]
fn a_record_parks_a_pause_at_the_machine_ceiling() {
    let (program, stack) = three_layer_case();
    let bytes = written(&program, &stack);
    let (starts, _) = walk_records(&bytes, program.layers.len());

    assert_eq!(f32_at(&bytes, starts[0] + 2), PLATE_Z_MM as f32);
}

#[test]
fn the_images_shrink_with_the_shapes_that_produced_them() {
    let (program, stack) = three_layer_case();
    let bytes = written(&program, &stack);
    let (starts, _) = walk_records(&bytes, program.layers.len());
    let ink = |start: &usize| -> usize {
        pixels_of(image_of(&bytes, *start))
            .iter()
            .filter(|value| **value == 255)
            .count()
    };

    assert!(ink(&starts[0]) > ink(&starts[1]));
    assert!(ink(&starts[1]) > ink(&starts[2]));
}

#[test]
fn the_first_rows_written_are_the_plate_minimum_y_and_columns_its_maximum_x() {
    // A rectangle over x in [-4, -2] and y in [-4, -3] mm. The
    // projection puts the plate's minimum corner at pixel (0, 0), so
    // it covers the 3rd to 6th columns from the plate's minimum X and
    // the 3rd and 4th rows from its minimum Y. The container stores
    // rows from the minimum Y and columns from the maximum X, which
    // puts those at rows 2 and 3 and columns 14 to 17 of 20.
    let program = program(vec![layer(0.1, 2.5)]);
    let stack = ContourStack::new_unchecked(vec![rect_layer(0.05, (-4.0, -2.0), (-4.0, -3.0))]);
    let bytes = written(&program, &stack);
    let (starts, _) = walk_records(&bytes, 1);
    let pixels = pixels_of(image_of(&bytes, starts[0]));
    let width = PIXELS_PER_SIDE as usize;

    let mut rows = Vec::new();
    let mut columns = Vec::new();
    for row in 0..width {
        for column in 0..width {
            if pixels[row * width + column] == 255 {
                if !rows.contains(&row) {
                    rows.push(row);
                }
                if !columns.contains(&column) {
                    columns.push(column);
                }
            }
        }
    }

    assert_eq!(rows, vec![2, 3]);
    assert_eq!(columns, vec![14, 15, 16, 17]);
}

#[test]
fn an_empty_program_writes_a_header_and_a_footer() {
    let bytes = written(
        &program(Vec::new()),
        &ContourStack::new_unchecked(Vec::new()),
    );
    assert_eq!(bytes.len(), HEADER_SIZE as usize + FOOTER_SIZE);
}

#[test]
fn a_preview_of_another_size_is_refused_before_anything_is_written() {
    let (program, stack) = three_layer_case();
    let wrong = PrintPreviews {
        small: PreviewImage::new(116, 116, vec![0; 116 * 116 * 3]).unwrap(),
        large: PreviewImage::new(64, 64, vec![0; 64 * 64 * 3]).unwrap(),
    };
    let mut out = Vec::new();

    let failure = write_file(&program, Some(&wrong), &stack, &mut out).unwrap_err();

    assert!(
        matches!(failure, WriteFailure::PreviewSize(err) if err.actual_width == 64),
        "failure: {failure:?}",
    );
    assert!(out.is_empty(), "{} bytes were written", out.len());
}

#[test]
fn a_supplied_preview_reaches_the_file_without_moving_anything_after_it() {
    let (program, stack) = three_layer_case();
    let previews = PrintPreviews {
        small: PreviewImage::new(116, 116, [0xFF, 0xFF, 0xFF].repeat(116 * 116)).unwrap(),
        large: PreviewImage::new(290, 290, [0xFF, 0xFF, 0xFF].repeat(290 * 290)).unwrap(),
    };
    let mut with_preview = Vec::new();
    write_file(&program, Some(&previews), &stack, &mut with_preview).unwrap();

    let black = written(&program, &stack);
    assert_eq!(with_preview.len(), black.len());
    assert_eq!(
        with_preview[HEADER_SIZE as usize..],
        black[HEADER_SIZE as usize..],
    );
    assert_eq!(&with_preview[194..196], [0xFF, 0xFF]);
}

#[test]
fn a_stack_that_does_not_match_the_program_is_refused() {
    let (_, stack) = three_layer_case();
    let short = program(vec![layer(0.1, 2.5), layer(0.2, 2.5)]);
    let mut out = Vec::new();
    let failure = write_file(&short, None, &stack, &mut out).unwrap_err();

    assert!(
        matches!(
            failure,
            WriteFailure::LayerCountMismatch {
                program_layers: 2,
                stack_layers: 3,
            },
        ),
        "{failure:?}",
    );
    assert!(out.is_empty());
}

#[test]
fn the_mismatch_message_reports_both_counts() {
    let message = WriteFailure::LayerCountMismatch {
        program_layers: 2,
        stack_layers: 3,
    }
    .to_string();

    assert!(message.contains("2 layer(s)"), "{message}");
    assert!(message.contains("stack 3"), "{message}");
}

#[test]
fn a_write_failure_carries_its_io_error() {
    let failure = WriteFailure::from(std::io::Error::other("disk full"));
    assert!(failure.to_string().contains("disk full"));
    assert!(failure.source().is_some());
    assert!(
        WriteFailure::LayerCountMismatch {
            program_layers: 1,
            stack_layers: 0,
        }
        .source()
        .is_none(),
    );
}
