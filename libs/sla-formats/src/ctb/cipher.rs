/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The fixed cipher the encrypted revisions are written under.
//!
//! Key and initialisation vector are the same 48 bytes in every
//! slicer that writes this container and at both of its encrypted
//! revisions. There is no per-file secret and nothing is exchanged,
//! so this is obfuscation rather than confidentiality: what it
//! decides is whether a machine accepts the file.

use aes::Aes256;
#[cfg(test)]
use aes::cipher::BlockModeDecrypt;
use aes::cipher::{Array, BlockModeEncrypt, KeyIvInit};
use sha2::{Digest, Sha256};

const KEY: [u8; 32] = [
    0xD0, 0x5B, 0x8E, 0x33, 0x71, 0xDE, 0x3D, 0x1A, 0xE5, 0x4F, 0x22, 0xDD, 0xDF, 0x5B, 0xFD, 0x94,
    0xAB, 0x5D, 0x64, 0x3A, 0x9D, 0x7E, 0xBF, 0xAF, 0x42, 0x03, 0xF3, 0x10, 0xD8, 0x52, 0x2A, 0xEA,
];

const IV: [u8; 16] = [
    0x0F, 0x01, 0x0A, 0x05, 0x05, 0x0B, 0x06, 0x07, 0x08, 0x06, 0x0A, 0x0C, 0x0C, 0x0D, 0x09, 0x0F,
];

/// The cipher pads nothing, so every range it covers is a multiple
/// of this.
pub const BLOCK_BYTES: usize = 16;

/// Encrypt `data` where it stands.
///
/// A range that is not a whole number of blocks does not compile,
/// which is the only guard the format's lack of padding admits: a
/// short range would be written out unrecoverable.
pub fn encrypt_in_place<const N: usize>(data: &mut [u8; N]) {
    const {
        assert!(
            N.is_multiple_of(BLOCK_BYTES),
            "an encrypted range is whole blocks"
        )
    };

    let (blocks, _) = data.as_chunks_mut::<BLOCK_BYTES>();
    cbc::Encryptor::<Aes256>::new(&KEY.into(), &IV.into())
        .encrypt_blocks(Array::cast_slice_from_core_mut(blocks));
}

/// Undo [`encrypt_in_place`], which exists so that a test can read
/// back what a writer produced the way a machine would.
#[cfg(test)]
pub(crate) fn decrypt_in_place<const N: usize>(data: &mut [u8; N]) {
    const {
        assert!(
            N.is_multiple_of(BLOCK_BYTES),
            "an encrypted range is whole blocks"
        )
    };

    let (blocks, _) = data.as_chunks_mut::<BLOCK_BYTES>();
    cbc::Decryptor::<Aes256>::new(&KEY.into(), &IV.into())
        .decrypt_blocks(Array::cast_slice_from_core_mut(blocks));
}

/// Sign `value`, which the file also states in the clear so that the
/// machine can hash it and compare.
///
/// The digest covers the value and never the file, so this attests
/// to holding the key and to nothing else.
pub fn signature(value: u64) -> [u8; 32] {
    let mut digest: [u8; 32] = Sha256::digest(value.to_le_bytes()).into();
    encrypt_in_place(&mut digest);
    digest
}
