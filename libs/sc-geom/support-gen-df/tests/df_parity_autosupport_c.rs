//! Parity with DragonFruit for block 6c: `fixtures/df_autosupport_c.json`
//! records DragonFruit's own `computeAutoSupportPlan`
//! (`scripts/df-oracle/golden_autosupport_c.ts`, DragonFruit f331fc55, `uuid`
//! replaced by one counter for the whole run, auto-bracing skipped) over seven
//! meshes: the forest it leaves, its analytics with the forest report and load
//! budget, and its result. Some runs build on an earlier run's forest. The port
//! replays the runs in order with one counter and one pooled distance field
//! per mesh.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::Value;

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
use support_gen_df::volume_analysis::islands::types::DetectedIsland;

fn fixture() -> Value {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/df_autosupport_c.json"
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
fn auto_support_plans_match_dragonfruit() {
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
        let committed = if run["committedIsBase"].as_bool().unwrap() {
            base.clone()
        } else {
            SupportState::default()
        };
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
                if std::env::var("DUMP_RUN").ok().as_deref() == Some(&r.to_string()) {
                    std::fs::write(
                        std::env::temp_dir().join("sc_run_dump.json"),
                        serde_json::to_string(&serde_json::json!({
                            "support": plan.support.collections,
                            "analytics": plan.analytics,
                        }))
                        .unwrap(),
                    )
                    .unwrap();
                }
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
            }
        }
    }
}
