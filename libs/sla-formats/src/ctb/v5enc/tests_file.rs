/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The file is read back the way a machine reads it: the plaintext
//! header locates the settings block, the settings block is
//! decrypted, and every address it records is followed. Agreement
//! with a vendor file is not among these checks, since no file this
//! writer produced has been put in front of a machine.

use std::io::Cursor;

use crc32fast::Hasher;
use sc_print_primitives::{
    AaLevel, BuildVolume, LightPwm, MassG, Mirror, NonEmptyString, NonNegativeFiniteSec,
    PixelResolution, PositiveFiniteMm, PositiveFiniteMmPerMin, PositiveFiniteSec, VolumeMm3,
};
use sc_sl1cer::contour::{ClosedRing, ContourLayer, ContourStack, Polygon};
use sc_sl1cer::geom2d::Point2D;
use sc_sl1cer::profile::{LayerSchedule, PackingMode, SliceProfile};

use super::file::write_file;
use super::layout::{
    HEADER_SIZE, LAYER_POINTER_SIZE, LAYER_RECORD_SIZE, POST_LAYER_MARKER_SIZE,
    PREVIEW_RECORD_SIZE, SETTINGS_SIZE, SIGNATURE_SIZE, TRAILER_SIZE,
};
use crate::ctb::PREVIEW_SIZES;
use crate::ctb::cipher::{decrypt_in_place, signature};
use crate::ctb::disclaimer::DISCLAIMER_SIZE;
use crate::ctb::layer_xor;
use crate::ctb::page_address::PAGE_BYTES;
use crate::error::WriteFailure;
use crate::preview::{PreviewImage, PreviewSize, PrintPreviews};
use crate::print_program::{CreationTime, LayerProgram, MotionStage, PrintProgram};
use crate::raster_setup::RasterSetup;

