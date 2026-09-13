/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The variable-length block format version 5 added.
//!
//! 40 fixed bytes then 3 strings laid end to end. Each address is
//! followed by its own length and a string carries no terminator, so
//! the length is the whole of it. This is the only place in the file
//! where the material reaches the machine's display.

use std::io::{self, Write};

use super::layout::Layout;
use crate::ctb::MM3_PER_ML;
use crate::ctb::wire::{write_f32, write_u32};
use crate::print_program::PrintProgram;

/// Bytes before the first string.
const FIXED_SIZE: u32 = 40;

/// Blue, green, red, alpha. Nothing upstream carries a material
/// colour, and the machine treats zero as unset.
const COLOUR_BGRA: [u8; 4] = [0, 0, 0, 0];

/// How many bytes the block will occupy for `program`.
pub fn byte_length(program: &PrintProgram) -> u32 {
    let [first, second, third] = strings(program);
    FIXED_SIZE + text_len(first) + text_len(second) + text_len(third)
}

/// Write the resin parameters for `program`, whose own address
/// `layout` holds because the block locates its strings absolutely.
///
/// # Errors
///
/// Propagates `out`'s failures.
pub fn write_resin_parameters(
    program: &PrintProgram,
    layout: &Layout,
    out: &mut impl Write,
) -> io::Result<()> {
    let texts = strings(program);
    let mut address = layout.resin_parameters + FIXED_SIZE;

    write_u32(out, 0)?;
    out.write_all(&COLOUR_BGRA)?;
    for text in texts {
        write_u32(out, address)?;
        write_u32(out, text_len(text))?;
        address += text_len(text);
    }
    write_f32(out, density_g_per_ml(program))?;
    write_u32(out, 0)?;
    for text in texts {
        out.write_all(text.as_bytes())?;
    }
    Ok(())
}

/// The 3 strings in the order the block lays them out. Which of the
/// 3 a machine shows where is not established by any file read so
/// far, so the material takes the 2 slots a real file fills with one
/// repeated value.
fn strings(program: &PrintProgram) -> [&str; 3] {
    [
        program.material_name.as_str(),
        program.material_name.as_str(),
        program.machine_name.as_str(),
    ]
}

fn text_len(text: &str) -> u32 {
    u32::try_from(text.len()).unwrap_or(u32::MAX)
}

/// What the material weighs per millilitre. A plate that encloses
/// nothing has no density to report.
fn density_g_per_ml(program: &PrintProgram) -> f32 {
    let millilitres = program.volume_mm3.value() / MM3_PER_ML;
    if millilitres > 0.0 {
        (program.mass_g.value() / millilitres) as f32
    } else {
        0.0
    }
}
