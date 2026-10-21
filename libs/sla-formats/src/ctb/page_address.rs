/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! An address as the records of this family store one.
//!
//! Every offset field is 32 bits wide while a file of tens of
//! thousands of layers on a 16K panel passes 4 GiB, so a record
//! carries a page number beside the offset inside that page.

/// How much of a file one page covers.
pub const PAGE_BYTES: u64 = 1 << u32::BITS;

/// An absolute file address, split as a record stores it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageAddress {
    pub page: u32,
    pub offset: u32,
}

impl PageAddress {
    /// Split `address` into the page it falls in and the offset
    /// inside that page.
    pub fn of(address: u64) -> Self {
        Self {
            page: (address >> u32::BITS) as u32,
            offset: address as u32,
        }
    }
}
