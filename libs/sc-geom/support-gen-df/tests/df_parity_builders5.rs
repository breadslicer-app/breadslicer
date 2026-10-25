//! Parity with DragonFruit for block 5: `fixtures/df_builders5.json` records
//! DragonFruit's own answers (`scripts/df-oracle/golden_builders5.ts`,
//! DragonFruit f331fc55, `uuid` replaced by a counter) for the Twig, Stick,
//! Stump and Kickstand builders (with and without the model mesh, over a torus
//! knot and a ceiling/floor gap scene, one of them moved), the registered
//! contact bridge builders, the stump's contact override, the kickstand layout,
//! the twig taper and the brace's span diameters. Mesh checks replay in
//! DragonFruit's call order against one pooled distance field per mesh.
//! Entities compare whole, ids included, bit for bit.

use std::collections::HashMap;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::Value;

use support_gen_df::supports::model_mesh::ModelMesh;
use support_gen_df::supports::placement_logic::pathfinding::sdf_cache::SdfCache;
use support_gen_df::supports::settings::types::SupportSettings;
use support_gen_df::supports::support_type_registry::SupportTypeId;
use support_gen_df::supports::support_types::brace::brace_span_diameters::brace_span_diameters;
use support_gen_df::supports::support_types::contact_bridge::*;
use support_gen_df::supports::support_types::kickstand::kickstand_builder::*;
use support_gen_df::supports::support_types::kickstand::kickstand_settings::*;
use support_gen_df::supports::support_types::kickstand::types::KickstandHostTarget;
use support_gen_df::supports::support_types::stick::stick_builder::*;
use support_gen_df::supports::support_types::stump::stump_auto_placement::*;
use support_gen_df::supports::support_types::stump::stump_builder::*;
use support_gen_df::supports::support_types::twig::twig_builder::*;
use support_gen_df::supports::support_types::twig::twig_taper::*;
use support_gen_df::supports::types::{BraceProfile, Twig, Vec3};
use support_gen_df::three::Matrix4;
use support_gen_df::three_mesh_bvh::{BvhMesh, MeshBvh};

fn fixture() -> Value {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/df_builders5.json"
    ))
    .unwrap();
    serde_json::from_str(&text).unwrap()
}

fn read<T: DeserializeOwned>(value: &Value) -> T {
    serde_json::from_value(value.clone()).unwrap_or_else(|e| panic!("{e}: {value}"))
}

/// An optional field: absent and `null` are both `None`.
fn field<T: DeserializeOwned>(value: &Value, key: &str) -> Option<T> {
    value.get(key).filter(|v| !v.is_null()).map(read)
}

fn list<'a>(fixture: &'a Value, name: &str) -> &'a [Value] {
    let cases = fixture[name]
        .as_array()
        .unwrap_or_else(|| panic!("no {name}"));
    assert!(!cases.is_empty(), "no {name}");
    cases
}

/// Compares two JSON graphs exactly (numbers as f64 bits). A field
/// DragonFruit left `undefined` is absent; the port must omit it too.
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

fn counter_ids() -> impl FnMut() -> String {
    let mut next = 0;
    move || {
        next += 1;
        format!("id-{next}")
    }
}

struct Scene {
    bvh: Arc<MeshBvh>,
    matrix: Matrix4,
}

fn scenes(fixture: &Value) -> HashMap<String, Scene> {
    fixture["meshes"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, m)| {
            let flat: Vec<f64> = read(&m["positions"]);
            let positions = flat
                .chunks(3)
                .map(|c| [c[0] as f32, c[1] as f32, c[2] as f32])
                .collect();
            let index: Option<Vec<u32>> = field(m, "index");
            let e: Vec<f64> = read(&m["matrix"]);
            let scene = Scene {
                bvh: Arc::new(MeshBvh::new(positions, index)),
                matrix: Matrix4::from_column_major(e.try_into().unwrap()),
            };
            (name.clone(), scene)
        })
        .collect()
}

fn vec(value: &Value, key: &str) -> Vec3 {
    read(&value[key])
}

