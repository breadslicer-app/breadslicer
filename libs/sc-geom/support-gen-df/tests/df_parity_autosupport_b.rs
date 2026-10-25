//! Parity with DragonFruit for block 6b: `fixtures/df_autosupport_b.json`
//! records DragonFruit's own answers (`scripts/df-oracle/golden_autosupport_b.ts`,
//! DragonFruit f331fc55, `uuid` replaced by one counter for the whole run) for
//! placement sequences — trunk candidate, grid decision, draft commit — over
//! four meshes, the forest diameter resize of each final snapshot, the member
//! diameters, cavity bridges and smoothed contact normals. The port replays the
//! same sequence with one counter and one pooled distance field per mesh.

use std::collections::HashMap;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use support_gen_df::supports::auto_support::support_draft::draft_commit_support;
use support_gen_df::supports::model_mesh::ModelMesh;
use support_gen_df::supports::placement_logic::grid::grid_placement::*;
use support_gen_df::supports::placement_logic::pathfinding::sdf_cache::SdfCache;
use support_gen_df::supports::placement_logic::placement_utils::calculate_smoothed_normal;
use support_gen_df::supports::placement_logic_v3::smart_placement_v3::RouterStats;
use support_gen_df::supports::settings::types::SupportSettings;
use support_gen_df::supports::support_type_registry::SupportEntityValue;
use support_gen_df::supports::support_types::trunk::cavity_bridge::*;
use support_gen_df::supports::support_types::trunk::trunk_builder::*;
use support_gen_df::supports::support_types::trunk::trunk_replacement::max_connected_diameter::*;
use support_gen_df::supports::types::{SupportState, Vec3};
use support_gen_df::three::{Matrix4, Ray};
use support_gen_df::three_mesh_bvh::{BvhMesh, MeshBvh, Side};

fn fixture() -> Value {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/df_autosupport_b.json"
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
            };
            (name.clone(), scene)
        })
        .collect()
}

