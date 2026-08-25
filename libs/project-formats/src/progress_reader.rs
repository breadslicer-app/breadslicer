/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Byte-position progress adapter for the linear mesh readers.

use std::io::{Read, Seek, SeekFrom};

/// `Read + Seek` adapter reporting the stream position after every
/// read and seek. Suited to the linear single-pass formats (STL,
/// OBJ), where position over file size is a faithful fraction.
/// Positions are reported raw, backward moves included; smoothing
/// them into a monotonic display is the consumer's concern.
///
/// Wrap the raw `File` and put the `BufReader` on the outside, so
/// reports fire once per buffer refill rather than once per parser
/// read call.
pub(crate) struct ProgressReader<R, F> {
    inner: R,
    position: u64,
    total: u64,
    on_position: F,
}

impl<R: Read + Seek, F: FnMut(u64, u64)> ProgressReader<R, F> {
    /// Wrap `inner`, learning the total by seeking to the end and
    /// back to the start. `on_position` receives `(position, total)`
    /// in bytes.
    pub(crate) fn new(mut inner: R, on_position: F) -> std::io::Result<Self> {
        let total = inner.seek(SeekFrom::End(0))?;
        inner.seek(SeekFrom::Start(0))?;
        Ok(Self {
            inner,
            position: 0,
            total,
            on_position,
        })
    }

    /// Total stream length in bytes, measured at construction.
    pub(crate) fn total_bytes(&self) -> u64 {
        self.total
    }
}

impl<R: Read, F: FnMut(u64, u64)> Read for ProgressReader<R, F> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.position += n as u64;
            (self.on_position)(self.position, self.total);
        }
        Ok(n)
    }
}

impl<R: Seek, F: FnMut(u64, u64)> Seek for ProgressReader<R, F> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.position = self.inner.seek(pos)?;
        (self.on_position)(self.position, self.total);
        Ok(self.position)
    }
}

#[cfg(test)]
mod progress_reader_tests {
    use super::*;
    use std::cell::Cell;
    use std::io::Cursor;

    #[test]
    fn construction_measures_the_total_and_rewinds() {
        let last: Cell<Option<(u64, u64)>> = Cell::new(None);
        let mut reader = ProgressReader::new(Cursor::new(vec![7u8; 10]), |p, t| {
            last.set(Some((p, t)));
        })
        .expect("cursor seeks");
        assert_eq!(reader.total_bytes(), 10);
        assert_eq!(last.get(), None, "no report before the first read");
        let mut all = Vec::new();
        reader.read_to_end(&mut all).expect("read");
        assert_eq!(all.len(), 10, "the rewind restored the start");
    }

    #[test]
    fn reading_reports_the_position_fraction() {
        let last: Cell<Option<(u64, u64)>> = Cell::new(None);
        let mut reader = ProgressReader::new(Cursor::new(vec![7u8; 10]), |p, t| {
            last.set(Some((p, t)));
        })
        .expect("cursor seeks");
        let mut half = [0u8; 5];
        reader.read_exact(&mut half).expect("read");
        assert_eq!(last.get(), Some((5, 10)));
        let mut rest = Vec::new();
        reader.read_to_end(&mut rest).expect("read");
        assert_eq!(last.get(), Some((10, 10)));
    }

    #[test]
    fn seeking_forward_reports_the_new_position() {
        let last: Cell<Option<(u64, u64)>> = Cell::new(None);
        let mut reader = ProgressReader::new(Cursor::new(vec![7u8; 10]), |p, t| {
            last.set(Some((p, t)));
        })
        .expect("cursor seeks");
        reader.seek(SeekFrom::Start(9)).expect("seek");
        assert_eq!(last.get(), Some((9, 10)));
    }

    #[test]
    fn a_backward_seek_reports_the_lower_position_raw() {
        // The STL sniffer probes the header and rewinds; the raw
        // report is this adapter's contract, and the GUI's progress
        // sink absorbs the regression on its side.
        let last: Cell<Option<(u64, u64)>> = Cell::new(None);
        let mut reader = ProgressReader::new(Cursor::new(vec![7u8; 10]), |p, t| {
            last.set(Some((p, t)));
        })
        .expect("cursor seeks");
        let mut probe = [0u8; 8];
        reader.read_exact(&mut probe).expect("read");
        assert_eq!(last.get(), Some((8, 10)));
        reader.seek(SeekFrom::Start(0)).expect("rewind");
        assert_eq!(last.get(), Some((0, 10)));
    }
}
