/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! One layer image, framed as the file stores it.

use sc_sl1cer::raster::rle::RleRun;

use super::chunk::RunEncoder;

/// Byte that opens a layer image.
const PAYLOAD_MAGIC: u8 = 0x55;

/// Append `runs` to `out` as one layer image: the magic byte, the run
/// chunks, then a checksum byte covering everything after the magic.
pub fn encode_payload(runs: &[RleRun], out: &mut Vec<u8>) {
    out.push(PAYLOAD_MAGIC);
    let body = out.len();
    let mut encoder = RunEncoder::default();
    for run in runs {
        encoder.push_run(run.value, run.length, out);
    }
    let sum = out[body..]
        .iter()
        .fold(0_u8, |sum, byte| sum.wrapping_add(*byte));
    out.push(!sum);
}
