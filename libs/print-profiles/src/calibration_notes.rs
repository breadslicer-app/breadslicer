/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Notes on calibration blocks, as a material profile file carries
//! them: the comment lines right above a `[per_printer.<id>]` header.
//!
//! A comment is not data to TOML, so the loader and the writers go
//! through here to keep a block's note across a load and a save.

use std::collections::BTreeMap;

use toml_edit::DocumentMut;

use crate::calibration_target::CalibrationTarget;

/// The table holding the calibration blocks.
const BLOCKS: &str = "per_printer";

/// Read the note above each calibration block of `document`.
///
/// Only comment lines count, each without its `#` and the one space
/// after it; a block with none above it has no note.
pub(crate) fn read(document: &DocumentMut) -> BTreeMap<CalibrationTarget, String> {
    let Some(blocks) = document.get(BLOCKS).and_then(toml_edit::Item::as_table) else {
        return BTreeMap::new();
    };
    blocks
        .iter()
        .filter_map(|(key, block)| {
            let prefix = block.as_table()?.decor().prefix()?.as_str()?;
            let lines: Vec<&str> = prefix
                .lines()
                .filter_map(|line| line.trim_start().strip_prefix('#'))
                .map(|line| line.strip_prefix(' ').unwrap_or(line))
                .collect();
            let target = CalibrationTarget::try_from(key.to_owned()).ok()?;
            (!lines.is_empty()).then(|| (target, lines.join("\n")))
        })
        .collect()
}

/// Write each of `notes` as comment lines above its block's header,
/// after the whitespace already there.
///
/// A note for a machine `document` has no block for is not written.
pub(crate) fn write(document: &mut DocumentMut, notes: &BTreeMap<CalibrationTarget, String>) {
    let Some(blocks) = document
        .get_mut(BLOCKS)
        .and_then(toml_edit::Item::as_table_mut)
    else {
        return;
    };
    for (target, note) in notes {
        let Some(block) = blocks
            .get_mut(target.as_str())
            .and_then(toml_edit::Item::as_table_mut)
        else {
            continue;
        };
        let mut prefix = block
            .decor()
            .prefix()
            .and_then(|prefix| prefix.as_str())
            .unwrap_or("")
            .to_owned();
        for line in note.lines() {
            prefix.push_str(if line.is_empty() { "#" } else { "# " });
            prefix.push_str(line);
            prefix.push('\n');
        }
        block.decor_mut().set_prefix(prefix);
    }
}
