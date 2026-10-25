//! Parity with DragonFruit for block 7: `fixtures/df_autobrace.json` records
//! DragonFruit's own auto-bracing (`scripts/df-oracle/golden_autobrace.ts`,
//! DragonFruit f331fc55, `uuid` replaced by one counter for the whole run):
//! `computeAutoSupportPlan` over seven meshes, half the runs with bracing
//! folded into the plan, then `buildAutoBracedSnapshot` on each planned forest
//! twice in a row with random bracing settings. Six models are registered for
//! brace clearance. The port replays the runs in order with one counter and one
//! pooled distance field per mesh, and checks the collation against V8's.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::Value;

use support_gen_df::supports::auto_bracing::auto_brace::build_auto_braced_snapshot;
use support_gen_df::supports::auto_bracing::mesh_clearance::{BraceMesh, BraceMeshes};
use support_gen_df::supports::auto_bracing::settings::AutoBracingSettings;
use support_gen_df::supports::auto_bracing::voronoi_partitioning::{
    Vec2, VoronoiPartitionSettings, VoronoiSupportNode, partition_supports_with_voronoi,
};
use support_gen_df::supports::auto_support::auto_place::helpers::{PlanCtx, PlanMesh};
use support_gen_df::supports::auto_support::auto_place::plan::{
    PlanInput, compute_auto_support_plan,
};
use support_gen_df::supports::auto_support::parameter_sizing::SizingBands;
use support_gen_df::supports::auto_support::settings::{
    AutoSupportSettings, create_default_auto_support_settings,
};
use support_gen_df::supports::placement_logic::pathfinding::sdf_cache::SdfCache;
use support_gen_df::supports::settings::types::SupportSettings;
use support_gen_df::supports::types::SupportState;
use support_gen_df::three::Matrix4;
use support_gen_df::three_mesh_bvh::{MeshBvh, Side};
use support_gen_df::utils::collation::js_locale_compare;
use support_gen_df::volume_analysis::islands::types::DetectedIsland;

fn fixture() -> Value {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/df_autobrace.json"
    ))
    .unwrap();
    serde_json::from_str(&text).unwrap()
}

fn read<T: DeserializeOwned>(value: &Value) -> T {
    serde_json::from_value(value.clone()).unwrap_or_else(|e| panic!("{e}: {value}"))
}

