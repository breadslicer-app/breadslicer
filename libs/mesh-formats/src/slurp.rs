/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

//! Whole-document read shared by the readers that parse from a byte
//! slice instead of a line stream.

use std::io::{Read, Seek, SeekFrom};

/// Reads the remaining stream into memory, learning the byte count
/// with 2 seeks and reserving it up front: without the reservation,
/// `read_to_end` through a non-specialized reader (the GUI wraps the
/// file in a progress-counting adapter) degrades to the growth loop
/// and costs about twice the ideal read. The loader enforces the
/// file-size cap upstream, so peak memory is bounded by that cap.
pub(crate) fn slurp<R: Read + Seek>(read: &mut R) -> std::io::Result<Vec<u8>> {
    let start = read.stream_position()?;
    let end = read.seek(SeekFrom::End(0))?;
    read.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity(end.saturating_sub(start) as usize);
    read.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod slurp_tests {
    use std::io::{Cursor, Read, Seek, SeekFrom};

    use super::slurp;

    /// A reader with no `read_to_end` specialization and no size hint,
    /// standing in for the GUI's progress-counting wrapper.
    struct OpaqueReader {
        inner: Cursor<Vec<u8>>,
    }

    impl Read for OpaqueReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            // One byte at a time: nothing here can be specialized away.
            let end = 1.min(buf.len());
            self.inner.read(&mut buf[..end])
        }
    }

    impl Seek for OpaqueReader {
        fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
            self.inner.seek(pos)
        }
    }

    #[test]
    fn reads_the_whole_stream_from_the_start() {
        let mut reader = Cursor::new(vec![1_u8, 2, 3, 4]);
        assert_eq!(slurp(&mut reader).unwrap(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn reads_only_the_remainder_from_the_current_position() {
        let mut reader = Cursor::new(vec![1_u8, 2, 3, 4]);
        reader.set_position(2);
        assert_eq!(slurp(&mut reader).unwrap(), vec![3, 4]);
    }

    #[test]
    fn reserves_the_exact_remaining_size() {
        let mut reader = OpaqueReader {
            inner: Cursor::new(vec![7_u8; 100]),
        };
        reader.seek(SeekFrom::Start(40)).unwrap();
        let bytes = slurp(&mut reader).unwrap();
        assert_eq!(bytes.len(), 60);
        assert_eq!(bytes.capacity(), 60);
    }

    #[test]
    fn an_empty_stream_yields_no_bytes() {
        let mut reader = Cursor::new(Vec::new());
        assert!(slurp(&mut reader).unwrap().is_empty());
    }
}
