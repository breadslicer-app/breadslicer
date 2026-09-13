/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use proptest::prelude::*;

use super::rle15::{MAX_COPIES, encode_preview};
use crate::preview::PreviewImage;

/// Build an image from its pixels, 3 bytes each.
fn image(width: u32, height: u32, rgb: Vec<u8>) -> PreviewImage {
    PreviewImage::new(width, height, rgb).expect("the pixels fill the size")
}

/// A 1 by `count` strip of one colour.
fn strip(count: u32, pixel: [u8; 3]) -> PreviewImage {
    image(
        count,
        1,
        pixel
            .iter()
            .copied()
            .cycle()
            .take(count as usize * 3)
            .collect(),
    )
}

fn encoded(image: &PreviewImage) -> Vec<u16> {
    let mut out = Vec::new();
    encode_preview(image, &mut out);
    out.as_chunks::<2>()
        .0
        .iter()
        .map(|word| u16::from_le_bytes(*word))
        .collect()
}

/// Decode a blob back into its pixels, as 16-bit words with the run
/// flag cleared.
fn decode(words: &[u16]) -> Vec<u16> {
    let mut pixels = Vec::new();
    let mut at = 0;
    while at < words.len() {
        let word = words[at];
        at += 1;
        let pixel = word & !0x0020;
        pixels.push(pixel);
        if word & 0x0020 != 0 {
            let count = words[at] & 0x0FFF;
            at += 1;
            pixels.extend(std::iter::repeat_n(pixel, count as usize));
        }
    }
    pixels
}

#[test]
fn a_single_pixel_packs_into_one_little_endian_word() {
    assert_eq!(encoded(&strip(1, [0xFF, 0xFF, 0xFF])), vec![0xFFDF]);
}

#[test]
fn each_channel_takes_its_own_bits() {
    assert_eq!(encoded(&strip(1, [0xFF, 0x00, 0x00])), vec![0xF800]);
    assert_eq!(encoded(&strip(1, [0x00, 0xFF, 0x00])), vec![0x07C0]);
    assert_eq!(encoded(&strip(1, [0x00, 0x00, 0xFF])), vec![0x001F]);
    assert_eq!(encoded(&strip(1, [0x00, 0x00, 0x00])), vec![0x0000]);
}

#[test]
fn the_count_holds_the_copies_that_follow_the_first_pixel() {
    assert_eq!(
        encoded(&strip(43, [0xFF, 0xFF, 0xFF])),
        vec![0xFFFF, 0x302A]
    );
    assert_eq!(encoded(&strip(2, [0xFF, 0xFF, 0xFF])), vec![0xFFFF, 0x3001]);
}

#[test]
fn a_run_reaching_the_cap_stays_in_one_record() {
    let pixels = MAX_COPIES + 1;
    assert_eq!(
        encoded(&strip(pixels, [0xFF, 0xFF, 0xFF])),
        vec![0xFFFF, 0x3000 | MAX_COPIES as u16],
    );
}

#[test]
fn a_run_past_the_cap_opens_another_record() {
    let pixels = MAX_COPIES + 2;
    assert_eq!(
        encoded(&strip(pixels, [0xFF, 0xFF, 0xFF])),
        vec![0xFFFF, 0x3000 | MAX_COPIES as u16, 0xFFDF],
    );
}

#[test]
fn an_image_of_distinct_pixels_carries_no_run_at_all() {
    let rgb: Vec<u8> = (0..16_u8).flat_map(|i| [i * 16, 0, 0]).collect();
    let blob = {
        let mut out = Vec::new();
        encode_preview(&image(4, 4, rgb), &mut out);
        out
    };

    assert_eq!(blob.len(), 4 * 4 * 2);
    let words = blob
        .as_chunks::<2>()
        .0
        .iter()
        .map(|word| u16::from_le_bytes(*word));
    for word in words {
        assert_eq!(word & 0x0020, 0, "word {word:#06X} claims a run");
    }
}

#[test]
fn a_run_wraps_across_rows() {
    // Every pixel of a 4 by 3 image is the same, so the whole frame is
    // one run rather than one per row.
    let rgb = vec![0xFF; 4 * 3 * 3];
    assert_eq!(encoded(&image(4, 3, rgb)), vec![0xFFFF, 0x300B]);
}

proptest! {
    #[test]
    fn an_image_round_trips_to_its_own_pixel_count(
        pixels in prop::collection::vec((0_u8..=255, 0_u8..=255, 0_u8..=255), 1..64)
    ) {
        let width = pixels.len() as u32;
        let rgb: Vec<u8> = pixels
            .iter()
            .flat_map(|(r, g, b)| [*r, *g, *b])
            .collect();
        let expected: Vec<u16> = pixels
            .iter()
            .map(|(r, g, b)| {
                (u16::from(*r >> 3) << 11) | (u16::from(*g >> 3) << 6) | u16::from(*b >> 3)
            })
            .collect();

        let words = encoded(&image(width, 1, rgb));

        prop_assert_eq!(decode(&words), expected);
    }
}
