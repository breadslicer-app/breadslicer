/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::layout::{
    HEADER_SIZE, LAYER_TABLE_RECORD_SIZE, Layout, PREVIEW_RECORD_SIZE, PRINT_PARAMETERS_SIZE,
    PRINT_PARAMETERS_V4_SIZE, SLICER_INFO_SIZE, VariableSections,
};
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
    assert_eq!(HEADER_SIZE, 112);
    assert_eq!(PREVIEW_RECORD_SIZE, 32);
    assert_eq!(PRINT_PARAMETERS_SIZE, 60);
    assert_eq!(SLICER_INFO_SIZE, 76);
    assert_eq!(DISCLAIMER_SIZE, 320);
    assert_eq!(PRINT_PARAMETERS_V4_SIZE, 464);
    assert_eq!(LAYER_TABLE_RECORD_SIZE, 36);
}

#[test]
fn the_first_preview_record_follows_the_header() {
    assert_eq!(Layout::plan(variable()).large_preview_record, HEADER_SIZE);
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

    assert_eq!(
        layout.small_preview_record,
        layout.large_preview_image + variable.large_preview_bytes,
    );
    assert_eq!(
        layout.print_parameters,
        layout.small_preview_image + variable.small_preview_bytes,
    );
    assert_eq!(
        layout.slicer_info,
        layout.print_parameters + PRINT_PARAMETERS_SIZE,
    );
    assert_eq!(layout.machine_name, layout.slicer_info + SLICER_INFO_SIZE);
    assert_eq!(
        layout.disclaimer,
        layout.machine_name + variable.machine_name_bytes,
    );
    assert_eq!(
        layout.print_parameters_v4,
        layout.disclaimer + DISCLAIMER_SIZE,
    );
    assert_eq!(
        layout.resin_parameters,
        layout.print_parameters_v4 + PRINT_PARAMETERS_V4_SIZE,
    );
    assert_eq!(
        layout.layer_table,
        layout.resin_parameters + variable.resin_parameters_bytes,
    );
}

#[test]
fn the_layer_table_holds_one_record_per_layer() {
    let layout = Layout::plan(variable());

    assert_eq!(
        layout.layer_data - layout.layer_table,
        LAYER_TABLE_RECORD_SIZE * 7,
    );
}

#[test]
fn a_program_without_layers_ends_its_table_where_it_starts() {
    let layout = Layout::plan(VariableSections {
        layer_count: 0,
        ..variable()
    });

    assert_eq!(layout.layer_data, layout.layer_table);
}

#[test]
fn a_longer_preview_moves_every_section_after_it() {
    let mut grown = variable();
    grown.large_preview_bytes += 1_000;
    let before = Layout::plan(variable());
    let after = Layout::plan(grown);

    assert_eq!(after.large_preview_record, before.large_preview_record);
    assert_eq!(after.large_preview_image, before.large_preview_image);
    assert_eq!(
        after.small_preview_record,
        before.small_preview_record + 1_000
    );
    assert_eq!(after.layer_data, before.layer_data + 1_000);
}

#[test]
fn the_sections_run_in_the_order_the_offsets_chain_them() {
    let layout = Layout::plan(variable());
    let offsets = [
        layout.large_preview_record,
        layout.large_preview_image,
        layout.small_preview_record,
        layout.small_preview_image,
        layout.print_parameters,
        layout.slicer_info,
        layout.machine_name,
        layout.disclaimer,
        layout.print_parameters_v4,
        layout.resin_parameters,
        layout.layer_table,
        layout.layer_data,
    ];

    assert!(
        offsets.windows(2).all(|pair| pair[0] < pair[1]),
        "{offsets:?}",
    );
}
