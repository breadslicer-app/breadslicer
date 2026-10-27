/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Little-endian writing and bounds-checked reading of chunk payloads.

/// Appends values to a payload.
#[derive(Debug, Default)]
pub(crate) struct Out(pub(crate) Vec<u8>);

impl Out {
    pub(crate) fn u8(&mut self, value: u8) {
        self.0.push(value);
    }

    pub(crate) fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn f64(&mut self, value: f64) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    /// A count or index; the payloads never hold more than `u32::MAX`.
    pub(crate) fn len(&mut self, value: usize) {
        self.u32(u32::try_from(value).expect("fewer than 2^32 items"));
    }

    pub(crate) fn bytes(&mut self, bytes: &[u8]) {
        self.len(bytes.len());
        self.0.extend_from_slice(bytes);
    }

    pub(crate) fn text(&mut self, text: Option<&str>) {
        match text {
            None => self.u8(0),
            Some(text) => {
                self.u8(1);
                self.bytes(text.as_bytes());
            }
        }
    }
}

/// Reads values back; `None` when the payload ends early or is malformed.
#[derive(Debug)]
pub(crate) struct In<'a>(pub(crate) &'a [u8]);

impl<'a> In<'a> {
    pub(crate) fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if n > self.0.len() {
            return None;
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Some(head)
    }

    pub(crate) fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    pub(crate) fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    pub(crate) fn f64(&mut self) -> Option<f64> {
        Some(f64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    pub(crate) fn len(&mut self) -> Option<usize> {
        usize::try_from(self.u32()?).ok()
    }

    /// A count of items at least `min_size` bytes each, so that a damaged
    /// count cannot ask for more memory than the rest of the payload holds.
    pub(crate) fn count(&mut self, min_size: usize) -> Option<usize> {
        let count = self.len()?;
        (count.checked_mul(min_size)? <= self.0.len()).then_some(count)
    }

    pub(crate) fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = self.len()?;
        self.take(n)
    }

    pub(crate) fn text(&mut self) -> Option<Option<String>> {
        match self.u8()? {
            0 => Some(None),
            1 => Some(Some(String::from_utf8(self.bytes()?.to_vec()).ok()?)),
            _ => None,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
