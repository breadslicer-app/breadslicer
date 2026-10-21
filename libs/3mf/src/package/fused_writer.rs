/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! A writer that stays failed after its first error.

use std::io::{self, Seek, SeekFrom, Write};

/// Passes calls through to `inner` until 1 fails with an error other than
/// `Interrupted`; after that, every write, flush and seek fails with the kind
/// of that error, without reaching `inner`.
///
/// `ZipWriter` tries to finish the archive again when it is dropped after an
/// error. With `zip` 8.6, that 2nd try can panic in debug builds, and it can
/// write into `inner` after the error was reported. Behind this wrapper, it
/// stops at its 1st call.
pub(super) struct FusedWriter<W> {
    inner: W,
    failure: Option<io::ErrorKind>,
}

impl<W> FusedWriter<W> {
    pub(super) fn new(inner: W) -> Self {
        Self {
            inner,
            failure: None,
        }
    }

    fn ensure_not_failed(&self) -> io::Result<()> {
        match self.failure {
            Some(kind) => Err(kind.into()),
            None => Ok(()),
        }
    }

    fn record<T>(&mut self, result: io::Result<T>) -> io::Result<T> {
        // `write_all` and `io::copy` retry an interrupted call.
        if let Err(error) = &result
            && error.kind() != io::ErrorKind::Interrupted
        {
            self.failure = Some(error.kind());
        }
        result
    }
}

impl<W: Write> Write for FusedWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.ensure_not_failed()?;
        let result = self.inner.write(buf);
        self.record(result)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.ensure_not_failed()?;
        let result = self.inner.flush();
        self.record(result)
    }
}

impl<W: Seek> Seek for FusedWriter<W> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.ensure_not_failed()?;
        let result = self.inner.seek(pos);
        self.record(result)
    }
}

#[cfg(test)]
mod fused_writer_tests {
    use super::*;

    /// A writer whose 1st write fails with `failure`, if any; it counts every
    /// call that reaches it.
    #[derive(Default)]
    struct CountingWriter {
        failure: Option<io::ErrorKind>,
        data: Vec<u8>,
        writes: usize,
        flushes: usize,
        seeks: usize,
    }

    impl Write for CountingWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.writes += 1;
            if let Some(kind) = self.failure.take() {
                return Err(kind.into());
            }
            self.data.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flushes += 1;
            Ok(())
        }
    }

    impl Seek for CountingWriter {
        fn seek(&mut self, _: SeekFrom) -> io::Result<u64> {
            self.seeks += 1;
            Ok(0)
        }
    }

    fn failing_with(kind: io::ErrorKind) -> FusedWriter<CountingWriter> {
        FusedWriter::new(CountingWriter {
            failure: Some(kind),
            ..CountingWriter::default()
        })
    }

    #[test]
    fn passes_calls_through_until_one_fails() {
        let mut writer = FusedWriter::new(CountingWriter::default());
        writer.write_all(b"abc").unwrap();
        writer.flush().unwrap();
        writer.seek(SeekFrom::Start(1)).unwrap();
        writer.write_all(b"d").unwrap();
        let inner = &writer.inner;
        assert_eq!(inner.data, b"abcd");
        assert_eq!((inner.writes, inner.flushes, inner.seeks), (2, 1, 1));
    }

    #[test]
    fn refuses_every_call_after_a_failure_without_reaching_the_writer() {
        let mut writer = failing_with(io::ErrorKind::StorageFull);
        let kinds = [
            writer.write(b"abc").unwrap_err().kind(),
            writer.write(b"abc").unwrap_err().kind(),
            writer.flush().unwrap_err().kind(),
            writer.seek(SeekFrom::Start(0)).unwrap_err().kind(),
        ];
        assert_eq!(kinds, [io::ErrorKind::StorageFull; 4]);
        let inner = &writer.inner;
        assert_eq!((inner.writes, inner.flushes, inner.seeks), (1, 0, 0));
    }

    #[test]
    fn an_interrupted_call_is_not_a_failure() {
        let mut writer = failing_with(io::ErrorKind::Interrupted);
        writer.write_all(b"abc").unwrap();
        writer.flush().unwrap();
        assert_eq!(writer.inner.data, b"abc");
    }
}
