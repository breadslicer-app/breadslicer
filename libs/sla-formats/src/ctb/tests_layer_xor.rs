/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::num::NonZeroU32;

use proptest::prelude::*;

use super::layer_xor::apply;

/// The key a file written by the vendor's own slicer declares.
const VENDOR_KEY: u32 = 1_755_969_410;

/// Run the stream over a run of zeros, which leaves the stream
/// itself.
fn keystream(key: u32, layer_index: u32, length: usize) -> Vec<u8> {
    let mut data = vec![0; length];
    apply(NonZeroU32::new(key).expect("a key"), layer_index, &mut data);
    data
}

#[test]
fn the_key_and_the_layer_index_place_the_stream() {
    assert_eq!(
        keystream(VENDOR_KEY, 0, 12),
        vec![
            0x7F, 0x04, 0x12, 0xB7, 0xFA, 0x9D, 0xA5, 0x7E, 0x75, 0x37, 0x39, 0x46
        ],
    );
    assert_eq!(
        keystream(VENDOR_KEY, 1, 8),
        vec![0xFE, 0xFB, 0x21, 0x36, 0x79, 0x95, 0xB5, 0xFD],
    );
    assert_eq!(
        keystream(1, 0, 8),
        vec![0xC3, 0xDB, 0x10, 0x68, 0x92, 0xDD, 0x3C, 0x6E],
    );
}

#[test]
fn an_image_that_ends_mid_word_is_covered_to_its_last_byte() {
    let whole = keystream(VENDOR_KEY, 0, 12);
    for length in 1..=12 {
        assert_eq!(keystream(VENDOR_KEY, 0, length), whole[..length]);
    }
}

#[test]
fn an_empty_image_is_left_alone() {
    assert!(keystream(VENDOR_KEY, 0, 0).is_empty());
}

proptest! {
    #[test]
    fn applying_the_stream_twice_returns_the_image(
        key in 1u32..,
        layer_index in 0u32..4096,
        image in prop::collection::vec(any::<u8>(), 0..512),
    ) {
        let key = NonZeroU32::new(key).expect("a key");
        let mut scrambled = image.clone();
        apply(key, layer_index, &mut scrambled);
        apply(key, layer_index, &mut scrambled);
        prop_assert_eq!(scrambled, image);
    }

    #[test]
    fn a_layer_is_scrambled_differently_from_its_neighbour(
        key in 1u32..,
        layer_index in 0u32..4095,
    ) {
        let key = NonZeroU32::new(key).expect("a key");
        let mut first = [0; 16];
        let mut second = [0; 16];
        apply(key, layer_index, &mut first);
        apply(key, layer_index + 1, &mut second);
        prop_assert_ne!(first, second);
    }
}
