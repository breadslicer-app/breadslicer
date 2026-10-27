/*
 * SPDX-FileCopyrightText: 2020-2026 PTRTECH, Tiago Conceição (sn4k3) and UVtools contributors
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 *
 * Ported to Rust from UVtools UVtools.Core/FileFormats/AnycubicFile.cs
 * (EncodePW0, DecodePW0, RLE4EncodingLimit), UVtools v7.1.1, commit
 * 8b5c91fd41b60bd6265ba1c1f2b2b64636b873e8, modified for Breadslicer: the
 * encoder reads the run-length rows the slicer produces instead of a decoded
 * image.
 *
 * This program is free software: you can redistribute it and/or modify it
 * under the terms of the GNU Affero General Public License as published by the
 * Free Software Foundation, either version 3 of the License, or (at your
 * option) any later version. It is distributed WITHOUT ANY WARRANTY; see the
 * GNU Affero General Public License (LICENSE.md) for details.
 */

//! Anycubic's PW0 layer codec: 4-bit grey runs over the whole frame.
//!
//! Each run stores the high nibble of its grey. Black and white runs take 2
//! bytes and count up to 4095 pixels; a grey in between takes 1 byte and counts
//! up to 15. A grey nibble `n` decodes to `n * 17`, so anti-aliased edges keep
//! 16 levels.

use sc_sl1cer::raster::rle::RleRun;

/// The longest black or white run one code holds.
const RLE4_ENCODING_LIMIT: u32 = 0xfff;

/// The longest grey run one code holds.
const GREY_LIMIT: u32 = 0xf;

/// Encodes a frame of `runs`, rows in image order, as a PW0 layer image.
pub(crate) fn encode(runs: &[RleRun]) -> Vec<u8> {
    let mut out = Vec::with_capacity(256);
    let mut last_color: Option<u8> = None;
    let mut reps: u32 = 0;

    for run in runs {
        if run.length == 0 {
            continue;
        }
        let color = run.value >> 4;
        if last_color == Some(color) {
            reps += run.length;
        } else {
            if let Some(last) = last_color {
                put_reps(&mut out, reps, last);
            }
            last_color = Some(color);
            reps = run.length;
        }
    }
    if let Some(last) = last_color {
        put_reps(&mut out, reps, last);
    }
    out
}

fn put_reps(out: &mut Vec<u8>, mut reps: u32, color: u8) {
    while reps > 0 {
        if color == 0 || color == 0xf {
            let done = reps.min(RLE4_ENCODING_LIMIT);
            let code = (done as u16) | (u16::from(color) << 12);
            out.extend_from_slice(&code.to_be_bytes());
            reps -= done;
        } else {
            let done = reps.min(GREY_LIMIT);
            out.push(done as u8 | (color << 4));
            reps -= done;
        }
    }
}

/// Returned when a PW0 image does not fill its frame exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Pw0Mismatch {
    pub(crate) decoded: usize,
    pub(crate) expected: usize,
}

/// Decodes a PW0 image of `pixel_count` pixels into 8-bit greys, rows in image
/// order.
pub(crate) fn decode(encoded: &[u8], pixel_count: usize) -> Result<Vec<u8>, Pw0Mismatch> {
    let mut image = vec![0_u8; pixel_count];
    let mut position = 0_usize;
    let mut index = 0_usize;
    while index < encoded.len() {
        let byte = encoded[index];
        let code = byte >> 4;
        let mut repeat = usize::from(byte & 0xf);
        let color = match code {
            0x0 | 0xf => {
                index += 1;
                if index >= encoded.len() {
                    repeat = pixel_count.saturating_sub(position);
                } else {
                    repeat = (repeat << 8) + usize::from(encoded[index]);
                }
                if code == 0 { 0 } else { 255 }
            }
            _ => (code << 4) | code,
        };
        if position + repeat > pixel_count {
            return Err(Pw0Mismatch {
                decoded: position + repeat,
                expected: pixel_count,
            });
        }
        image[position..position + repeat].fill(color);
        position += repeat;
        index += 1;
        if position == pixel_count {
            break;
        }
    }
    if position != pixel_count {
        return Err(Pw0Mismatch {
            decoded: position,
            expected: pixel_count,
        });
    }
    Ok(image)
}

#[cfg(test)]
mod pw0_tests {
    use proptest::prelude::*;

    use super::*;

    fn run(value: u8, length: u32) -> RleRun {
        RleRun { value, length }
    }

    #[test]
    fn a_long_black_run_splits_at_the_limit() {
        let encoded = encode(&[run(0, 5000)]);
        assert_eq!(encoded, [0x0f, 0xff, 0x03, 0x89]);
    }

    #[test]
    fn white_runs_carry_the_white_code() {
        assert_eq!(encode(&[run(255, 3)]), [0xf0, 0x03]);
    }

    #[test]
    fn greys_keep_their_high_nibble_in_runs_of_up_to_15() {
        assert_eq!(encode(&[run(0x87, 20)]), [0x8f, 0x85]);
    }

    #[test]
    fn neighbouring_runs_with_one_nibble_merge() {
        assert_eq!(encode(&[run(0x81, 2), run(0x8e, 3)]), [0x85]);
    }

    #[test]
    fn a_frame_too_short_for_its_size_is_refused() {
        assert!(decode(&encode(&[run(0, 10)]), 11).is_err());
    }

    proptest! {
        #[test]
        fn decoding_gives_back_every_grey_at_4_bits(
            frame in proptest::collection::vec((any::<u8>(), 1_u32..6000), 1..40)
        ) {
            let runs: Vec<RleRun> = frame.iter().map(|&(v, l)| run(v, l)).collect();
            let pixels: usize = frame.iter().map(|&(_, l)| l as usize).sum();
            let decoded = decode(&encode(&runs), pixels).unwrap();
            let mut expected = Vec::with_capacity(pixels);
            for &(value, length) in &frame {
                let nibble = value >> 4;
                expected.extend(std::iter::repeat_n((nibble << 4) | nibble, length as usize));
            }
            prop_assert_eq!(decoded, expected);
        }
    }
}
