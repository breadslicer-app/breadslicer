/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::disclaimer::write_disclaimer;
use super::layout::DISCLAIMER_SIZE;

fn written() -> Vec<u8> {
    let mut bytes = Vec::new();
    write_disclaimer(&mut bytes).unwrap();
    bytes
}

#[test]
fn the_block_is_the_size_the_layout_reserves_for_it() {
    assert_eq!(written().len(), DISCLAIMER_SIZE as usize);
}

#[test]
fn the_text_is_ascii_so_its_length_is_its_character_count() {
    assert!(written().is_ascii());
}

#[test]
fn the_text_is_the_vendors_own_wording() {
    let bytes = written();
    let text = std::str::from_utf8(&bytes).unwrap();

    assert!(
        text.starts_with(
            "Layout and record format for the ctb and cbddlp file types are the copyrighted",
        ),
        "{text}",
    );
    assert!(
        text.ends_with("or sublicense the said programs or codes."),
        "{text}",
    );
}

#[test]
fn the_doubled_stop_after_the_company_name_is_preserved() {
    let bytes = written();
    let text = std::str::from_utf8(&bytes).unwrap();

    assert!(
        text.contains("CBD Technology (China) Inc..The Customer"),
        "{text}"
    );
}
