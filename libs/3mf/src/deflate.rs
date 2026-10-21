/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Parallel DEFLATE for large package parts.
//!
//! The ZIP writer compresses one part on the calling thread. For the
//! big model parts (a detailed mesh, a full slice stack) that single
//! stream dominates the save. This module compresses such a part with
//! the strategy `pigz` uses: split it into chunks, deflate them
//! concurrently, and concatenate the results into one valid raw-deflate
//! stream, then wrap that stream in a one-entry ZIP so the package
//! writer can splice it in verbatim with `ZipWriter::raw_copy_file`,
//! without recompressing.
//!
//! The compression level matches the ZIP writer's deflate default, so
//! the stored bytes differ from a single-threaded pass only by the tiny
//! ratio cost of the chunk boundaries, which a preset dictionary at the
//! start of each chunk recovers.

use std::io;

use flate2::{Compress, Compression, FlushCompress, Status};
use rayon::prelude::*;

/// DEFLATE level applied to every chunk. Kept equal to the sequential
/// ZIP writer's deflate default (6) so compressing a part in parallel
/// never changes how hard it is compressed.
const DEFLATE_LEVEL: u32 = 6;

/// Bytes of the preceding input replayed as a preset dictionary at the
/// start of each chunk after the first, so a back-reference that would
/// cross a chunk boundary still resolves against the decoder's window.
/// 32 KiB is the DEFLATE window, the most a preset dictionary carries.
const DICTIONARY_WINDOW: usize = 32 * 1024;

/// Smallest chunk fed to a deflater. Splitting into chunks smaller than
/// this buys no parallelism worth the per-chunk ratio and setup cost,
/// so a part is divided into at most `len / MIN_CHUNK_BYTES` chunks.
const MIN_CHUNK_BYTES: usize = 512 * 1024;

/// Compress `data` into a one-entry ZIP archive named `entry_name`,
/// deflating it across threads. The returned bytes are a complete,
/// valid ZIP that [`zip::read::ZipArchive`] reads back to the original
/// and whose single entry [`zip::write::ZipWriter::raw_copy_file`]
/// splices into another archive without recompressing.
///
/// `entry_name` is the ZIP entry path (no leading slash).
pub(crate) fn parallel_entry(entry_name: &str, data: &[u8]) -> io::Result<Vec<u8>> {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(data);
    let crc = hasher.finalize();

    // #[gamma::skip(iter.max_to_min, reason = "only the chunks change")]
    // #[gamma::skip(literal.int_decrement, reason = "at least 1 thread")]
    // #[gamma::skip(literal.int_increment, reason = "only the chunks change")]
    let threads = rayon::current_num_threads().max(1);
    // #[gamma::skip(expr.decrement, reason = "only the chunks change")]
    // #[gamma::skip(expr.increment, reason = "only the chunks change")]
    let chunk_size = data.len().div_ceil(threads).max(MIN_CHUNK_BYTES);
    // #[gamma::skip(try.propagate_to_unwrap, reason = "cannot fail")]
    let compressed = pigz_deflate(data, chunk_size)?;

    if data.len() > u32::MAX as usize || compressed.len() > u32::MAX as usize {
        return Err(io::Error::other("part is too large for a 32-bit ZIP entry"));
    }
    Ok(build_single_entry_zip(
        entry_name,
        crc,
        data.len() as u32,
        &compressed,
    ))
}

/// Deflate `data` as one raw stream whose chunks are compressed in
/// parallel, then concatenated. The result is a single valid
/// raw-deflate stream (what a ZIP entry stores), recoverable in one
/// decompression pass.
// #[gamma::skip(parameter.default_shadow, reason = "1-byte chunks: too slow")]
fn pigz_deflate(data: &[u8], chunk_size: usize) -> io::Result<Vec<u8>> {
    if data.is_empty() {
        // #[gamma::skip(option.none_to_some, reason = "data is empty here")]
        return deflate_chunk(&[], None, true);
    }
    // #[gamma::skip(literal.int_increment, reason = "only the chunks change")]
    let chunk_size = chunk_size.max(1);
    let starts: Vec<usize> = (0..data.len()).step_by(chunk_size).collect();
    let last = starts.len() - 1;
    // #[gamma::skip(try.propagate_to_unwrap, reason = "cannot fail")]
    let chunks: Vec<Vec<u8>> = starts
        .par_iter()
        .enumerate()
        .map(|(index, &start)| {
            let end = (start + chunk_size).min(data.len());
            // #[gamma::skip(relational.gt_to_ge, reason = "empty dictionary")]
            // #[gamma::skip(literal.int_increment, reason = "ratio only")]
            let dictionary =
                (start > 0).then(|| &data[start.saturating_sub(DICTIONARY_WINDOW)..start]);
            deflate_chunk(&data[start..end], dictionary, index == last)
        })
        .collect::<io::Result<Vec<_>>>()?;
    Ok(chunks.concat())
}

