/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The run-length chunk grammar of a GOO layer image.
//!
//! A chunk's tag byte is `TT SS CCCC`: the pair `TT` names the run's
//! colour, `SS` counts the length bytes that follow, and `CCCC` holds
//! the run length's low nibble. The length bytes carry the rest of it,
//! most significant first.
//!
//! The delta type reads the same tag differently: after `TT`, one bit
//! gives the step's sign, the next says whether a run length byte
//! follows, and `CCCC` is the step's magnitude rather than a length.

/// Chunk type bits, occupying the tag byte's top 2 bits.
const TYPE_BLACK_RUN: u8 = 0b0000_0000;
const TYPE_GREY_RUN: u8 = 0b0100_0000;
const TYPE_DELTA_RUN: u8 = 0b1000_0000;
const TYPE_WHITE_RUN: u8 = 0b1100_0000;

/// Set on a delta chunk whose step goes down.
const DELTA_NEGATIVE: u8 = 0b0010_0000;
/// Set on a delta chunk carrying its run length in a second byte.
const DELTA_RUN_BYTE: u8 = 0b0001_0000;
/// Widest step a delta chunk's magnitude nibble expresses.
const MAX_DELTA_MAGNITUDE: u8 = 0x0F;

const BLACK: u8 = 0x00;
const WHITE: u8 = 0xFF;

/// Longest pixel run a single chunk expresses.
pub const MAX_RUN_LENGTH: u32 = 0x0FFF_FFFF;

/// Encoder for one layer image's chunk stream, tracking the value the
/// last chunk carried.
///
/// One instance per image: no chunk may encode relative to a pixel of
/// a different image, so the tracked value must not outlive one.
#[derive(Debug, Default)]
pub struct RunEncoder {
    previous: Option<u8>,
}

impl RunEncoder {
    /// Append the chunks covering `length` pixels of `value` to
    /// `out`, splitting a run longer than [`MAX_RUN_LENGTH`].
    pub fn push_run(&mut self, value: u8, length: u32, out: &mut Vec<u8>) {
        let mut remaining = length;
        while remaining > 0 {
            let chunk_length = remaining.min(MAX_RUN_LENGTH);
            match self.shorter_delta(value, chunk_length) {
                Some(delta) => delta.push(out),
                None => push_absolute_chunk(value, chunk_length, out),
            }
            self.previous = Some(value);
            remaining -= chunk_length;
        }
    }

    /// Build the delta chunk for this run, when one can express it and
    /// is strictly shorter than the black, grey or white chunk would
    /// be. Ties go to the other form, which keeps the emitted stream
    /// matching what the format's own producers write.
    fn shorter_delta(&self, value: u8, length: u32) -> Option<DeltaChunk> {
        // Black and white have chunk types of their own that a delta
        // can never beat, so only a grey run is worth pricing.
        if !is_grey(value) {
            return None;
        }
        let previous = self.previous?;
        let magnitude = value.abs_diff(previous);
        if magnitude > MAX_DELTA_MAGNITUDE {
            return None;
        }
        let run = match length {
            1 => None,
            _ => Some(u8::try_from(length).ok()?),
        };
        let delta_len = if run.is_some() { 2 } else { 1 };
        (delta_len < absolute_chunk_len(value, length)).then_some(DeltaChunk {
            negative: value < previous,
            magnitude,
            run,
        })
    }
}

/// A step from the previous pixel, with the run length when that
/// needs a byte of its own.
struct DeltaChunk {
    negative: bool,
    magnitude: u8,
    run: Option<u8>,
}

impl DeltaChunk {
    fn push(&self, out: &mut Vec<u8>) {
        let mut tag = TYPE_DELTA_RUN | self.magnitude;
        if self.negative {
            tag |= DELTA_NEGATIVE;
        }
        match self.run {
            Some(run) => {
                out.push(tag | DELTA_RUN_BYTE);
                out.push(run);
            }
            None => out.push(tag),
        }
    }
}

fn push_absolute_chunk(value: u8, length: u32, out: &mut Vec<u8>) {
    let length_bytes = length_byte_count(length);
    let tag_tail = (length_bytes << 4) | ((length & 0x0F) as u8);

    match value {
        BLACK => out.push(TYPE_BLACK_RUN | tag_tail),
        WHITE => out.push(TYPE_WHITE_RUN | tag_tail),
        grey => {
            out.push(TYPE_GREY_RUN | tag_tail);
            out.push(grey);
        }
    }

    for position in (0..length_bytes).rev() {
        let shift = 4 + 8 * u32::from(position);
        out.push(((length >> shift) & 0xFF) as u8);
    }
}

/// Count the bytes the black, grey or white chunk for this run takes.
fn absolute_chunk_len(value: u8, length: u32) -> usize {
    1 + usize::from(length_byte_count(length)) + usize::from(is_grey(value))
}

fn is_grey(value: u8) -> bool {
    value != BLACK && value != WHITE
}

/// Count the length bytes a run needs beyond the tag byte's nibble.
fn length_byte_count(length: u32) -> u8 {
    match length {
        0..=0xF => 0,
        0x10..=0xFFF => 1,
        0x1000..=0xF_FFFF => 2,
        _ => 3,
    }
}
