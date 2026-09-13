/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! CTB at format version 5, encrypted.
//!
//! The revision the recent Elegoo machines declare. It keeps the
//! plain revision's codecs, disclaimer and resin block, and replaces
//! everything else: one encrypted settings block instead of 4 plain
//! ones, a pointer table in front of the layer records, a second
//! pass of encryption over part of some layer images, and a
//! signature and checksum closing the file.

pub mod header;
pub mod layout;

#[cfg(test)]
mod tests_header;
#[cfg(test)]
mod tests_layout;
