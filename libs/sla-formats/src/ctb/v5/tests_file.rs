/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! No plain file of this format has ever been read by this project,
//! so these are self-consistency checks: every offset the file
//! records is followed and has to land on the section it names.
//! Agreement with a vendor file is not among them.

use std::io::Cursor;

use sc_print_primitives::{
    AaLevel, BuildVolume, LightPwm, MassG, Mirror, NonEmptyString, NonNegativeFiniteSec,
    PixelResolution, PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec, VolumeMm3,
};
use sc_sl1cer::contour::{ClosedRing, ContourLayer, ContourStack, Polygon};
use sc_sl1cer::geom2d::Point2D;
use sc_sl1cer::profile::{LayerSchedule, PackingMode, SliceProfile};

use super::file::write_file;
use super::layout::{
    DISCLAIMER_SIZE, HEADER_SIZE, LAYER_RECORD_SIZE, LAYER_TABLE_RECORD_SIZE,
    PRINT_PARAMETERS_SIZE, PRINT_PARAMETERS_V4_SIZE, SLICER_INFO_SIZE,
};
use super::preview_record::PREVIEW_SIZES;
use crate::error::WriteFailure;
use crate::preview::{PreviewImage, PreviewSize, PrintPreviews};
use crate::print_program::{CreationTime, LayerProgram, MotionStage, PrintProgram};
use crate::raster_setup::RasterSetup;

const PLATE_MM: f64 = 10.0;
const PLATE_Z_MM: f64 = 100.0;
const PIXELS_PER_SIDE: u32 = 20;
const LAYER_HEIGHT_MM: f64 = 0.1;
const MACHINE: &str = "ELEGOO Jupiter SE";
const CHANNELS: usize = 3;

fn mm(value: f64) -> PositiveFiniteMm {
    PositiveFiniteMm::new(value).unwrap()
}

fn raster() -> RasterSetup {
    let profile = SliceProfile::new(
        BuildVolume::new(PLATE_MM, PLATE_MM, PLATE_Z_MM).unwrap(),
        PixelResolution::new(PIXELS_PER_SIDE, PIXELS_PER_SIDE).unwrap(),
        LayerSchedule::uniform(LAYER_HEIGHT_MM, LAYER_HEIGHT_MM, 3).unwrap(),
        Mirror { x: true, y: false },
        AaLevel::Off,
        PackingMode::None,
    );
    RasterSetup::from_profile(&profile)
}

fn layer(slab_top_mm: f64, exposure_sec: f64) -> LayerProgram {
    LayerProgram {
        slab_top_mm: mm(slab_top_mm),
        exposure_sec: PositiveFiniteSec::new(exposure_sec).unwrap(),
        wait_before_cure_sec: NonNegativeFiniteSec::new(0.5).unwrap(),
        wait_after_cure_sec: NonNegativeFiniteSec::new(0.25).unwrap(),
        wait_after_lift_sec: NonNegativeFiniteSec::new(1.25).unwrap(),
        lift: MotionStage {
            distance_mm: mm(5.5),
            speed_mm_min: PositiveFiniteMmPerMin::new(91.0).unwrap(),
        },
        retract: MotionStage {
            distance_mm: mm(6.5),
            speed_mm_min: PositiveFiniteMmPerMin::new(137.0).unwrap(),
        },
        second_lift: Some(MotionStage {
            distance_mm: mm(2.75),
            speed_mm_min: PositiveFiniteMmPerMin::new(243.0).unwrap(),
        }),
        second_retract: None,
        light_pwm: LightPwm::FULL,
    }
}

fn program(layers: Vec<LayerProgram>) -> PrintProgram {
    PrintProgram {
        raster: raster(),
        machine_name: NonEmptyString::new(MACHINE).unwrap(),
        material_name: NonEmptyString::new("Magna Durable").unwrap(),
        layers,
        layer_height_mm: mm(LAYER_HEIGHT_MM),
        bottom_layer_count: 1,
        transition_layer_count: 0,
        volume_mm3: VolumeMm3::new(15_000.0).unwrap(),
        mass_g: MassG::new(17.5).unwrap(),
        print_time_sec: NonNegativeFiniteSec::new(5664.0).unwrap(),
        created_at: CreationTime {
            rendered: NonEmptyString::new("2026-09-04 21:07:00").unwrap(),
            unix_sec: 1_788_721_620,
        },
        slicer_version: "0.0.12",
    }
}

/// An axis-aligned rectangle in printer-frame millimetres.
fn rect_layer(z_mm: f64, half_side: f64) -> ContourLayer {
    let ring = ClosedRing::new(vec![
        Point2D::new(-half_side, -half_side),
        Point2D::new(half_side, -half_side),
        Point2D::new(half_side, half_side),
        Point2D::new(-half_side, half_side),
    ]);
    ContourLayer::new(z_mm, vec![Polygon::new_unchecked(ring, Vec::new())])
}