/// Raw-deflate one chunk. `dictionary`, when present, primes the
/// compressor with the bytes immediately before the chunk. `last`
/// selects the final flush (`Finish`, which writes the stream-end
/// marker) over the boundary flush (`Sync`, which ends on a byte
/// boundary without ending the stream), so that concatenating the
/// chunks yields one continuous stream.
fn deflate_chunk(data: &[u8], dictionary: Option<&[u8]>, last: bool) -> io::Result<Vec<u8>> {
    // `false`: raw deflate, no zlib header or trailer, which is what a
    // ZIP entry stores.
    let mut compressor = Compress::new(Compression::new(DEFLATE_LEVEL), false);
    if let Some(dictionary) = dictionary {
        // #[gamma::skip(try.propagate_to_unwrap, reason = "cannot fail")]
        compressor
            .set_dictionary(dictionary)
            .map_err(io::Error::other)?;
    }
    // #[gamma::skip(arith.add_to_mul, reason = "capacity hint only")]
    // #[gamma::skip(arith.div_to_mul, reason = "capacity hint only")]
    // #[gamma::skip(arith.div_to_rem, reason = "capacity hint only")]
    // #[gamma::skip(literal.int_decrement, reason = "capacity hint only")]
    // #[gamma::skip(literal.int_increment, reason = "capacity hint only")]
    // #[gamma::skip(literal.int_to_one, reason = "capacity hint only")]
    let mut out = Vec::with_capacity(data.len() / 2 + 64);
    let mut scratch = vec![0u8; 128 * 1024];

    // Feed the whole chunk with no flush, draining output as the scratch
    // buffer fills. zlib consumes available input while it has output
    // room, so `consumed` strictly advances and the loop terminates.
    let mut consumed = 0usize;
    // #[gamma::skip(relational.lt_to_le, reason = "endless loop")]
    // #[gamma::skip(stmt.delete_assign, reason = "endless loop")]
    while consumed < data.len() {
        let before_in = compressor.total_in();
        let before_out = compressor.total_out();
        // #[gamma::skip(try.propagate_to_unwrap, reason = "cannot fail")]
        compressor
            .compress(&data[consumed..], &mut scratch, FlushCompress::None)
            .map_err(io::Error::other)?;
        consumed += (compressor.total_in() - before_in) as usize;
        let produced = (compressor.total_out() - before_out) as usize;
        out.extend_from_slice(&scratch[..produced]);
    }

    // Flush: `Finish` closes the stream (its final block sets the
    // end-of-stream marker); `Sync` ends the chunk on a byte boundary
    // without closing it, so concatenated chunks stay one stream. Both
    // may need several calls to drain, so loop until done.
    let flush = if last {
        FlushCompress::Finish
    } else {
        FlushCompress::Sync
    };
    // #[gamma::skip(loop.break_to_continue, reason = "endless loop")]
    // #[gamma::skip(match_guard.always_false, reason = "endless loop")]
    // #[gamma::skip(match_arm.never_matches, reason = "next call ends it")]
    loop {
        let before_out = compressor.total_out();
        // #[gamma::skip(try.propagate_to_unwrap, reason = "cannot fail")]
        let status = compressor
            .compress(&[], &mut scratch, flush)
            .map_err(io::Error::other)?;
        let produced = (compressor.total_out() - before_out) as usize;
        out.extend_from_slice(&scratch[..produced]);
        match status {
            Status::StreamEnd => break,
            _ if produced == 0 => break,
            _ => {}
        }
    }
    Ok(out)
}

