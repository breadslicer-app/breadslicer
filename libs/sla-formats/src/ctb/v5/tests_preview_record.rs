/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::layout::PREVIEW_RECORD_SIZE;
use super::preview_record::write_preview_record;
use crate::preview::PreviewImage;

const CHANNELS: usize = 3;

fn image(width: u32, height: u32) -> PreviewImage {
    let pixels = (width * height) as usize;
    PreviewImage::new(width, height, vec![0; pixels * CHANNELS]).unwrap()
}

fn written(image: &PreviewImage, blob: &[u8], blob_offset: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_preview_record(image, blob, blob_offset, &mut bytes).unwrap();
    bytes
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

#[test]
fn the_record_is_the_size_the_layout_reserves_for_it() {
    let bytes = written(&image(400, 300), &[0; 64], 1_000);

    assert_eq!(bytes.len(), PREVIEW_RECORD_SIZE as usize);
}

#[test]
fn the_record_carries_its_own_images_size() {
    let bytes = written(&image(200, 125), &[0; 64], 1_000);

    assert_eq!(u32_at(&bytes, 0), 200);
    assert_eq!(u32_at(&bytes, 4), 125);
}

#[test]
fn the_record_locates_its_blob_and_counts_it() {
    let bytes = written(&image(400, 300), &[0; 64], 1_000);

    assert_eq!(u32_at(&bytes, 8), 1_000);
    assert_eq!(u32_at(&bytes, 12), 64);
}

#[test]
fn an_uncompressed_blob_is_counted_the_same_way() {
    let image = image(4, 3);
    let blob = vec![0; 4 * 3 * 2];
    let bytes = written(&image, &blob, 500);

    assert_eq!(u32_at(&bytes, 12), 24);
}

#[test]
fn the_record_ends_in_reserved_bytes() {
    let bytes = written(&image(400, 300), &[0; 64], 1_000);

    assert!(bytes[16..].iter().all(|byte| *byte == 0));
}

#[test]
fn two_records_can_describe_images_of_different_shapes() {
    let large = written(&image(400, 300), &[0; 64], 1_000);
    let small = written(&image(200, 125), &[0; 32], 2_000);

    assert_eq!(u32_at(&large, 0), 400);
    assert_eq!(u32_at(&small, 0), 200);
    assert_ne!(u32_at(&large, 8), u32_at(&small, 8));
}
