/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The ChiTuBox CTB container family.
//!
//! One skeleton of offset-linked sections carries several revisions,
//! which share the layer codec and differ in their fixed blocks and in
//! whether the settings block is encrypted. Everything multi-byte is
//! little-endian except a layer run's length.

pub mod rle15;
pub mod rle7;
pub mod v5;

#[cfg(test)]
mod tests_rle15;
#[cfg(test)]
mod tests_rle7;