#[test]
fn grid_decisions_resize_bridges_and_normals_match_dragonfruit() {
    let fixture = fixture();
    let scenes = scenes(&fixture);
    let mut pool: HashMap<String, SdfCache> = HashMap::new();
    let mut stats = RouterStats::default();
    // One id counter for the whole run, as the oracle's uuid stub.
    let mut next = 0u64;
    let mut new_id = move || {
        next += 1;
        format!("id-{next}")
    };

    for (w, world) in fixture["worlds"].as_array().unwrap().iter().enumerate() {
        let name = world["scene"].as_str().unwrap();
        let scene = &scenes[name];
        let mut settings: SupportSettings = read(&world["settings"]);
        let mut snapshot = SupportState::default();
        for (i, step) in world["steps"].as_array().unwrap().iter().enumerate() {
            let path = format!("worlds[{w}].steps[{i}]");
            // The live settings at this step (half the worlds respace mid-run).
            settings = read(&step["settings"]);
            let tip_pos: Vec3 = read(&step["tip"]["pos"]);
            let tip_normal: Vec3 = read(&step["tip"]["normal"]);
            let with_mesh = step["withMesh"].as_bool().unwrap();
            let input = TrunkBuildInput {
                tip_pos,
                tip_normal,
                model_id: "m",
                ..TrunkBuildInput::default()
            };
            let sdf = pool
                .entry(name.to_owned())
                .or_insert_with(|| SdfCache::new(scene.bvh.clone(), scene.matrix, Some(0.5)));
            let candidate = if with_mesh {
                build_trunk_data_routed(
                    &input,
                    &mut TrunkRouter {
                        sdf,
                        matrix_world: scene.matrix,
                        stats: &mut stats,
                    },
                    &settings,
                    &mut new_id,
                )
            } else {
                build_trunk_data(&input, &settings, &mut new_id).unwrap()
            };
            let decision = decide_grid_placement(
                &DecideGridPlacementArgs {
                    settings: &settings,
                    snapshot: &snapshot,
                    candidate: &candidate,
                    tip_pos,
                    tip_normal,
                    model_id: "m",
                    mesh: with_mesh.then_some(&*sdf as _),
                    is_preview: None,
                },
                &settings,
                &mut new_id,
            );
            check(&format!("{path}.decision"), &decision, &step["decision"]);
            if let GridPlacementDecision::Place { placed, .. } = decision {
                draft_commit_support(
                    &mut snapshot,
                    placed.type_id,
                    placed.entity,
                    &placed.supplied,
                );
            }
        }
        check(
            &format!("worlds[{w}].snapshot"),
            &snapshot.collections,
            &world["snapshot"],
        );
        let resized = compute_forest_diameter_profile(&snapshot, &settings, &mut new_id);
        check(
            &format!("worlds[{w}].resized"),
            &resized.collections,
            &world["resized"],
        );
        let c = &resized.collections;
        let mut diameters = serde_json::Map::new();
        let entities = c
            .trunks
            .values()
            .map(|e| SupportEntityValue::Trunk(e.clone()))
            .chain(
                c.branches
                    .values()
                    .map(|e| SupportEntityValue::Branch(e.clone())),
            )
            .chain(
                c.leaves
                    .values()
                    .map(|e| SupportEntityValue::Leaf(e.clone())),
            )
            .chain(
                c.twigs
                    .values()
                    .map(|e| SupportEntityValue::Twig(e.clone())),
            )
            .chain(
                c.sticks
                    .values()
                    .map(|e| SupportEntityValue::Stick(e.clone())),
            )
            .chain(
                c.braces
                    .values()
                    .map(|e| SupportEntityValue::Brace(e.clone())),
            )
            .chain(
                c.stumps
                    .values()
                    .map(|e| SupportEntityValue::Stump(e.clone())),
            )
            .chain(
                c.kickstands
                    .values()
                    .map(|e| SupportEntityValue::Kickstand(e.clone())),
            );
        for entity in entities {
            let id = serde_json::to_value(&entity).unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned();
            diameters.insert(id, json!(member_diameter_of(&entity)));
        }
        check(
            &format!("worlds[{w}].diameters"),
            &diameters,
            &world["diameters"],
        );
    }

    for (i, probe) in fixture["probes"].as_array().unwrap().iter().enumerate() {
        let path = format!("probes[{i}]");
        let name = probe["scene"].as_str().unwrap();
        let scene = &scenes[name];
        let settings: SupportSettings = read(&probe["settings"]);
        let rays = BvhMesh {
            bvh: &scene.bvh,
            matrix_world: &scene.matrix,
        };
        match probe["op"].as_str().unwrap() {
            "cavity" => {
                let sdf = pool
                    .entry(name.to_owned())
                    .or_insert_with(|| SdfCache::new(scene.bvh.clone(), scene.matrix, Some(0.5)));
                let sizing =
                    probe
                        .get("sizing")
                        .filter(|s| !s.is_null())
                        .map(|s| CavityBridgeSizing {
                            tip_contact_diameter_mm: s["tipContactDiameterMm"].as_f64().unwrap(),
                            shaft_diameter_mm: s["shaftDiameterMm"].as_f64().unwrap(),
                        });
                let result = build_cavity_bridge(
                    read(&probe["tip"]["pos"]),
                    read(&probe["tip"]["normal"]),
                    "m",
                    ModelMesh { field: &*sdf, rays },
                    scene.side,
                    sizing,
                    &settings,
                    &mut new_id,
                );
                check(&path, result, &probe["result"]);
            }
            _ => {
                let ray = Ray {
                    origin: read(&probe["origin"]),
                    direction: read(&probe["dir"]),
                };
                let hit = rays
                    .intersect_object(&ray, scene.side, 0.0, f64::INFINITY, false)
                    .into_iter()
                    .next();
                let result = hit.map(|h| {
                    json!({
                        "faceIndex": h.face_index,
                        "point": h.point,
                        "normal": calculate_smoothed_normal(&h, scene.bvh.positions(), Some(&scene.normals), &scene.matrix),
                    })
                });
                check(&path, result, &probe["result"]);
            }
        }
    }
}
