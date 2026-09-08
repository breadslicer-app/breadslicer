/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! GOO at format version V3.0.
//!
//! Everything multi-byte is big-endian, `f32` values included, which
//! is the opposite of the other printer formats.

pub mod chunk;
pub mod file;
pub mod header;
pub mod payload;
pub mod record;
pub mod wire;

#[cfg(test)]
mod test_decode;
#[cfg(test)]
mod tests_chunk;
#[cfg(test)]
mod tests_file;
#[cfg(test)]
mod tests_header;
#[cfg(test)]
mod tests_payload;
#[cfg(test)]
mod tests_record;
#[cfg(test)]
mod tests_vendor_layer;