fn assert_same(actual: &Value, expected: &Value, path: &str) {
    match (actual, expected) {
        (Value::Number(a), Value::Number(e)) => {
            let (a, e) = (a.as_f64().unwrap(), e.as_f64().unwrap());
            assert!(a == e, "{path}: port {a:?} vs DragonFruit {e:?}");
        }
        (Value::Object(a), Value::Object(e)) => {
            let mut a_keys: Vec<&String> = a.keys().collect();
            let mut e_keys: Vec<&String> = e.keys().collect();
            a_keys.sort();
            e_keys.sort();
            assert_eq!(a_keys, e_keys, "{path}: fields differ");
            for (key, e_value) in e {
                assert_same(&a[key], e_value, &format!("{path}.{key}"));
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

fn check(path: &str, actual: impl serde::Serialize, expected: &Value) {
    assert_same(&serde_json::to_value(actual).unwrap(), expected, path);
}

struct Scene {
    bvh: Arc<MeshBvh>,
    normals: Vec<[f32; 3]>,
    matrix: Matrix4,
    side: Side,
    blocked: Option<HashSet<usize>>,
    brace_mesh: bool,
}

fn triples(value: &Value) -> Vec<[f32; 3]> {
    let flat: Vec<f64> = read(value);
    flat.chunks(3)
        .map(|c| [c[0] as f32, c[1] as f32, c[2] as f32])
        .collect()
}

fn scenes(fixture: &Value) -> HashMap<String, Scene> {
    fixture["meshes"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, m)| {
            let index: Option<Vec<u32>> = read(&m["index"]);
            let e: Vec<f64> = read(&m["matrix"]);
            let side = match m["side"].as_str().unwrap() {
                "front" => Side::Front,
                "back" => Side::Back,
                _ => Side::Double,
            };
            let scene = Scene {
                bvh: Arc::new(MeshBvh::new(triples(&m["positions"]), index)),
                normals: triples(&m["normals"]),
                matrix: Matrix4::from_column_major(e.try_into().unwrap()),
                side,
                blocked: read(&m["blocked"]),
                brace_mesh: m["braceMesh"].as_bool().unwrap(),
            };
            (name.clone(), scene)
        })
        .collect()
}

/// DragonFruit's partial `settingsOverride`, over the defaults.
fn override_settings(partial: &Value) -> AutoSupportSettings {
    let mut full = serde_json::to_value(create_default_auto_support_settings()).unwrap();
    for (k, v) in partial.as_object().unwrap() {
        full[k] = v.clone();
    }
    read(&full)
}

#[test]
fn voronoi_partitions_match_dragonfruit() {
    let fixture = fixture();
    for (i, case) in fixture["partitions"].as_array().unwrap().iter().enumerate() {
        let nodes: Vec<VoronoiSupportNode> = case["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| VoronoiSupportNode {
                support_id: n["supportId"].as_str().unwrap().to_owned(),
                model_id: n["modelId"].as_str().unwrap().to_owned(),
                point: Vec2 {
                    x: n["point"]["x"].as_f64().unwrap(),
                    y: n["point"]["y"].as_f64().unwrap(),
                },
            })
            .collect();
        let s = &case["settings"];
        let settings = VoronoiPartitionSettings {
            seed_spacing_mm: s["seedSpacingMm"].as_f64().unwrap(),
            seed_jitter_mm: s["seedJitterMm"].as_f64().unwrap(),
            max_neighbor_distance_mm: s["maxNeighborDistanceMm"].as_f64().unwrap_or(f64::NAN),
        };
        let groups = partition_supports_with_voronoi(&nodes, &settings);
        let expected: Vec<Vec<String>> = read(&case["groups"]);
        assert_eq!(groups, expected, "partitions[{i}]");
    }
}

#[test]
fn locale_compare_matches_v8() {
    let fixture = fixture();
    let mut words: Vec<String> = read(&fixture["collation"]["words"]);
    words.sort_by(|a, b| js_locale_compare(a, b));
    let expected: Vec<String> = read(&fixture["collation"]["sorted"]);
    assert_eq!(words, expected);
}

#[test]
fn auto_bracing_matches_dragonfruit() {
    let fixture = fixture();
    let scenes = scenes(&fixture);
    let bands = SizingBands::factory();
    let pool: HashMap<String, RefCell<SdfCache>> = scenes
        .iter()
        .map(|(name, s)| {
            (
                name.clone(),
                RefCell::new(SdfCache::new(s.bvh.clone(), s.matrix, Some(0.5))),
            )
        })
        .collect();
    let brace_meshes: BraceMeshes<'_> = scenes
        .iter()
        .filter(|(_, s)| s.brace_mesh)
        .map(|(name, s)| {
            (
                name.clone(),
                BraceMesh {
                    bvh: &s.bvh,
                    matrix: s.matrix,
                },
            )
        })
        .collect();
    // One id counter for the whole run, as the oracle's uuid stub.
    let mut next = 0u64;
    let mut new_id = move || {
        next += 1;
        format!("id-{next}")
    };

    for (r, run) in fixture["runs"].as_array().unwrap().iter().enumerate() {
        let path = format!("runs[{r}]");
        let name = run["scene"].as_str().unwrap();
        let scene = &scenes[name];
        let live: SupportSettings = read(&run["settings"]);
        let auto = override_settings(&run["auto"]);
        let islands: Vec<DetectedIsland> = read(&run["islands"]);
        // A chained run builds on an earlier forest as DragonFruit's store
        // holds it after the commit.
        let base = match &run["base"] {
            Value::Null => SupportState::default(),
            forest => SupportState {
                collections: read(forest),
                ..SupportState::default()
            },
        };
        // Every run starts from an empty committed store.
        let committed = SupportState::default();
        let mesh = PlanMesh {
            bvh: &scene.bvh,
            matrix: scene.matrix,
            side: scene.side,
            normals: Some(&scene.normals),
            sdf: &pool[name],
            blocked: scene.blocked.as_ref(),
        };
        let ctx = PlanCtx::new(&live, &bands, Some(mesh), &mut new_id);
        let plan = compute_auto_support_plan(
            &PlanInput {
                islands: &islands,
                model_id: name,
                settings_override: Some(&auto),
                base_state: &base,
                committed: &committed,
                has_raft: run["hasRaft"].as_bool().unwrap(),
                brace_meshes: &brace_meshes,
            },
            &ctx,
        );
        let expected = &run["result"];
        match plan {
            None => {
                assert!(
                    expected.is_null(),
                    "{path}: port disabled, DragonFruit planned"
                );
            }
            Some(plan) => {
                assert!(
                    !expected.is_null(),
                    "{path}: port planned, DragonFruit disabled"
                );
                // The oracle records the result without its copy of the analytics.
                let mut result = plan.result.clone();
                result.analytics = None;
                check(&format!("{path}.result"), &result, &expected["result"]);
                check(
                    &format!("{path}.support"),
                    &plan.support.collections,
                    &expected["support"],
                );
                check(
                    &format!("{path}.analytics"),
                    &plan.analytics,
                    &expected["analytics"],
                );
                drop(ctx);
                // Brace the committed forest twice in a row. Each pass reads
                // the store as DragonFruit's commit normalised it.
                for (p, pass) in run["braces"].as_array().unwrap().iter().enumerate() {
                    let state = SupportState {
                        collections: read(&pass["input"]),
                        ..SupportState::default()
                    };
                    let mut pass_live = live.clone();
                    pass_live.grid = read(&pass["grid"]);
                    let settings: AutoBracingSettings = read(&pass["settings"]);
                    let (next, counts) = build_auto_braced_snapshot(
                        &state,
                        &settings,
                        &pass_live,
                        &brace_meshes,
                        // `runAutoBracing` braces a clone of the store itself.
                        &state,
                        &mut new_id,
                    );
                    check(
                        &format!("{path}.braces[{p}].counts"),
                        counts,
                        &pass["counts"],
                    );
                    check(
                        &format!("{path}.braces[{p}].snapshot"),
                        &next.collections,
                        &pass["snapshot"],
                    );
                }
            }
        }
    }
}
