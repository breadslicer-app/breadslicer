/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Writing the scalar types the container stores, little-endian.
//!
//! A layer image's run lengths are the one exception in the family
//! and the layer codec writes those itself.

use std::io::{self, Read, Write};

#[inline]
pub fn write_u8(out: &mut impl Write, value: u8) -> io::Result<()> {
    out.write_all(&[value])
}

#[inline]
pub fn write_u16(out: &mut impl Write, value: u16) -> io::Result<()> {
    out.write_all(&value.to_le_bytes())
}

#[inline]
pub fn write_u32(out: &mut impl Write, value: u32) -> io::Result<()> {
    out.write_all(&value.to_le_bytes())
}

#[inline]
pub fn write_u64(out: &mut impl Write, value: u64) -> io::Result<()> {
    out.write_all(&value.to_le_bytes())
}

#[inline]
pub fn write_f32(out: &mut impl Write, value: f32) -> io::Result<()> {
    out.write_all(&value.to_le_bytes())
}

#[inline]
pub fn write_zeros(out: &mut impl Write, count: usize) -> io::Result<()> {
    io::copy(&mut io::repeat(0).take(count as u64), out)?;
    Ok(())
}

/// Narrow a length or an address to the 32 bits this container
/// counts it in, naming `what` was too large for it.
///
/// # Errors
///
/// [`io::ErrorKind::InvalidInput`] when it does not fit, which for
/// an address means the file outgrew what its own fields can reach.
pub fn counted(value: impl TryInto<u32>, what: &str) -> io::Result<u32> {
    value.try_into().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{what} does not fit the 32 bits this container counts it in"),
        )
    })
}
