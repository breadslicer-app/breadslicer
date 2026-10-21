/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The obfuscation a layer image carries when the file declares a
//! key.
//!
//! A linear congruential generator produces a 32-bit keystream from
//! the key and the layer's own index, and the image is XORed with it
//! word by word. A key of zero means the images are stored plainly
//! and no stream exists, which is why one cannot be asked for here.

use std::num::NonZeroU32;

/// The generator's own step, from the key.
const KEY_MULTIPLIER: u32 = 0x2D83_CDAC;
const KEY_ADDEND: u32 = 0xD8A8_3423;

/// Where the stream starts, from the layer index.
const INDEX_MULTIPLIER: u32 = 0x1E15_30CD;
const INDEX_ADDEND: u32 = 0xEC3D_47CD;

const WORD_BYTES: usize = 4;

/// XOR `data` with the keystream that `key` and `layer_index`
/// generate.
///
/// The layer index is the initialisation vector, so one key drives a
/// different stream per layer. Applying this twice returns the
/// original bytes.
pub fn apply(key: NonZeroU32, layer_index: u32, data: &mut [u8]) {
    let step = key
        .get()
        .wrapping_mul(KEY_MULTIPLIER)
        .wrapping_add(KEY_ADDEND);
    let mut word = layer_index
        .wrapping_mul(INDEX_MULTIPLIER)
        .wrapping_add(INDEX_ADDEND)
        .wrapping_mul(step);

    for chunk in data.chunks_mut(WORD_BYTES) {
        for (byte, mask) in chunk.iter_mut().zip(word.to_le_bytes()) {
            *byte ^= mask;
        }
        word = word.wrapping_add(step);
    }
}
