/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The layer codec against a file the format's author wrote.
//!
//! The grammar admits more than one encoding of the same image: a
//! single-pixel run has a short form and a repeat form, and a long
//! run may be split. Re-encoding a real file's own layers is what
//! says whether this encoder chooses as its author's does.

use std::num::NonZeroU32;

use sc_sl1cer::raster::rle::RleRun;

use super::layout::{LAYER_POINTER_SIZE, SETTINGS_SIZE};
use crate::ctb::cipher::decrypt_in_place;
use crate::ctb::page_address::PAGE_BYTES;
use crate::ctb::{layer_xor, rle7};

const FIXTURE: &[u8] = include_bytes!(
    "../../../tests/fixtures/official/chitubox-ctb-v5enc/orientation-test-plate-z-asymmetric.ctb"
);

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn paged(bytes: &[u8], offset_at: usize, page_at: usize) -> usize {
    let page = u64::from(u32_at(bytes, page_at));
    (page * PAGE_BYTES + u64::from(u32_at(bytes, offset_at))) as usize
}

fn settings() -> [u8; SETTINGS_SIZE as usize] {
    let at = u32_at(FIXTURE, 8) as usize;
    let mut block: [u8; SETTINGS_SIZE as usize] =
        FIXTURE[at..at + SETTINGS_SIZE as usize].try_into().unwrap();
    decrypt_in_place(&mut block);
    block
}

/// Decode RLE7 into the runs it carries, as 7-bit values.
fn decode(bytes: &[u8]) -> Vec<(u8, u32)> {
    let mut runs = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let value = bytes[at] & 0x7F;
        let repeated = bytes[at] & 0x80 != 0;
        at += 1;
        if !repeated {
            runs.push((value, 1));
            continue;
        }
        let lead = bytes[at];
        let (extra, mut length) = match lead {
            0x00..=0x7F => (0, u32::from(lead)),
            0x80..=0xBF => (1, u32::from(lead & 0x3F)),
            0xC0..=0xDF => (2, u32::from(lead & 0x1F)),
            _ => (3, u32::from(lead & 0x0F)),
        };
        at += 1;
        for _ in 0..extra {
            length = (length << 8) | u32::from(bytes[at]);
            at += 1;
        }
        runs.push((value, length));
    }
    runs
}

/// Every layer's image, with the obfuscation undone, skipping the
/// ones that carry a second pass of encryption.
fn layer_images() -> Vec<(usize, Vec<u8>)> {
    let settings = settings();
    let table = u32_at(&settings, 8) as usize;
    let count = u32_at(&settings, 64) as usize;
    let key = NonZeroU32::new(u32_at(&settings, 128)).expect("the file declares a key");

    (0..count)
        .filter_map(|index| {
            let entry = table + index * LAYER_POINTER_SIZE as usize;
            let record = paged(FIXTURE, entry, entry + 4);
            if u32_at(FIXTURE, record + 36) != 0 {
                return None;
            }
            let image = paged(FIXTURE, record + 16, record + 20);
            let length = u32_at(FIXTURE, record + 24) as usize;

            let mut payload = FIXTURE[image..image + length].to_vec();
            layer_xor::apply(key, index as u32, &mut payload);
            Some((index, payload))
        })
        .collect()
}

#[test]
fn the_fixture_is_the_file_its_provenance_names() {
    let settings = settings();

    assert_eq!(u32_at(FIXTURE, 0), 0x12FD_0107, "the encrypted magic");
    assert_eq!(u32_at(FIXTURE, 16), 5, "the revision");
    assert_eq!(u32_at(&settings, 64), 50, "layers");
    assert_eq!(u32_at(&settings, 56), 8520);
    assert_eq!(u32_at(&settings, 60), 4320);
}

#[test]
fn every_layer_decodes_to_exactly_the_panel() {
    let settings = settings();
    let pixels = u64::from(u32_at(&settings, 56)) * u64::from(u32_at(&settings, 60));

    let images = layer_images();
    assert!(!images.is_empty(), "no layer was readable");
    for (index, payload) in images {
        let covered: u64 = decode(&payload)
            .iter()
            .map(|(_, length)| u64::from(*length))
            .sum();

        assert_eq!(covered, pixels, "layer {index}");
    }
}

#[test]
fn every_layer_re_encodes_to_the_bytes_the_vendor_wrote() {
    for (index, payload) in layer_images() {
        let runs: Vec<RleRun> = decode(&payload)
            .into_iter()
            .map(|(value, length)| RleRun {
                value: value << 1,
                length,
            })
            .collect();

        let mut ours = Vec::new();
        rle7::encode_layer(&runs, &mut ours);

        assert_eq!(
            ours.len(),
            payload.len(),
            "layer {index} encodes to a different length",
        );
        assert!(ours == payload, "layer {index} differs from the vendor's");
    }
}