/// 3 layers whose rectangles differ in size, so their images differ
/// in length too.
fn three_layer_case() -> (PrintProgram, ContourStack) {
    let program = program(vec![layer(0.1, 30.0), layer(0.2, 2.5), layer(0.3, 2.5)]);
    let stack = ContourStack::new_unchecked(vec![
        rect_layer(0.05, 4.0),
        rect_layer(0.15, 2.0),
        rect_layer(0.25, 1.0),
    ]);
    (program, stack)
}

fn frame(size: PreviewSize) -> PreviewImage {
    let pixels = size.width as usize * size.height as usize;
    PreviewImage::new(size.width, size.height, vec![0x80; pixels * CHANNELS]).unwrap()
}

fn previews() -> PrintPreviews {
    PrintPreviews {
        small: frame(PREVIEW_SIZES.small),
        large: frame(PREVIEW_SIZES.large),
    }
}

fn written(program: &PrintProgram, stack: &ContourStack) -> Vec<u8> {
    write_with(program, stack, None)
}

fn write_with(
    program: &PrintProgram,
    stack: &ContourStack,
    previews: Option<&PrintPreviews>,
) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    write_file(program, previews, stack, &mut out).unwrap();
    out.into_inner()
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

/// Follow the layer table, returning each entry's image address and
/// length as the file records them.
fn table_entries(bytes: &[u8]) -> Vec<(usize, usize)> {
    let table = u32_at(bytes, 64) as usize;
    let count = u32_at(bytes, 68) as usize;
    (0..count)
        .map(|index| {
            let entry = table + index * LAYER_TABLE_RECORD_SIZE as usize;
            let page = u32_at(bytes, entry + 20) as usize;
            let offset = u32_at(bytes, entry + 12) as usize;
            (
                page * (1usize << 32) + offset,
                u32_at(bytes, entry + 16) as usize,
            )
        })
        .collect()
}

#[test]
fn the_file_opens_with_a_whole_header() {
    let (program, stack) = three_layer_case();
    let bytes = written(&program, &stack);

    assert_eq!(u32_at(&bytes, 0), 0x12FD_0106);
    assert!(bytes.len() > HEADER_SIZE as usize);
}

#[test]
fn every_offset_the_header_records_lands_inside_the_file() {
    let (program, stack) = three_layer_case();
    let bytes = written(&program, &stack);

    for offset in [60, 64, 72, 84, 104] {
        let target = u32_at(&bytes, offset) as usize;
        assert!(target < bytes.len(), "header offset {offset} -> {target}");
    }
}

#[test]
fn the_preview_records_locate_blobs_that_lie_inside_the_file() {
    let (program, stack) = three_layer_case();
    let bytes = write_with(&program, &stack, Some(&previews()));

    for (record_at, size) in [
        (u32_at(&bytes, 60) as usize, PREVIEW_SIZES.large),
        (u32_at(&bytes, 72) as usize, PREVIEW_SIZES.small),
    ] {
        assert_eq!(u32_at(&bytes, record_at), size.width);
        assert_eq!(u32_at(&bytes, record_at + 4), size.height);
        let blob = u32_at(&bytes, record_at + 8) as usize;
        let length = u32_at(&bytes, record_at + 12) as usize;
        assert_eq!(blob, record_at + 32, "the blob follows its record");
        assert!(blob + length <= bytes.len());
    }
}

#[test]
fn a_file_without_previews_still_carries_both_records() {
    let (program, stack) = three_layer_case();
    let bytes = written(&program, &stack);

    for record_at in [u32_at(&bytes, 60) as usize, u32_at(&bytes, 72) as usize] {
        assert!(u32_at(&bytes, record_at) > 0, "a width is still declared");
        assert!(u32_at(&bytes, record_at + 12) > 0, "a blob is still there");
    }
}

#[test]
fn the_chain_of_offsets_reaches_every_fixed_section_in_order() {
    let (program, stack) = three_layer_case();
    let bytes = written(&program, &stack);

    let print_parameters = u32_at(&bytes, 84) as usize;
    let slicer_info = u32_at(&bytes, 104) as usize;
    assert_eq!(
        slicer_info,
        print_parameters + PRINT_PARAMETERS_SIZE as usize
    );

    let machine_name = u32_at(&bytes, slicer_info + 28) as usize;
    let machine_name_size = u32_at(&bytes, slicer_info + 32) as usize;
    assert_eq!(machine_name, slicer_info + SLICER_INFO_SIZE as usize);
    assert_eq!(
        &bytes[machine_name..machine_name + machine_name_size],
        MACHINE.as_bytes(),
    );

    let v4 = u32_at(&bytes, slicer_info + 64) as usize;
    let disclaimer = u32_at(&bytes, v4 + 72) as usize;
    assert_eq!(disclaimer, machine_name + machine_name_size);
    assert_eq!(u32_at(&bytes, v4 + 76), DISCLAIMER_SIZE);
    assert_eq!(v4, disclaimer + DISCLAIMER_SIZE as usize);
    assert!(
        bytes[disclaimer..].starts_with(b"Layout and record format for the ctb"),
        "the disclaimer is where the block says it is",
    );

    let resin = u32_at(&bytes, v4 + 80) as usize;
    assert_eq!(resin, v4 + PRINT_PARAMETERS_V4_SIZE as usize);
    assert_eq!(
        u32_at(&bytes, 64) as usize,
        resin + 40 + "Magna Durable".len() * 2 + MACHINE.len(),
        "the layer table follows the resin block",
    );
}

