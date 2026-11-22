/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The support mesh that is sliced keeps its detail however large its set
//! grows (#503): the inherited budget dropped to 3 sides per shaft past
//! 20,000 segments, so a large model printed with thinner supports than its
//! preset designed.

use std::collections::HashSet;

use serde_json::Value;

use support_engine::features::slicing::smooth_shafts::{ShaftStyle, SmoothShaftParams};
use support_engine::features::slicing::support_world_triangles::{
    SUPPORT_SLICE_TESSELLATION, SupportSliceInput, build_support_and_raft_world_triangles,
    resolve_support_slice_tessellation,
};
use support_engine::supports::rafts::crenelated::raft_types::DEFAULT_RAFT_SETTINGS;
use support_engine::supports::types::SupportCollections;

fn load(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// A set of the fixture, the model it holds, and the same set with `copies`
/// more trunks held by a model that is not drawn: past every threshold the
/// old budget counted, without changing what is drawn.
fn small_and_padded(copies: usize) -> (SupportCollections, SupportCollections, String) {
    let fixture = load("islands_c.json");
    let native = load("native_islands.json");
    for run in fixture["runs"].as_array().unwrap() {
        let Some(support) = run["result"].get("support") else {
            continue;
        };
        let state: SupportCollections = serde_json::from_value(support.clone()).unwrap();
        let Some(trunk) = state.trunks.values().next().cloned() else {
            continue;
        };
        let scene = native["runs"][run["run"].as_u64().unwrap() as usize]["scene"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut padded = state.clone();
        for i in 0..copies {
            let mut copy = trunk.clone();
            copy.base.id = format!("padding-{i}");
            copy.base.model_id = "not-drawn".to_owned();
            padded.trunks.insert(copy.base.id.clone(), copy);
        }
        return (state, padded, scene);
    }
    panic!("the fixture holds a set with a trunk");
}

#[test]
fn every_set_size_slices_at_the_full_detail() {
    let (small, padded, _) = small_and_padded(25_000);
    assert_eq!(
        resolve_support_slice_tessellation(&small),
        SUPPORT_SLICE_TESSELLATION
    );
    assert_eq!(
        resolve_support_slice_tessellation(&padded),
        SUPPORT_SLICE_TESSELLATION
    );
    assert!(resolve_support_slice_tessellation(&padded).shaft_radial_segments >= 12);
}

#[test]
fn a_large_set_draws_its_supports_as_a_small_one_does() {
    let (small, padded, scene) = small_and_padded(25_000);
    let visible: HashSet<String> = [scene].into();
    for shaft_style in [
        ShaftStyle::Segmented,
        ShaftStyle::Smooth(SmoothShaftParams::default()),
    ] {
        let soup_of = |state: &SupportCollections| {
            build_support_and_raft_world_triangles(&SupportSliceInput {
                state,
                visible_model_ids: &visible,
                raft: &DEFAULT_RAFT_SETTINGS,
                plate_clearance_models: &[],
                support_tip_shrink_percent: 0.0,
                tip_penetration_mm: 0.0,
                tip_drawing: Default::default(),
                shaft_style,
                draw_rafts: true,
            })
            .unwrap()
        };
        let small = soup_of(&small);
        assert!(!small.is_empty());
        assert_eq!(
            small,
            soup_of(&padded),
            "the set's size changed how its supports are drawn"
        );
    }
}