#[test]
fn builders_match_dragonfruit() {
    let fixture = fixture();
    let scenes = scenes(&fixture);
    // DragonFruit's pool: one field per mesh, built on first use.
    let mut pool: HashMap<String, SdfCache> = HashMap::new();

    for (i, c) in list(&fixture, "ops").iter().enumerate() {
        let name = c["scene"].as_str().unwrap();
        let scene = &scenes[name];
        let with_mesh = c["withMesh"].as_bool().unwrap();
        if with_mesh {
            pool.entry(name.to_owned())
                .or_insert_with(|| SdfCache::new(scene.bvh.clone(), scene.matrix, Some(0.5)));
        }
        let field_ref = with_mesh.then(|| &pool[name]);
        let mesh = field_ref.map(|sdf| ModelMesh {
            field: sdf,
            rays: BvhMesh {
                bvh: &scene.bvh,
                matrix_world: &scene.matrix,
            },
        });
        let settings: SupportSettings = read(&c["settings"]);
        let input = &c["input"];
        let ids = &mut counter_ids();
        let path = format!("ops[{i}].{}", c["op"].as_str().unwrap());
        let expected = &c["result"];

        match c["op"].as_str().unwrap() {
            "twig" => {
                let result = build_twig(
                    &TwigBuildInput {
                        model_id: "m",
                        a_pos: vec(input, "aPos"),
                        a_normal: vec(input, "aNormal"),
                        b_pos: vec(input, "bPos"),
                        b_normal: vec(input, "bNormal"),
                        mesh,
                        tip_contact_diameter_mm: field(input, "tipContactDiameterMm"),
                    },
                    &settings,
                    ids,
                );
                check(&path, result, expected);
            }
            "stick" => {
                let result = build_stick(
                    &StickBuildInput {
                        model_id: "m",
                        shaft_diameter_mm: field(input, "shaftDiameterMm"),
                        tip_contact_diameter_mm: field(input, "tipContactDiameterMm"),
                        a_pos: vec(input, "aPos"),
                        a_normal: vec(input, "aNormal"),
                        b_pos: vec(input, "bPos"),
                        b_normal: vec(input, "bNormal"),
                        mesh,
                    },
                    &settings,
                    ids,
                );
                check(&path, result, expected);
            }
            "bridge" => {
                let type_id = SupportTypeId::parse(c["typeId"].as_str().unwrap()).unwrap();
                let result = build_contact_bridge(
                    type_id,
                    &ContactBridgeRequest {
                        model_id: "m",
                        a_pos: vec(input, "aPos"),
                        a_normal: vec(input, "aNormal"),
                        b_pos: vec(input, "bPos"),
                        b_normal: vec(input, "bNormal"),
                        shaft_diameter_mm: field(input, "shaftDiameterMm"),
                        tip_contact_diameter_mm: field(input, "tipContactDiameterMm"),
                        mesh,
                        manual: field(input, "manual"),
                    },
                    &settings,
                    ids,
                );
                check(&format!("{path}.{type_id}"), result, expected);
            }
            "stump" => {
                let result = build_stump_data(
                    &StumpBuildInput {
                        tip_pos: vec(input, "tipPos"),
                        tip_normal: vec(input, "tipNormal"),
                        model_id: "m",
                        mesh: mesh.map(|m| m.field),
                    },
                    &settings,
                    ids,
                );
                check(&path, result, expected);
            }
            "stumpOverride" => {
                let result = build_stump_contact_override(
                    &ContactOverrideRequest {
                        tip_pos: vec(input, "tipPos"),
                        tip_normal: vec(input, "tipNormal"),
                        model_id: "m",
                        mesh: mesh.map(|m| m.field),
                    },
                    &settings,
                    ids,
                );
                check(&path, result, expected);
            }
            _ => {
                let host: KickstandHostTarget = read(&input["host"]);
                let result = build_kickstand_data(
                    &KickstandBuildInput {
                        model_id: "m",
                        root_pos: vec(input, "rootPos"),
                        host: &host,
                        layout_overrides: field(input, "layoutOverrides"),
                        authored_column_top: input.get("authoredColumn").map(|a| vec(a, "topPos")),
                        collision_mesh: mesh.map(|m| m.field),
                        max_shaft_angle_deg: field(input, "maxShaftAngleDeg"),
                    },
                    &settings,
                    ids,
                );
                match (result, c.get("error").and_then(Value::as_str)) {
                    (Err(message), Some(expected)) => assert_eq!(message, expected, "{path}"),
                    (Ok(build), None) => {
                        let preview = to_kickstand_preview_data(&build);
                        check(
                            &path,
                            serde_json::json!({ "build": build, "preview": preview }),
                            expected,
                        );
                    }
                    (result, error) => panic!("{path}: port {result:?} vs DragonFruit {error:?}"),
                }
            }
        }
    }
}

#[test]
fn kickstand_layout_taper_and_brace_spans_match_dragonfruit() {
    let fixture = fixture();
    for (i, c) in list(&fixture, "layouts").iter().enumerate() {
        let overrides: Option<KickstandLayoutOverrides> = field(c, "overrides");
        check(
            &format!("layouts[{i}]"),
            resolve_kickstand_layout(overrides.as_ref()),
            &c["result"],
        );
    }

    for (i, c) in list(&fixture, "tapers").iter().enumerate() {
        let twig: Twig = read(&c["twig"]);
        let layout = build_twig_taper_layout(&twig);
        let actual_layout = serde_json::json!({
            "totalLength": layout.total_length,
            "segments": layout.segments.iter().map(|r| serde_json::json!({
                "segmentId": r.segment_id, "sStart": r.s_start, "sEnd": r.s_end,
            })).collect::<Vec<_>>(),
        });
        check(&format!("tapers[{i}].layout"), actual_layout, &c["layout"]);
        let diameters: serde_json::Map<String, Value> = resolve_twig_segment_diameters(&twig)
            .into_iter()
            .map(|(id, (start, end))| {
                (
                    id,
                    serde_json::json!({ "diameterStart": start, "diameterEnd": end }),
                )
            })
            .collect();
        check(
            &format!("tapers[{i}].diameters"),
            diameters,
            &c["diameters"],
        );
        for (k, q) in c["queries"].as_array().unwrap().iter().enumerate() {
            let result = resolve_twig_diameter_at_segment_t(
                &twig,
                q["segmentId"].as_str().unwrap(),
                q["t"].as_f64().unwrap(),
            );
            check(&format!("tapers[{i}].queries[{k}]"), result, &q["result"]);
        }
    }

    for (i, c) in list(&fixture, "braceSpans").iter().enumerate() {
        let profile: Option<BraceProfile> = field(c, "profile");
        let knot = |key: &str| -> Option<f64> { c.get(key).and_then(|k| field(k, "diameter")) };
        let result = brace_span_diameters(profile, knot("start"), knot("end"));
        check(&format!("braceSpans[{i}]"), result, &c["result"]);
    }
}
