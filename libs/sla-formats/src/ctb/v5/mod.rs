/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! CTB at format version 5, unencrypted.
//!
//! The revision that added the resin parameters block to the v4
//! skeleton. Its sections are linked by absolute file offset rather
//! than laid out in a fixed order, so a writer measures the chain
//! before it writes the header that points into it.

pub mod header;
pub mod layout;

#[cfg(test)]
mod tests_header;
#[cfg(test)]
mod tests_layout;
