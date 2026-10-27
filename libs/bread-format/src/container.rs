/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The chunked container.
//!
//! Layout, all integers little-endian:
//!
//! ```text
//! header  "BREAD\0\r\n" (8 bytes), container version u16
//! chunk   tag [u8; 4], chunk version u16, raw length u64,
//!         stored length u64, CRC-32 of the stored bytes u32,
//!         stored bytes (zstd)
//! ...     chunks until the end of the file
//! ```
//!
//! The `\r\n` in the magic catches a file mangled by a text-mode transfer,
//! as PNG's does.

use std::fmt;
use std::io::{self, Read, Write};

/// The first bytes of every `.bread` file.
pub const MAGIC: [u8; 8] = *b"BREAD\0\r\n";

/// The container layout this build writes and reads.
pub const CONTAINER_VERSION: u16 = 1;

/// The zstd level chunks are written at: fast, and most of the ratio.
const ZSTD_LEVEL: i32 = 3;

/// A chunk's kind, four ASCII bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ChunkTag(pub [u8; 4]);

impl fmt::Display for ChunkTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&String::from_utf8_lossy(&self.0))
    }
}

/// One chunk read back: its tag, version and decompressed payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub tag: ChunkTag,
    pub version: u16,
    pub payload: Vec<u8>,
}

/// Why a `.bread` file could not be read.
#[derive(Debug)]
pub enum ReadBreadError {
    Io(io::Error),
    /// The file does not start with the `.bread` magic.
    NotBread,
    /// The file was written by a newer container layout.
    UnsupportedVersion(u16),
    /// The file ends inside a chunk.
    Truncated,
    /// A chunk's bytes do not match their checksum.
    Corrupt(ChunkTag),
    /// A chunk's payload does not decompress to its recorded length.
    BadPayload(ChunkTag),
}

impl fmt::Display for ReadBreadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::NotBread => f.write_str("not a .bread file"),
            Self::UnsupportedVersion(v) => {
                write!(f, "written by a newer Breadslicer (container version {v})")
            }
            Self::Truncated => f.write_str("the file is cut short"),
            Self::Corrupt(tag) => write!(f, "the {tag} chunk is damaged"),
            Self::BadPayload(tag) => write!(f, "the {tag} chunk does not decode"),
        }
    }
}

impl std::error::Error for ReadBreadError {}

impl From<io::Error> for ReadBreadError {
    fn from(err: io::Error) -> Self {
        if err.kind() == io::ErrorKind::UnexpectedEof {
            Self::Truncated
        } else {
            Self::Io(err)
        }
    }
}

/// Writes a `.bread` file chunk by chunk.
#[derive(Debug)]
pub struct BreadWriter<W: Write> {
    out: W,
}

impl<W: Write> BreadWriter<W> {
    /// Starts a file: writes the header.
    pub fn new(mut out: W) -> io::Result<Self> {
        out.write_all(&MAGIC)?;
        out.write_all(&CONTAINER_VERSION.to_le_bytes())?;
        Ok(Self { out })
    }

    /// Compresses `payload` and writes it as one chunk.
    pub fn chunk(&mut self, tag: ChunkTag, version: u16, payload: &[u8]) -> io::Result<()> {
        let stored = zstd::bulk::compress(payload, ZSTD_LEVEL)?;
        self.out.write_all(&tag.0)?;
        self.out.write_all(&version.to_le_bytes())?;
        self.out.write_all(&(payload.len() as u64).to_le_bytes())?;
        self.out.write_all(&(stored.len() as u64).to_le_bytes())?;
        self.out
            .write_all(&crc32fast::hash(&stored).to_le_bytes())?;
        self.out.write_all(&stored)
    }

    /// Ends the file, handing back the writer.
    pub fn finish(mut self) -> io::Result<W> {
        self.out.flush()?;
        Ok(self.out)
    }
}

/// Reads a `.bread` file chunk by chunk.
#[derive(Debug)]
pub struct BreadReader<R: Read> {
    input: R,
}

impl<R: Read> BreadReader<R> {
    /// Opens a file: checks the header.
    pub fn new(mut input: R) -> Result<Self, ReadBreadError> {
        let mut magic = [0; 8];
        input
            .read_exact(&mut magic)
            .map_err(|err| match err.kind() {
                io::ErrorKind::UnexpectedEof => ReadBreadError::NotBread,
                _ => ReadBreadError::Io(err),
            })?;
        if magic != MAGIC {
            return Err(ReadBreadError::NotBread);
        }
        let version = read_u16(&mut input)?;
        if version > CONTAINER_VERSION {
            return Err(ReadBreadError::UnsupportedVersion(version));
        }
        Ok(Self { input })
    }

    /// The next chunk, decompressed and checked, or `None` at the end of the
    /// file.
    pub fn next_chunk(&mut self) -> Result<Option<Chunk>, ReadBreadError> {
        let mut tag = [0; 4];
        match read_fully(&mut self.input, &mut tag)? {
            0 => return Ok(None),
            4 => {}
            _ => return Err(ReadBreadError::Truncated),
        }
        let tag = ChunkTag(tag);
        let version = read_u16(&mut self.input)?;
        let raw_len = read_u64(&mut self.input)?;
        let stored_len = read_u64(&mut self.input)?;
        let crc = read_u32(&mut self.input)?;
        let mut stored = Vec::new();
        (&mut self.input)
            .take(stored_len)
            .read_to_end(&mut stored)?;
        if stored.len() as u64 != stored_len {
            return Err(ReadBreadError::Truncated);
        }
        if crc32fast::hash(&stored) != crc {
            return Err(ReadBreadError::Corrupt(tag));
        }
        // Streamed, not preallocated from the recorded length: a damaged
        // length cannot ask for more memory than the payload really holds.
        let mut payload = Vec::new();
        zstd::stream::read::Decoder::new(stored.as_slice())
            .and_then(|decoder| {
                decoder
                    .take(raw_len.saturating_add(1))
                    .read_to_end(&mut payload)
            })
            .map_err(|_| ReadBreadError::BadPayload(tag))?;
        if payload.len() as u64 != raw_len {
            return Err(ReadBreadError::BadPayload(tag));
        }
        Ok(Some(Chunk {
            tag,
            version,
            payload,
        }))
    }
}

/// Reads until `buf` is full or the input ends; returns the bytes read.
fn read_fully(input: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match input.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    Ok(filled)
}

fn read_u16(input: &mut impl Read) -> io::Result<u16> {
    let mut bytes = [0; 2];
    input.read_exact(&mut bytes)?;
    Ok(u16::from_le_bytes(bytes))
}

fn read_u32(input: &mut impl Read) -> io::Result<u32> {
    let mut bytes = [0; 4];
    input.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(input: &mut impl Read) -> io::Result<u64> {
    let mut bytes = [0; 8];
    input.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}