fn le16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn le32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// Assemble a one-entry ZIP around a pre-built deflate stream: a local
/// file header, the compressed bytes, a central directory, and an end
/// record. All multi-byte fields are little-endian; the layouts follow
/// the ZIP specification (APPNOTE sections 4.3.7, 4.3.12, 4.3.16).
/// Sizes are 32-bit, so the caller must confirm they fit before this is
/// reached (this crate does not write ZIP64).
// #[gamma::skip(call_result.default, reason = "le16 and le32 return ()")]
fn build_single_entry_zip(name: &str, crc: u32, uncompressed: u32, compressed: &[u8]) -> Vec<u8> {
    const METHOD_DEFLATED: u16 = 8;
    const VERSION: u16 = 20;
    const DOS_DATE_1980: u16 = 0x21;

    let name = name.as_bytes();
    let compressed_len = compressed.len() as u32;
    // #[gamma::skip(arith.add_to_mul, reason = "capacity hint only")]
    // #[gamma::skip(arith.mul_to_add, reason = "capacity hint only")]
    // #[gamma::skip(arith.mul_to_div, reason = "capacity; names not empty")]
    // #[gamma::skip(literal.int_decrement, reason = "capacity hint only")]
    // #[gamma::skip(literal.int_increment, reason = "capacity hint only")]
    // #[gamma::skip(literal.int_to_one, reason = "capacity hint only")]
    // #[gamma::skip(literal.int_to_zero, reason = "capacity hint only")]
    let mut out = Vec::with_capacity(compressed.len() + 2 * name.len() + 98);

    // Local file header.
    le32(&mut out, 0x0403_4b50);
    le16(&mut out, VERSION); // version needed to extract
    le16(&mut out, 0); // general purpose flags
    le16(&mut out, METHOD_DEFLATED);
    le16(&mut out, 0); // last-modified time
    le16(&mut out, DOS_DATE_1980); // last-modified date
    le32(&mut out, crc);
    le32(&mut out, compressed_len);
    le32(&mut out, uncompressed);
    le16(&mut out, name.len() as u16);
    le16(&mut out, 0); // extra field length
    out.extend_from_slice(name);
    out.extend_from_slice(compressed);

    // Central directory.
    let cd_offset = out.len() as u32;
    le32(&mut out, 0x0201_4b50);
    le16(&mut out, VERSION); // version made by
    le16(&mut out, VERSION); // version needed to extract
    le16(&mut out, 0); // general purpose flags
    le16(&mut out, METHOD_DEFLATED);
    le16(&mut out, 0); // last-modified time
    le16(&mut out, DOS_DATE_1980); // last-modified date
    le32(&mut out, crc);
    le32(&mut out, compressed_len);
    le32(&mut out, uncompressed);
    le16(&mut out, name.len() as u16);
    le16(&mut out, 0); // extra field length
    le16(&mut out, 0); // file comment length
    le16(&mut out, 0); // disk number start
    le16(&mut out, 0); // internal attributes
    le32(&mut out, 0); // external attributes
    le32(&mut out, 0); // local header offset
    out.extend_from_slice(name);
    let cd_size = out.len() as u32 - cd_offset;

    // End of central directory record.
    le32(&mut out, 0x0605_4b50);
    le16(&mut out, 0); // this disk number
    le16(&mut out, 0); // disk with central directory
    le16(&mut out, 1); // central directory entries on this disk
    le16(&mut out, 1); // total central directory entries
    le32(&mut out, cd_size);
    le32(&mut out, cd_offset);
    le16(&mut out, 0); // archive comment length

    out
}

#[cfg(test)]
mod deflate_tests {
    use std::io::{Read, Write};

    use flate2::{Decompress, FlushDecompress, Status};
    use zip::read::ZipArchive;
    use zip::write::{SimpleFileOptions, ZipWriter};

    use super::*;

    /// Decompress a raw-deflate stream in one pass, the way the ZIP
    /// reader does, so a `pigz` stream is checked against the same
    /// decoder that reads a single-threaded one. Panics when the stream
    /// ends without its final block.
    fn inflate_raw(compressed: &[u8], expected_len: usize) -> Vec<u8> {
        let mut decompressor = Decompress::new(false);
        let mut out = Vec::with_capacity(expected_len);
        let mut scratch = vec![0u8; 128 * 1024];
        let mut consumed = 0usize;
        loop {
            let before_in = decompressor.total_in();
            let before_out = decompressor.total_out();
            let status = decompressor
                .decompress(&compressed[consumed..], &mut scratch, FlushDecompress::None)
                .expect("inflate");
            consumed += (decompressor.total_in() - before_in) as usize;
            let produced = (decompressor.total_out() - before_out) as usize;
            out.extend_from_slice(&scratch[..produced]);
            match status {
                Status::StreamEnd => break,
                _ if produced == 0 && consumed >= compressed.len() => {
                    panic!("the stream ends without its final block")
                }
                _ => {}
            }
        }
        out
    }

