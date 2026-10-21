/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The chunk encoder measured against one layer a real producer
//! wrote. Provenance and licensing of the fixture are in the README
//! beside it.

use sc_sl1cer::raster::rle::RleRun;

use super::payload::encode_payload;
use super::test_decode::decode_chunks;

const LAYER_RUNS: &str =
    include_str!("../../../tests/fixtures/official/elegoo-goo-v3/layer-32-runs.txt");

/// Bytes the vendor's own encoding of these runs occupies, magic byte
/// and checksum included.
const VENDOR_PAYLOAD_BYTES: usize = 8127;

fn fixture_runs() -> Vec<RleRun> {
    LAYER_RUNS
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(|line| {
            let (value, length) = line.split_once(' ').expect("a run is 2 fields");
            RleRun {
                value: value.parse().expect("a value is a u8"),
                length: length.parse().expect("a length is a u32"),
            }
        })
        .collect()
}

#[test]
fn the_fixture_is_the_layer_it_claims_to_be() {
    let runs = fixture_runs();
    assert_eq!(runs.len(), 4243);
    assert_eq!(
        runs.iter().map(|run| u64::from(run.length)).sum::<u64>(),
        4098 * 2560,
    );
}

#[test]
fn the_layer_reencodes_to_the_vendors_own_byte_count() {
    // Equality, not "no worse than": the encoder reaches the same
    // choice as the producer on every one of these 4243 runs, so a
    // rule change that loses or wins a single byte lands here.
    let mut out = Vec::new();
    encode_payload(&fixture_runs(), &mut out);
    assert_eq!(out.len(), VENDOR_PAYLOAD_BYTES);
}

#[test]
fn the_layer_survives_the_round_trip() {
    let runs = fixture_runs();
    let mut out = Vec::new();
    encode_payload(&runs, &mut out);

    let decoded = decode_chunks(&out[1..out.len() - 1]);
    let expected: Vec<(u8, u32)> = runs.iter().map(|run| (run.value, run.length)).collect();
    assert_eq!(decoded, expected);
}