#[test]
fn the_layer_table_holds_one_entry_per_layer() {
    let (program, stack) = three_layer_case();
    let bytes = written(&program, &stack);

    assert_eq!(u32_at(&bytes, 68), 3);
    assert_eq!(table_entries(&bytes).len(), 3);
}

#[test]
fn each_image_is_preceded_by_the_record_that_repeats_its_table_entry() {
    let (program, stack) = three_layer_case();
    let bytes = written(&program, &stack);
    let table = u32_at(&bytes, 64) as usize;

    for (index, (image_at, _)) in table_entries(&bytes).iter().enumerate() {
        let record_at = image_at - LAYER_RECORD_SIZE as usize;
        let entry = table + index * LAYER_TABLE_RECORD_SIZE as usize;
        assert_eq!(
            &bytes[record_at..record_at + LAYER_TABLE_RECORD_SIZE as usize],
            &bytes[entry..entry + LAYER_TABLE_RECORD_SIZE as usize],
            "layer {index}",
        );
    }
}

#[test]
fn walking_the_layers_reaches_the_end_with_nothing_left_over() {
    let (program, stack) = three_layer_case();
    let bytes = written(&program, &stack);
    let entries = table_entries(&bytes);

    let mut cursor = u32_at(&bytes, 64) as usize + entries.len() * LAYER_TABLE_RECORD_SIZE as usize;
    for (index, (image_at, length)) in entries.iter().enumerate() {
        assert_eq!(
            cursor + LAYER_RECORD_SIZE as usize,
            *image_at,
            "layer {index} follows the one before it",
        );
        cursor = image_at + length;
    }

    assert_eq!(cursor, bytes.len(), "the last image ends the file");
}

#[test]
fn every_layer_image_is_recorded_with_its_own_length() {
    let (program, stack) = three_layer_case();
    let bytes = written(&program, &stack);
    let entries = table_entries(&bytes);
    let lengths: Vec<usize> = entries.iter().map(|(_, length)| *length).collect();

    assert!(lengths.iter().all(|length| *length > 0));
    assert!(
        lengths[0] != lengths[2],
        "rectangles of different sizes compress differently: {lengths:?}",
    );
}

#[test]
fn the_fuller_record_counts_itself_and_the_image_it_precedes() {
    let (program, stack) = three_layer_case();
    let bytes = written(&program, &stack);

    for (image_at, length) in table_entries(&bytes) {
        let record_at = image_at - LAYER_RECORD_SIZE as usize;
        assert_eq!(
            u32_at(&bytes, record_at + 36) as usize,
            LAYER_RECORD_SIZE as usize + length,
        );
    }
}

#[test]
fn a_program_without_layers_ends_where_its_table_would_start() {
    let program = program(Vec::new());
    let stack = ContourStack::new_unchecked(Vec::new());
    let bytes = written(&program, &stack);

    assert_eq!(u32_at(&bytes, 68), 0);
    assert_eq!(u32_at(&bytes, 64) as usize, bytes.len());
}

#[test]
fn a_program_that_does_not_match_the_stack_is_refused_before_any_byte() {
    let (program, _) = three_layer_case();
    let stack = ContourStack::new_unchecked(vec![rect_layer(0.05, 4.0)]);
    let mut out = Cursor::new(Vec::new());
    let failure = write_file(&program, None, &stack, &mut out).unwrap_err();

    assert!(
        matches!(
            failure,
            WriteFailure::LayerCountMismatch {
                program_layers: 3,
                stack_layers: 1,
            },
        ),
        "{failure}",
    );
    assert!(out.into_inner().is_empty());
}

#[test]
fn a_preview_of_another_size_is_refused_before_any_byte() {
    let (program, stack) = three_layer_case();
    let previews = PrintPreviews {
        small: frame(PreviewSize {
            width: 64,
            height: 64,
        }),
        large: frame(PREVIEW_SIZES.large),
    };
    let mut out = Cursor::new(Vec::new());
    let failure = write_file(&program, Some(&previews), &stack, &mut out).unwrap_err();

    assert!(
        matches!(failure, WriteFailure::PreviewSize(err) if err.actual_width == 64),
        "{failure}",
    );
    assert!(out.into_inner().is_empty());
}