const PLATE_MM: f64 = 10.0;
const PLATE_Z_MM: f64 = 100.0;
const PIXELS_PER_SIDE: u32 = 20;
const LAYER_HEIGHT_MM: f64 = 0.1;
const MACHINE: &str = "ELEGOO Mars 5 Ultra";
const MATERIAL: &str = "Magna Durable";
const CHANNELS: usize = 3;
const CREATED_SEC: u64 = 1_788_721_620;

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
        material_name: NonEmptyString::new(MATERIAL).unwrap(),
        layers,
        layer_height_mm: mm(LAYER_HEIGHT_MM),
        bottom_layer_count: 1,
        transition_layer_count: 0,
        volume_mm3: VolumeMm3::new(15_000.0).unwrap(),
        mass_g: MassG::new(17.5).unwrap(),
        print_time_sec: NonNegativeFiniteSec::new(5664.0).unwrap(),
        created_at: CreationTime {
            rendered: NonEmptyString::new("2026-09-04 21:07:00").unwrap(),
            unix_sec: CREATED_SEC,
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

fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

/// An address as the records store one, page number beside offset.
fn paged(bytes: &[u8], offset_at: usize, page_at: usize) -> usize {
    let page = u64::from(u32_at(bytes, page_at));
    let offset = u64::from(u32_at(bytes, offset_at));
    (page * PAGE_BYTES + offset) as usize
}

/// Read the settings block back the way a machine does.
fn settings(bytes: &[u8]) -> [u8; SETTINGS_SIZE as usize] {
    let at = u32_at(bytes, 8) as usize;
    let size = u32_at(bytes, 4) as usize;
    assert_eq!(size, SETTINGS_SIZE as usize);

    let mut block: [u8; SETTINGS_SIZE as usize] = bytes[at..at + size].try_into().unwrap();
    decrypt_in_place(&mut block);
    block
}

/// Every layer's record address, image address and image length, as
/// the pointer table and the records state them.
fn layer_records(bytes: &[u8]) -> Vec<(usize, usize, usize)> {
    let settings = settings(bytes);
    let table = u32_at(&settings, 8) as usize;
    let count = u32_at(&settings, 64) as usize;

    (0..count)
        .map(|index| {
            let entry = table + index * LAYER_POINTER_SIZE as usize;
            let record = paged(bytes, entry, entry + 4);
            let image = paged(bytes, record + 16, record + 20);
            (record, image, u32_at(bytes, record + 24) as usize)
        })
        .collect()
}

/// Decode an RLE7 image, reporting how many pixels it covers, or
/// nothing at all when a record runs off the end of it.
fn decoded_pixels(bytes: &[u8]) -> Option<usize> {
    let mut pixels = 0;
    let mut at = 0;
    while at < bytes.len() {
        let repeated = bytes[at] & 0x80 != 0;
        at += 1;
        if !repeated {
            pixels += 1;
            continue;
        }
        let lead = *bytes.get(at)?;
        let (extra, mut length) = match lead {
            0x00..=0x7F => (0, usize::from(lead)),
            0x80..=0xBF => (1, usize::from(lead & 0x3F)),
            0xC0..=0xDF => (2, usize::from(lead & 0x1F)),
            _ => (3, usize::from(lead & 0x0F)),
        };
        at += 1;
        for _ in 0..extra {
            length = (length << 8) | usize::from(*bytes.get(at)?);
            at += 1;
        }
        pixels += length;
    }
    Some(pixels)
}

#[test]
fn the_file_opens_with_a_plaintext_header() {
    let (program, stack) = three_layer_case();
    let bytes = written(&program, &stack);

    assert_eq!(u32_at(&bytes, 0), 0x12FD_0107);
    assert_eq!(u32_at(&bytes, 16), 5);
    assert_eq!(u32_at(&bytes, 8), HEADER_SIZE);
    assert_eq!(u32_at(&bytes, 4), SETTINGS_SIZE);
}

#[test]
fn the_settings_block_reaches_the_file_encrypted() {
    let (program, stack) = three_layer_case();
    let bytes = written(&program, &stack);
    let at = HEADER_SIZE as usize;
    let raw = &bytes[at..at + SETTINGS_SIZE as usize];

    let block = settings(&bytes);
    assert_ne!(raw, block, "the block on disk is not its own plaintext");
    assert_eq!(
        u64_at(&block, 0),
        0,
        "the value the signature is taken over"
    );
    assert_eq!(u32_at(&block, 64), 3, "the layer count");
}

#[test]
fn every_address_the_settings_block_records_lands_inside_the_file() {
    let (program, stack) = three_layer_case();
    let bytes = written(&program, &stack);
    let block = settings(&bytes);

    for offset in [8, 68, 72, 160, 264, 276] {
        let target = u32_at(&block, offset) as usize;
        assert!(target < bytes.len(), "settings offset {offset} -> {target}");
    }
}

#[test]
fn the_sections_abut_in_the_order_the_settings_block_records_them() {
    let (program, stack) = three_layer_case();
    let bytes = write_with(&program, &stack, Some(&previews()));
    let block = settings(&bytes);

    let large = u32_at(&block, 68) as usize;
    let small = u32_at(&block, 72) as usize;
    assert_eq!(large, (HEADER_SIZE + SETTINGS_SIZE) as usize);
    assert_eq!(
        small,
        large + PREVIEW_RECORD_SIZE as usize + u32_at(&bytes, large + 12) as usize,
    );

    let machine_name = u32_at(&block, 160) as usize;
    let machine_name_size = u32_at(&block, 164) as usize;
    assert_eq!(
        machine_name,
        small + PREVIEW_RECORD_SIZE as usize + u32_at(&bytes, small + 12) as usize,
    );
    assert_eq!(
        &bytes[machine_name..machine_name + machine_name_size],
        MACHINE.as_bytes(),
    );

    let disclaimer = u32_at(&block, 264) as usize;
    assert_eq!(disclaimer, machine_name + machine_name_size);
    assert_eq!(u32_at(&block, 268), DISCLAIMER_SIZE);
    assert!(
        bytes[disclaimer..].starts_with(b"Layout and record format for the ctb"),
        "the disclaimer is where the block says it is",
    );

    let resin = u32_at(&block, 276) as usize;
    assert_eq!(resin, disclaimer + DISCLAIMER_SIZE as usize);
    assert_eq!(
        u32_at(&block, 8) as usize,
        resin + 40 + MATERIAL.len() * 2 + MACHINE.len(),
        "the pointer table follows the resin block",
    );
}

#[test]
fn the_pointer_table_holds_one_entry_per_layer() {
    let (program, stack) = three_layer_case();
    let bytes = written(&program, &stack);
    let block = settings(&bytes);
    let table = u32_at(&block, 8) as usize;

    assert_eq!(u32_at(&block, 64) as usize, program.layers.len());
    for index in 0..program.layers.len() {
        let entry = table + index * LAYER_POINTER_SIZE as usize;
        assert_eq!(u32_at(&bytes, entry + 8), LAYER_RECORD_SIZE);
        assert_eq!(u32_at(&bytes, entry + 12), 0);
    }
}

#[test]
fn each_record_carries_its_own_image_behind_it() {
    let (program, stack) = three_layer_case();
    let bytes = written(&program, &stack);
    let records = layer_records(&bytes);

    let mut previous_end =
        u32_at(&settings(&bytes), 8) as usize + LAYER_POINTER_SIZE as usize * program.layers.len();
    for (record, image, length) in records {
        assert_eq!(record, previous_end, "the records run on with no gap");
        assert_eq!(u32_at(&bytes, record), LAYER_RECORD_SIZE);
        assert_eq!(image, record + LAYER_RECORD_SIZE as usize);
        assert!(image + length <= bytes.len());
        previous_end = image + length;
    }
}

#[test]
fn the_last_image_ends_where_the_marker_and_the_signature_begin() {
    let (program, stack) = three_layer_case();
    let bytes = written(&program, &stack);
    let (_, image, length) = *layer_records(&bytes).last().unwrap();
    let signature_at = u32_at(&bytes, 24) as usize;

    assert_eq!(
        signature_at,
        image + length + POST_LAYER_MARKER_SIZE as usize
    );
    assert_eq!(
        bytes.len(),
        signature_at + (SIGNATURE_SIZE + TRAILER_SIZE) as usize,
    );
}

#[test]
fn every_layer_image_is_obfuscated_with_the_key_the_file_records() {
    let (program, stack) = three_layer_case();
    let bytes = written(&program, &stack);
    let block = settings(&bytes);
    let key = std::num::NonZeroU32::new(u32_at(&block, 128)).unwrap();
    let pixels = (PIXELS_PER_SIDE * PIXELS_PER_SIDE) as usize;

    for (index, (_, image, length)) in layer_records(&bytes).into_iter().enumerate() {
        let mut payload = bytes[image..image + length].to_vec();
        assert_ne!(decoded_pixels(&payload), Some(pixels), "still obfuscated");

        layer_xor::apply(key, index as u32, &mut payload);
        assert_eq!(decoded_pixels(&payload), Some(pixels), "layer {index}");
    }
}

#[test]
fn no_layer_asks_for_the_second_pass_of_encryption() {
    let (program, stack) = three_layer_case();
    let bytes = written(&program, &stack);

    for (record, _, _) in layer_records(&bytes) {
        assert_eq!(u32_at(&bytes, record + 32), 0);
        assert_eq!(u32_at(&bytes, record + 36), 0);
    }
}

#[test]
fn the_signature_signs_the_value_the_settings_block_states() {
    let (program, stack) = three_layer_case();
    let bytes = written(&program, &stack);
    let at = u32_at(&bytes, 24) as usize;
    let size = u32_at(&bytes, 20) as usize;

    assert_eq!(size, SIGNATURE_SIZE as usize);
    assert_eq!(
        &bytes[at..at + size],
        signature(u64_at(&settings(&bytes), 0)),
    );
}

#[test]
fn the_trailer_sums_everything_before_it() {
    let (program, stack) = three_layer_case();
    let bytes = written(&program, &stack);
    let trailer_at = bytes.len() - TRAILER_SIZE as usize;

    let mut sum = Hasher::new();
    sum.update(&bytes[..trailer_at]);

    assert_eq!(u32_at(&bytes, trailer_at), sum.finalize());
}

#[test]
fn a_file_without_previews_still_carries_both_records() {
    let (program, stack) = three_layer_case();
    let bytes = written(&program, &stack);
    let block = settings(&bytes);

    for record_at in [u32_at(&block, 68) as usize, u32_at(&block, 72) as usize] {
        assert!(u32_at(&bytes, record_at) > 0, "a width is still declared");
        assert!(u32_at(&bytes, record_at + 12) > 0, "a blob is still there");
    }
}

#[test]
fn a_program_and_a_stack_of_different_lengths_are_refused() {
    let program = program(vec![layer(0.1, 30.0)]);
    let stack = ContourStack::new_unchecked(vec![rect_layer(0.05, 4.0), rect_layer(0.15, 2.0)]);

    let mut out = Cursor::new(Vec::new());
    let err = write_file(&program, None, &stack, &mut out).unwrap_err();

    assert!(matches!(
        err,
        WriteFailure::LayerCountMismatch {
            program_layers: 1,
            stack_layers: 2,
        },
    ));
    assert!(out.into_inner().is_empty(), "nothing is written first");
}

#[test]
fn a_rendering_of_the_wrong_size_is_refused() {
    let (program, stack) = three_layer_case();
    let previews = PrintPreviews {
        small: frame(PREVIEW_SIZES.small),
        large: frame(PreviewSize {
            width: PREVIEW_SIZES.large.width,
            height: PREVIEW_SIZES.large.height + 1,
        }),
    };

    let mut out = Cursor::new(Vec::new());
    let err = write_file(&program, Some(&previews), &stack, &mut out).unwrap_err();

    assert!(matches!(err, WriteFailure::PreviewSize(_)));
    assert!(out.into_inner().is_empty(), "nothing is written first");
}