    /// A byte pattern with enough local redundancy that the compressor
    /// finds cross-chunk back-references, so the dictionary priming is
    /// actually exercised.
    fn sample(len: usize) -> Vec<u8> {
        let mut data = Vec::with_capacity(len);
        let mut state = 0x1234_5678u32;
        while data.len() < len {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let word = format!(
                "<vertex x=\"{}\" y=\"{}\"/>",
                state % 997,
                (state >> 8) % 997
            );
            data.extend_from_slice(word.as_bytes());
        }
        data.truncate(len);
        data
    }

    /// The compressed bytes stored for the only entry of `archive_bytes`.
    fn stored_stream(archive_bytes: Vec<u8>) -> Vec<u8> {
        let mut archive = ZipArchive::new(io::Cursor::new(archive_bytes)).unwrap();
        let mut stream = Vec::new();
        archive
            .by_index_raw(0)
            .unwrap()
            .read_to_end(&mut stream)
            .unwrap();
        stream
    }

    #[test]
    fn single_chunk_stream_round_trips() {
        let data = sample(50_000);
        // A chunk size past the data length forces exactly one chunk.
        let compressed = pigz_deflate(&data, data.len() * 2).unwrap();
        assert_eq!(inflate_raw(&compressed, data.len()), data);
    }

    #[test]
    fn multi_chunk_stream_round_trips() {
        let data = sample(200_000);
        // A small chunk size forces many chunks with dictionary priming.
        let compressed = pigz_deflate(&data, 1000).unwrap();
        assert_eq!(inflate_raw(&compressed, data.len()), data);
    }

    #[test]
    fn empty_input_produces_a_valid_empty_stream() {
        let compressed = pigz_deflate(&[], 1024).unwrap();
        assert_eq!(inflate_raw(&compressed, 0), Vec::<u8>::new());
    }

    #[test]
    fn dictionary_priming_keeps_the_ratio_close_to_a_single_stream() {
        let data = sample(400_000);
        let single = pigz_deflate(&data, data.len() * 2).unwrap();
        let chunked = pigz_deflate(&data, 20_000).unwrap();
        // Both must decompress to the original.
        assert_eq!(inflate_raw(&single, data.len()), data);
        assert_eq!(inflate_raw(&chunked, data.len()), data);
        // Splitting into chunks costs a little ratio; priming keeps it
        // small. Well under 5% on this redundant input.
        let overhead = (chunked.len() as f64 - single.len() as f64) / single.len() as f64;
        assert!(overhead < 0.05, "chunk overhead {overhead} too large");
    }

    #[test]
    fn hand_built_zip_reads_back_through_the_zip_crate() {
        let data = sample(120_000);
        let zip_bytes = parallel_entry("3D/3dmodel.model", &data).unwrap();
        let mut archive = ZipArchive::new(io::Cursor::new(zip_bytes)).unwrap();
        assert_eq!(archive.len(), 1);
        let mut entry = archive.by_index(0).unwrap();
        assert_eq!(entry.name(), "3D/3dmodel.model");
        let mut got = Vec::new();
        entry.read_to_end(&mut got).unwrap();
        assert_eq!(got, data);
    }

    #[test]
    fn entry_splices_into_another_archive_with_raw_copy_file() {
        let mesh = sample(150_000);
        let other = b"a small normally-compressed part".to_vec();
        let entry_zip = parallel_entry("3D/3dmodel.model", &mesh).unwrap();

        let final_zip = {
            let mut writer = ZipWriter::new(io::Cursor::new(Vec::new()));
            let mut source = ZipArchive::new(io::Cursor::new(entry_zip)).unwrap();
            writer.raw_copy_file(source.by_index(0).unwrap()).unwrap();
            writer
                .start_file(
                    "3D/other.model",
                    SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated),
                )
                .unwrap();
            writer.write_all(&other).unwrap();
            writer.finish().unwrap().into_inner()
        };

