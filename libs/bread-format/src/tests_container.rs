/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

use crate::container::{
    BreadReader, BreadWriter, CONTAINER_VERSION, ChunkTag, MAGIC, ReadBreadError,
};

const A: ChunkTag = ChunkTag(*b"AAAA");
const B: ChunkTag = ChunkTag(*b"BBBB");

fn file(chunks: &[(ChunkTag, u16, &[u8])]) -> Vec<u8> {
    let mut writer = BreadWriter::new(Vec::new()).unwrap();
    for (tag, version, payload) in chunks {
        writer.chunk(*tag, *version, payload).unwrap();
    }
    writer.finish().unwrap()
}

fn read_all(bytes: &[u8]) -> Result<Vec<(ChunkTag, u16, Vec<u8>)>, ReadBreadError> {
    let mut reader = BreadReader::new(bytes)?;
    let mut chunks = Vec::new();
    while let Some(chunk) = reader.next_chunk()? {
        chunks.push((chunk.tag, chunk.version, chunk.payload));
    }
    Ok(chunks)
}

#[test]
fn chunks_come_back_in_order_with_their_versions() {
    let big: Vec<u8> = (0..100_000u32)
        .flat_map(|i| (i % 251).to_le_bytes())
        .collect();
    let bytes = file(&[(A, 1, b"hello"), (B, 7, &big), (A, 2, b"")]);
    let chunks = read_all(&bytes).unwrap();
    assert_eq!(chunks.len(), 3);
    assert_eq!(chunks[0], (A, 1, b"hello".to_vec()));
    assert_eq!(chunks[1], (B, 7, big));
    assert_eq!(chunks[2], (A, 2, Vec::new()));
}

#[test]
fn an_empty_file_has_a_header_and_no_chunks() {
    let bytes = file(&[]);
    assert_eq!(&bytes[..8], &MAGIC);
    assert!(read_all(&bytes).unwrap().is_empty());
}

#[test]
fn other_files_are_not_bread() {
    assert!(matches!(
        read_all(b"PK\x03\x04 a zip"),
        Err(ReadBreadError::NotBread)
    ));
    assert!(matches!(read_all(b""), Err(ReadBreadError::NotBread)));
    // A text-mode transfer turns "\r\n" into "\n".
    let mut mangled = file(&[]);
    mangled.remove(6);
    assert!(matches!(read_all(&mangled), Err(ReadBreadError::NotBread)));
}

#[test]
fn a_newer_container_is_refused() {
    let mut bytes = file(&[(A, 1, b"x")]);
    bytes[8..10].copy_from_slice(&(CONTAINER_VERSION + 1).to_le_bytes());
    assert!(matches!(
        read_all(&bytes),
        Err(ReadBreadError::UnsupportedVersion(v)) if v == CONTAINER_VERSION + 1
    ));
}

#[test]
fn a_flipped_byte_is_caught_by_the_checksum() {
    let mut bytes = file(&[(A, 1, b"some payload worth guarding")]);
    let last = bytes.len() - 1;
    bytes[last] ^= 0x40;
    assert!(matches!(read_all(&bytes), Err(ReadBreadError::Corrupt(tag)) if tag == A));
}

#[test]
fn a_file_cut_short_is_truncated() {
    let bytes = file(&[(A, 1, b"first"), (B, 1, b"second chunk")]);
    for cut in [bytes.len() - 1, bytes.len() - 20, 12] {
        assert!(
            matches!(read_all(&bytes[..cut]), Err(ReadBreadError::Truncated)),
            "cut at {cut}"
        );
    }
}

#[test]
fn a_wrong_recorded_length_is_a_bad_payload() {
    let mut bytes = file(&[(A, 1, b"twelve bytes")]);
    // The raw length sits after the header (10), tag (4) and version (2).
    bytes[16..24].copy_from_slice(&(1u64 << 40).to_le_bytes());
    assert!(matches!(read_all(&bytes), Err(ReadBreadError::BadPayload(tag)) if tag == A));
}
