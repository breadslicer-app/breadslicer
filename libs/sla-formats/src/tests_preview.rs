/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use crate::preview::{PixelCountMismatch, PreviewImage, PreviewSizeMismatch};

#[test]
fn an_image_reports_the_size_and_pixels_it_was_given() {
    let rgb = vec![7u8; 2 * 3 * 3];
    let image = PreviewImage::new(2, 3, rgb.clone()).unwrap();

    assert_eq!(image.width(), 2);
    assert_eq!(image.height(), 3);
    assert_eq!(image.rgb(), rgb.as_slice());
}

#[test]
fn pixels_short_of_the_stated_size_are_refused() {
    let err = PreviewImage::new(2, 3, vec![0u8; 17]).unwrap_err();
    assert_eq!(
        err,
        PixelCountMismatch {
            width: 2,
            height: 3,
            bytes: 17,
        },
    );
}

#[test]
fn pixels_beyond_the_stated_size_are_refused() {
    let err = PreviewImage::new(2, 3, vec![0u8; 19]).unwrap_err();
    assert_eq!(err.bytes, 19);
}

#[test]
fn a_size_no_buffer_could_fill_is_refused() {
    let err = PreviewImage::new(u32::MAX, u32::MAX, Vec::new()).unwrap_err();
    assert_eq!(err.bytes, 0);
}

#[test]
fn the_pixel_count_message_names_both_counts() {
    let msg = PixelCountMismatch {
        width: 2,
        height: 3,
        bytes: 17,
    }
    .to_string();
    assert!(msg.contains("18"), "msg: {msg}");
    assert!(msg.contains("17"), "msg: {msg}");
}

#[test]
fn the_size_mismatch_message_names_what_was_wanted_and_what_arrived() {
    let msg = PreviewSizeMismatch {
        expected_width: 116,
        expected_height: 116,
        actual_width: 200,
        actual_height: 150,
    }
    .to_string();
    assert!(msg.contains("116 x 116"), "msg: {msg}");
    assert!(msg.contains("200 x 150"), "msg: {msg}");
}