        let mut archive = ZipArchive::new(io::Cursor::new(final_zip)).unwrap();
        assert_eq!(archive.len(), 2);
        let mut mesh_back = Vec::new();
        archive
            .by_name("3D/3dmodel.model")
            .unwrap()
            .read_to_end(&mut mesh_back)
            .unwrap();
        let mut other_back = Vec::new();
        archive
            .by_name("3D/other.model")
            .unwrap()
            .read_to_end(&mut other_back)
            .unwrap();
        assert_eq!(mesh_back, mesh);
        assert_eq!(other_back, other);
    }

    #[test]
    fn hand_built_zip_follows_the_appnote_layout() {
        let zip = build_single_entry_zip("ab", 0x1122_3344, 0x5566_7788, &[0xC0, 0xDE, 0x01]);
        let expected: Vec<u8> = [
            // Local file header (APPNOTE 4.3.7), at offset 0.
            &[0x50, 0x4b, 0x03, 0x04][..], // signature
            &[20, 0],                      // version needed: 2.0 (Deflate)
            &[0, 0],                       // general purpose flags
            &[8, 0],                       // method: Deflated
            &[0, 0],                       // last-modified time
            &[0x21, 0],                    // last-modified date: 1980-01-01
            &[0x44, 0x33, 0x22, 0x11],     // CRC-32
            &[3, 0, 0, 0],                 // compressed size
            &[0x88, 0x77, 0x66, 0x55],     // uncompressed size
            &[2, 0],                       // file name length
            &[0, 0],                       // extra field length
            b"ab",
            &[0xC0, 0xDE, 0x01], // file data
            // Central directory header (APPNOTE 4.3.12), at offset 35.
            &[0x50, 0x4b, 0x01, 0x02], // signature
            &[20, 0],                  // version made by: 2.0, MS-DOS
            &[20, 0],                  // version needed: 2.0 (Deflate)
            &[0, 0],                   // general purpose flags
            &[8, 0],                   // method: Deflated
            &[0, 0],                   // last-modified time
            &[0x21, 0],                // last-modified date: 1980-01-01
            &[0x44, 0x33, 0x22, 0x11], // CRC-32
            &[3, 0, 0, 0],             // compressed size
            &[0x88, 0x77, 0x66, 0x55], // uncompressed size
            &[2, 0],                   // file name length
            &[0, 0],                   // extra field length
            &[0, 0],                   // file comment length
            &[0, 0],                   // disk number start
            &[0, 0],                   // internal file attributes
            &[0, 0, 0, 0],             // external file attributes
            &[0, 0, 0, 0],             // offset of the local header
            b"ab",
            // End of central directory record (APPNOTE 4.3.16).
            &[0x50, 0x4b, 0x05, 0x06], // signature
            &[0, 0],                   // number of this disk
            &[0, 0],                   // disk with the central directory
            &[1, 0],                   // entries on this disk
            &[1, 0],                   // entries in total
            &[48, 0, 0, 0],            // size of the central directory
            &[35, 0, 0, 0],            // offset of the central directory
            &[0, 0],                   // comment length
        ]
        .concat();
        assert_eq!(zip, expected);
    }

    #[test]
    fn parallel_entry_records_the_crc_and_size_of_the_part() {
        let zip_bytes = parallel_entry("p", b"123456789").unwrap();
        let mut archive = ZipArchive::new(io::Cursor::new(zip_bytes)).unwrap();
        let entry = archive.by_index(0).unwrap();
        // The standard check value of CRC-32 for these 9 bytes.
        assert_eq!(entry.crc32(), 0xCBF4_3926);
        assert_eq!(entry.size(), 9);
    }

    #[test]
    fn a_single_chunk_is_compressed_as_the_zip_writer_does() {
        let data = sample(3000);
        let zip_writer_stream = {
            let mut writer = ZipWriter::new(io::Cursor::new(Vec::new()));
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            writer.start_file("p", options).unwrap();
            writer.write_all(&data).unwrap();
            stored_stream(writer.finish().unwrap().into_inner())
        };
        // 1 chunk has no chunk boundary, so the level alone decides the bytes.
        assert_eq!(pigz_deflate(&data, data.len()).unwrap(), zip_writer_stream);
        let entry = parallel_entry("p", &data).unwrap();
        assert_eq!(stored_stream(entry), zip_writer_stream);
    }

    #[test]
    fn a_last_chunk_of_1_byte_round_trips() {
        let data = sample(1001);
        let compressed = pigz_deflate(&data, 1000).unwrap();
        assert_eq!(inflate_raw(&compressed, data.len()), data);
    }

    #[test]
    fn a_chunk_size_of_0_round_trips() {
        let data = sample(300);
        let compressed = pigz_deflate(&data, 0).unwrap();
        assert_eq!(inflate_raw(&compressed, data.len()), data);
    }

    #[test]
    fn a_large_incompressible_chunk_round_trips() {
        // Pseudo-random bytes do not compress: their stream fills the output
        // buffer of `deflate_chunk` more than once.
        let mut state = 0x9E37_79B9u32;
        let data: Vec<u8> = (0..1 << 20)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 24) as u8
            })
            .collect();
        let compressed = pigz_deflate(&data, data.len()).unwrap();
        assert_eq!(inflate_raw(&compressed, data.len()), data);
    }
}
