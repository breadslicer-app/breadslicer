//! Parity with DragonFruit for block 8c, mesh to braced forest:
//! `fixtures/df_islands_c.json` (`scripts/df-oracle/golden_islands_c.ts`,
//! DragonFruit f331fc55, `uuid` replaced by one counter for the whole run)
//! derives each native scan's islands with the Islands panel's defaults and
//! runs `computeAutoSupportPlan` on the scene with the default settings and
//! auto-bracing, as the Auto Supports panel does. The port takes the same scan
//! (the combined scan in DragonFruit's voxel order, the soup scan recomputed),
//! derives with `derive_islands` and plans with `compute_auto_support_plan`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use support_gen_df::native::mesh_minima::CombinedIslandScanResult;
use support_gen_df::native::overhang::{OverhangScan, overhang_and_stability_from_soup};
use support_gen_df::supports::auto_bracing::mesh_clearance::{BraceMesh, BraceMeshes};
use support_gen_df::supports::auto_support::auto_place::helpers::{PlanCtx, PlanMesh};
use support_gen_df::supports::auto_support::auto_place::plan::{
    PlanInput, compute_auto_support_plan,
};
use support_gen_df::supports::auto_support::parameter_sizing::SizingBands;
use support_gen_df::supports::placement_logic::pathfinding::sdf_cache::SdfCache;
use support_gen_df::supports::settings::types::SupportSettings;
use support_gen_df::supports::types::SupportState;
use support_gen_df::three::Matrix4;
use support_gen_df::three_mesh_bvh::{MeshBvh, Side};
use support_gen_df::volume_analysis::islands::use_islands::{IslandPanelSettings, derive_islands};

fn load(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn assert_same(actual: &Value, expected: &Value, path: &str) {
    match (actual, expected) {
        (Value::Number(a), Value::Number(e)) => {
            assert!(
                a.as_f64() == e.as_f64(),
                "{path}: port {a} vs DragonFruit {e}"
            );
        }
        (Value::Object(a), Value::Object(e)) => {
            let mut ak: Vec<&String> = a.keys().collect();
            let mut ek: Vec<&String> = e.keys().collect();
            ak.sort();
            ek.sort();
            assert_eq!(ak, ek, "{path}: fields differ");
            for (k, ev) in e {
                assert_same(&a[k], ev, &format!("{path}.{k}"));
            }
        }
        (Value::Array(a), Value::Array(e)) => {
            assert_eq!(a.len(), e.len(), "{path}: lengths differ");
            for (i, (a, e)) in a.iter().zip(e).enumerate() {
                assert_same(a, e, &format!("{path}[{i}]"));
            }
        }
        _ => assert_eq!(actual, expected, "{path}"),
    }
}

fn triples(flat: &[f64]) -> Vec<[f32; 3]> {
    flat.chunks(3)
        .map(|c| [c[0] as f32, c[1] as f32, c[2] as f32])
        .collect()
}

struct Scene {
    soup: Vec<f32>,
    bvh: Arc<MeshBvh>,
    normals: Vec<[f32; 3]>,
}

#[test]
fn mesh_to_braced_forest_matches_dragonfruit() {
    let native = load("df_native_islands.json");
    let fixture = load("df_islands_c.json");
    let identity = Matrix4::IDENTITY;
    let scenes: HashMap<String, Scene> = native["meshes"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, soup)| {
            let flat: Vec<f64> = serde_json::from_value(soup.clone()).unwrap();
            let normals: Vec<f64> =
                serde_json::from_value(fixture["normals"][name].clone()).unwrap();
            let scene = Scene {
                soup: flat.iter().map(|&v| v as f32).collect(),
                bvh: Arc::new(MeshBvh::new(triples(&flat), None)),
                normals: triples(&normals),
            };
            (name.clone(), scene)
        })
        .collect();
    let pool: HashMap<String, RefCell<SdfCache>> = scenes
        .iter()
        .map(|(n, s)| {
            (
                n.clone(),
                RefCell::new(SdfCache::new(s.bvh.clone(), identity, Some(0.5))),
            )
        })
        .collect();
    let brace_meshes: BraceMeshes<'_> = scenes
        .iter()
        .map(|(n, s)| {
            (
                n.clone(),
                BraceMesh {
                    bvh: &s.bvh,
                    matrix: identity,
                },
            )
        })
        .collect();
    let bands = SizingBands::factory();
    let mut next = 0u64;
    let mut new_id = move || {
        next += 1;
        format!("id-{next}")
    };

    for (i, case) in fixture["runs"].as_array().unwrap().iter().enumerate() {
        let run = &native["runs"][case["run"].as_u64().unwrap() as usize];
        let name = run["scene"].as_str().unwrap();
        let path = format!("runs[{i}] {name}");
        let scene = &scenes[name];
        let combined: CombinedIslandScanResult =
            serde_json::from_value(run["combined"].clone()).unwrap();
        let (regions, stability) = overhang_and_stability_from_soup(
            &scene.soup,
            run["angleDeg"].as_f64().unwrap() as f32,
            run["overhangPxMm"].as_f64().unwrap() as f32,
            run["hasRaft"].as_bool().unwrap(),
        );
        let soup_scan = OverhangScan { regions, stability };
        let derived = derive_islands(
            &combined,
            &soup_scan,
            &[],
            &IslandPanelSettings::defaults(run["layerHeightMm"].as_f64().unwrap()),
        );
        let ids: Vec<&str> = derived.filtered.iter().map(|i| i.id.as_str()).collect();
        let expected_ids: Vec<&str> = case["islandIds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(ids, expected_ids, "{path}: islands differ");

        let live: SupportSettings = serde_json::from_value(case["settings"].clone()).unwrap();
        let mesh = PlanMesh {
            bvh: &scene.bvh,
            matrix: identity,
            side: Side::Front,
            normals: Some(&scene.normals),
            sdf: &pool[name],
            blocked: None,
        };
        let ctx = PlanCtx::new(&live, &bands, Some(mesh), &mut new_id);
        let empty = SupportState::default();
        let plan = compute_auto_support_plan(
            &PlanInput {
                islands: &derived.filtered,
                model_id: name,
                settings_override: Some(&live.auto_support),
                base_state: &empty,
                committed: &empty,
                // DragonFruit's default raft is solid.
                has_raft: true,
                brace_meshes: &brace_meshes,
            },
            &ctx,
        );
        let expected = &case["result"];
        match plan {
            None => assert!(
                expected.is_null(),
                "{path}: port disabled, DragonFruit planned"
            ),
            Some(plan) => {
                let mut result = plan.result.clone();
                result.analytics = None;
                assert_same(
                    &serde_json::to_value(&result).unwrap(),
                    &expected["result"],
                    &format!("{path}.result"),
                );
                assert_same(
                    &serde_json::to_value(&plan.support.collections).unwrap(),
                    &expected["support"],
                    &format!("{path}.support"),
                );
                assert_same(
                    &serde_json::to_value(&plan.analytics).unwrap(),
                    &expected["analytics"],
                    &format!("{path}.analytics"),
                );
            }
        }
    }
}
