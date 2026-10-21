/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Writing the scalar types the container stores, big-endian.

use std::io::{self, Read, Write};

pub fn write_u16(out: &mut impl Write, value: u16) -> io::Result<()> {
    out.write_all(&value.to_be_bytes())
}

pub fn write_u32(out: &mut impl Write, value: u32) -> io::Result<()> {
    out.write_all(&value.to_be_bytes())
}

pub fn write_f32(out: &mut impl Write, value: f32) -> io::Result<()> {
    out.write_all(&value.to_be_bytes())
}

pub fn write_flag(out: &mut impl Write, flag: bool) -> io::Result<()> {
    out.write_all(&[u8::from(flag)])
}

/// Write `text` into a fixed-width slot, cut at a character boundary
/// so at least one zero byte terminates it.
pub fn write_text(out: &mut impl Write, text: &str, width: usize) -> io::Result<()> {
    let mut end = text.len().min(width - 1);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    out.write_all(&text.as_bytes()[..end])?;
    write_zeros(out, width - end)
}

pub fn write_zeros(out: &mut impl Write, count: usize) -> io::Result<()> {
    io::copy(&mut io::repeat(0).take(count as u64), out)?;
    Ok(())
}
