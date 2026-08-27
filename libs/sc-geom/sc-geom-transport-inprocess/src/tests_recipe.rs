/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 EntoMind Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use crate::recipe::{AlgorithmId, JobRecipe};

#[test]
fn algorithm_id_displays_as_its_string() {
    let id = AlgorithmId::new("trivial-support-v0");
    assert_eq!(id.as_str(), "trivial-support-v0");
    assert_eq!(id.to_string(), "trivial-support-v0");
}

#[test]
fn get_finds_the_first_matching_key() {
    let recipe = JobRecipe {
        algorithm_id: AlgorithmId::new("trivial-support-v0"),
        algorithm_version: "0.0.1".to_string(),
        resolved_settings: vec![
            ("radius_mm".to_string(), "1.5".to_string()),
            ("anchors".to_string(), "0,0,0".to_string()),
        ],
        input_mesh_hash: "deadbeef".to_string(),
        seeds: Vec::new(),
    };
    assert_eq!(recipe.get("radius_mm"), Some("1.5"));
    assert_eq!(recipe.get("anchors"), Some("0,0,0"));
    assert_eq!(recipe.get("missing"), None);
}
