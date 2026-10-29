/*
 * SPDX-FileCopyrightText: 2020-2026 PTRTECH, Tiago Conceição (sn4k3) and UVtools contributors
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 *
 * Ported to Rust from UVtools UVtools.Core/FileFormats/BitPlaneRle.cs and
 * UVtools.Core/FileFormats/AnycubicFile.cs (EncodePWS, DecodePWS,
 * RLE1EncodingLimit), UVtools v7.1.1, commit
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

//! Anycubic's PWS layer codec: one 1-bit plane per anti-aliasing level.
//!
//! Plane `k` of `levels` lights every pixel at or above its threshold, so a
//! pixel's grey is the number of planes it is lit in. Each plane is a run of
//! bytes over the whole frame: the low 7 bits count up to 125 pixels, the high
//! bit says whether they are lit.

use sc_sl1cer::raster::rle::RleRun;

/// The longest run one byte holds.
const RLE1_ENCODING_LIMIT: usize = 0x7d;

/// The grey from which plane `level` (1-based) of `levels` lights a pixel.
fn threshold(level: u8, levels: u8) -> u8 {
    (255 * u32::from(level) / (u32::from(levels) + 1) + 1) as u8
}

/// Encodes a frame of `runs`, rows in image order, as `levels` bit planes.
pub(crate) fn encode(runs: &[RleRun], levels: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(256);
    for level in 1..=levels.max(1) {
        let threshold = threshold(level, levels.max(1));
        let mut state: Option<bool> = None;
        let mut length = 0_usize;
        for run in runs.iter().filter(|run| run.length > 0) {
            let lit = run.value >= threshold;
            if state == Some(lit) {
                length += run.length as usize;
            } else {
                if let Some(lit) = state {
                    put_run(&mut out, length, lit);
                }
                state = Some(lit);
                length = run.length as usize;
            }
        }
        if let Some(lit) = state {
            put_run(&mut out, length, lit);
        }
    }
    out
}

/// Writes one plane run, split into bytes of up to 125 pixels.
fn put_run(out: &mut Vec<u8>, mut length: usize, lit: bool) {
    let flag = if lit { 0x80 } else { 0 };
    while length > 0 {
        let take = length.min(RLE1_ENCODING_LIMIT);
        out.push(take as u8 | flag);
        length -= take;
    }
}

/// Returned when the planes do not cover a frame of the expected size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PwsMismatch {
    /// The plane that fell short or ran over, 0-based.
    pub(crate) plane: u8,
}

/// Decodes `levels` bit planes over `pixel_count` pixels back to 8-bit greys,
/// as UVtools does: a pixel lit in `n` planes reads `n * (256 / levels) - 1`.
pub(crate) fn decode(
    encoded: &[u8],
    pixel_count: usize,
    levels: u8,
) -> Result<Vec<u8>, PwsMismatch> {
    let mut counts = vec![0_u8; pixel_count];
    let mut index = 0;
    for plane in 0..levels {
        let mut pixel = 0;
        while pixel < pixel_count {
            let &byte = encoded.get(index).ok_or(PwsMismatch { plane })?;
            index += 1;
            let reps = usize::from(byte & 0x7f);
            if reps > pixel_count - pixel {
                return Err(PwsMismatch { plane });
            }
            if byte & 0x80 != 0 {
                for count in &mut counts[pixel..pixel + reps] {
                    *count += 1;
                }
            }
            pixel += reps;
        }
    }
    let step = 256 / u32::from(levels.max(1));
    Ok(counts
        .into_iter()
        .map(|count| (u32::from(count) * step).saturating_sub(1) as u8)
        .collect())
}

/// The grey a pixel of grey `value` reads back as after a round trip through
/// `levels` planes.
#[cfg(test)]
pub(crate) fn quantise(value: u8, levels: u8) -> u8 {
    let lit = (1..=levels)
        .filter(|&level| value >= threshold(level, levels))
        .count() as u32;
    (lit * (256 / u32::from(levels))).saturating_sub(1) as u8
}

#[cfg(test)]
mod pws_tests {
    use super::*;

    fn run(value: u8, length: u32) -> RleRun {
        RleRun { value, length }
    }

    #[test]
    fn thresholds_split_the_greys_evenly() {
        assert_eq!(threshold(1, 1), 128);
        let four: Vec<u8> = (1..=4).map(|level| threshold(level, 4)).collect();
        assert_eq!(four, [52, 103, 154, 205]);
    }

    #[test]
    fn a_long_run_splits_into_bytes_of_125() {
        assert_eq!(encode(&[run(0, 300)], 1), [125, 125, 50]);
        assert_eq!(encode(&[run(255, 130)], 1), [0x80 | 125, 0x80 | 5]);
    }

    #[test]
    fn each_level_adds_a_plane() {
        // A white pixel, a mid grey and a black pixel, 4 levels.
        let encoded = encode(&[run(255, 1), run(120, 1), run(0, 1)], 4);
        assert_eq!(
            encoded,
            [
                0x80 | 2,
                1, // plane 1: lit up to 52
                0x80 | 2,
                1, // plane 2: up to 103
                0x80 | 1,
                2, // plane 3: up to 154
                0x80 | 1,
                2, // plane 4: up to 205
            ]
        );
    }

    #[test]
    fn decoding_gives_back_the_quantised_greys() {
        for levels in [1, 2, 4, 8] {
            let runs: Vec<RleRun> = (0..=255).map(|value| run(value, 3)).collect();
            let decoded = decode(&encode(&runs, levels), 256 * 3, levels).unwrap();
            let expected: Vec<u8> = (0..=255_u8)
                .flat_map(|value| [quantise(value, levels); 3])
                .collect();
            assert_eq!(decoded, expected, "{levels} levels");
        }
    }

    #[test]
    fn planes_that_do_not_cover_the_frame_are_refused() {
        // 2 planes of 10 pixels cover 1 frame of 20, then run out.
        assert_eq!(
            decode(&encode(&[run(255, 10)], 2), 20, 2),
            Err(PwsMismatch { plane: 1 })
        );
        // A run longer than the frame.
        assert_eq!(
            decode(&encode(&[run(255, 10)], 1), 5, 1),
            Err(PwsMismatch { plane: 0 })
        );
    }
}
