/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::layout::{
    HEADER_SIZE, LAYER_POINTER_SIZE, LAYER_RECORD_SIZE, Layout, POST_LAYER_MARKER_SIZE,
    PREVIEW_RECORD_SIZE, SETTINGS_SIZE, SIGNATURE_SIZE, TRAILER_SIZE, VariableSections,
};
use crate::ctb::cipher::BLOCK_BYTES;
use crate::ctb::disclaimer::DISCLAIMER_SIZE;

/// Lengths that are all distinct, so a test can tell which one an
/// offset moved by.
fn variable() -> VariableSections {
    VariableSections {
        large_preview_bytes: 5_000,
        small_preview_bytes: 900,
        machine_name_bytes: 20,
        resin_parameters_bytes: 64,
        layer_count: 7,
    }
}

#[test]
fn the_fixed_blocks_are_the_sizes_the_format_states() {
    assert_eq!(HEADER_SIZE, 48);
    assert_eq!(SETTINGS_SIZE, 288);
    assert_eq!(PREVIEW_RECORD_SIZE, 16);
    assert_eq!(DISCLAIMER_SIZE, 320);
    assert_eq!(LAYER_POINTER_SIZE, 16);
    assert_eq!(LAYER_RECORD_SIZE, 88);
    assert_eq!(POST_LAYER_MARKER_SIZE, 8);
    assert_eq!(SIGNATURE_SIZE, 32);
    assert_eq!(TRAILER_SIZE, 4);
}

#[test]
fn the_encrypted_blocks_are_whole_numbers_of_cipher_blocks() {
    assert_eq!(SETTINGS_SIZE as usize % BLOCK_BYTES, 0);
    assert_eq!(SIGNATURE_SIZE as usize % BLOCK_BYTES, 0);
}

#[test]
fn the_settings_block_follows_the_header() {
    assert_eq!(Layout::plan(variable()).settings, HEADER_SIZE);
}

#[test]
fn each_preview_image_follows_its_own_record() {
    let layout = Layout::plan(variable());

    assert_eq!(
        layout.large_preview_image,
        layout.large_preview_record + PREVIEW_RECORD_SIZE,
    );
    assert_eq!(
        layout.small_preview_image,
        layout.small_preview_record + PREVIEW_RECORD_SIZE,
    );
}

#[test]
fn every_section_abuts_the_one_before_it() {
    let variable = variable();
    let layout = Layout::plan(variable);

    assert_eq!(layout.large_preview_record, layout.settings + SETTINGS_SIZE);
    assert_eq!(
        layout.small_preview_record,
        layout.large_preview_image + variable.large_preview_bytes,
    );
    assert_eq!(
        layout.machine_name,
        layout.small_preview_image + variable.small_preview_bytes,
    );
    assert_eq!(
        layout.disclaimer,
        layout.machine_name + variable.machine_name_bytes,
    );
    assert_eq!(layout.resin_parameters, layout.disclaimer + DISCLAIMER_SIZE);
    assert_eq!(
        layout.layer_pointer_table,
        layout.resin_parameters + variable.resin_parameters_bytes,
    );
}

#[test]
fn the_layer_data_follows_a_pointer_per_layer() {
    let variable = variable();
    let layout = Layout::plan(variable);

    assert_eq!(
        layout.layer_data - layout.layer_pointer_table,
        LAYER_POINTER_SIZE * variable.layer_count,
    );
}

#[test]
fn a_file_with_no_layers_puts_its_layer_data_where_its_table_would_start() {
    let layout = Layout::plan(VariableSections {
        layer_count: 0,
        ..variable()
    });

    assert_eq!(layout.layer_data, layout.layer_pointer_table);
}

/// The chain a file written by the vendor's own slicer records,
/// which fixes every constant above against a real file: 2 previews
/// of 41,776 bytes at 273 x 500, a 19-byte machine name, a 59-byte
/// resin block and 960 layers.
#[test]
fn the_chain_reproduces_a_real_files_own_offsets() {
    let layout = Layout::plan(VariableSections {
        large_preview_bytes: 41_776,
        small_preview_bytes: 41_776,
        machine_name_bytes: 19,
        resin_parameters_bytes: 59,
        layer_count: 960,
    });

    assert_eq!(layout.large_preview_record, 336);
    assert_eq!(layout.large_preview_image, 352);
    assert_eq!(layout.small_preview_record, 42_128);
    assert_eq!(layout.small_preview_image, 42_144);
    assert_eq!(layout.machine_name, 83_920);
    assert_eq!(layout.disclaimer, 83_939);
    assert_eq!(layout.resin_parameters, 84_259);
    assert_eq!(layout.layer_pointer_table, 84_318);
    assert_eq!(layout.layer_data, 99_678);
}
