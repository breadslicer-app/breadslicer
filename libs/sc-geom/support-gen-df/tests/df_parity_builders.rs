//! Parity with DragonFruit for block 3: `fixtures/df_builders.json` records
//! DragonFruit's own answers (`scripts/df-oracle/golden_builders.ts`,
//! DragonFruit f331fc55, `uuid` replaced by a counter) for the cone axis
//! policy, three-mesh-bvh raycasts under a world matrix, CollisionUtils'
//! whisker checks, Settings normalization and the hex codec, gridMath,
//! StandardPlacement, and the Trunk, Branch and Leaf builders with and without
//! the model's collision field. Entities compare whole, ids included, bit for bit.

use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::Value;

use support_gen_df::supports::placement_logic::collision_avoidance::CollisionField;
use support_gen_df::supports::placement_logic::collision_utils::*;
use support_gen_df::supports::placement_logic::cone_axis_policy::*;
use support_gen_df::supports::placement_logic::grid::grid_math::*;
use support_gen_df::supports::placement_logic::pathfinding::sdf_cache::SdfCache;
use support_gen_df::supports::placement_logic::standard_placement::{
    TrunkPlacementInput, calculate_standard_placement,
};
use support_gen_df::supports::settings::state::set_settings;
use support_gen_df::supports::settings::support_settings_codec::*;
use support_gen_df::supports::settings::types::SupportSettings;
use support_gen_df::supports::support_primitives::contact_cone::types::SupportTipProfile;
use support_gen_df::supports::support_types::branch::branch_builder::*;
use support_gen_df::supports::support_types::leaf::leaf_builder::*;
use support_gen_df::supports::support_types::trunk::trunk_builder::*;
use support_gen_df::supports::types::{Knot, Vec3};
use support_gen_df::three::{Matrix4, Ray};
use support_gen_df::three_mesh_bvh::{BvhMesh, MeshBvh, Side};

fn fixture() -> Value {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/df_builders.json"
    ))
    .unwrap();
    serde_json::from_str(&text).unwrap()
}

fn read<T: DeserializeOwned>(value: &Value) -> T {
    serde_json::from_value(value.clone()).unwrap_or_else(|e| panic!("{e}: {value}"))
}

fn opt<T: DeserializeOwned>(value: &Value) -> Option<T> {
    if value.is_null() {
        None
    } else {
        Some(read(value))
    }
}

fn num(value: &Value) -> f64 {
    value
        .as_f64()
        .unwrap_or_else(|| panic!("not a number: {value}"))
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

fn positions(value: &Value) -> Vec<[f32; 3]> {
    let flat: Vec<f64> = read(value);
    flat.chunks(3)
        .map(|c| [c[0] as f32, c[1] as f32, c[2] as f32])
        .collect()
}

fn counter_ids() -> impl FnMut() -> String {
    let mut next = 0;
    move || {
        next += 1;
        format!("id-{next}")
    }
}

#[test]
fn cone_axis_policy_matches_dragonfruit() {
    let fixture = fixture();
    for (i, c) in list(&fixture, "coneAxis").iter().enumerate() {
        let mode: ConeAngleMode = read(&c["mode"]);
        let offset: Option<f64> = opt(&c["offset"]);
        let n: Vec3 = read(&c["n"]);
        let policy = resolve_cone_axis_policy(ConeAxisPolicyInput {
            surface_normal: n,
            cone_angle_mode: mode,
            adaptive_cone_angle_offset_deg: offset,
        });
        check(
            &format!("coneAxis[{i}].policy"),
            serde_json::json!({
                "surfaceAngleFromUpDeg": policy.surface_angle_from_up_deg,
                "minAllowedSurfaceAngleFromUpDeg": policy.min_allowed_surface_angle_from_up_deg,
                "coneAxis": policy.cone_axis,
            }),
            &c["policy"],
        );
        check(
            &format!("coneAxis[{i}].clamp"),
            clamp_cone_axis_deviation_from_surface_normal(
                n,
                read(&c["desired"]),
                num(&c["maxDev"]),
            ),
            &c["clamp"],
        );
        check(
            &format!("coneAxis[{i}].lean"),
            cone_axis_lean_from_vertical_deg(n, mode, offset),
            &c["lean"],
        );
        check(
            &format!("coneAxis[{i}].sideWall"),
            is_side_wall_contact(n, mode, offset),
            &c["sideWall"],
        );
        check(
            &format!("coneAxis[{i}].highest"),
            cone_keeps_contact_highest(
                read(&c["desired"]),
                read(&c["socket"]),
                read(&c["tip"]),
                num(&c["body"]),
            ),
            &c["highest"],
        );
    }
}

#[test]
fn raycasts_and_collision_utils_match_dragonfruit() {
    let fixture = fixture();
    let moved = MeshBvh::new(positions(&fixture["movedPositions"]), None);
    let moved_matrix = Matrix4::from_column_major(
        read::<Vec<f64>>(&fixture["movedMatrix"])
            .try_into()
            .unwrap(),
    );
    let moved_mesh = BvhMesh {
        bvh: &moved,
        matrix_world: &moved_matrix,
    };
    for (i, c) in list(&fixture, "raycasts").iter().enumerate() {
        let side = match c["side"].as_str().unwrap() {
            "front" => Side::Front,
            "back" => Side::Back,
            _ => Side::Double,
        };
        let far = opt::<f64>(&c["far"]).unwrap_or(f64::INFINITY);
        let hits = moved_mesh.intersect_object(
            &Ray::new(read(&c["origin"]), read(&c["dir"])),
            side,
            num(&c["near"]),
            far,
            c["firstHitOnly"].as_bool().unwrap(),
        );
        let hits: Vec<Value> = hits
            .iter()
            .map(|h| {
                serde_json::json!({
                    "distance": h.distance, "point": h.point, "faceIndex": h.face_index,
                    "face": { "a": h.face.a, "b": h.face.b, "c": h.face.c, "normal": h.face.normal },
                })
            })
            .collect();
        check(&format!("raycasts[{i}]"), hits, &c["hits"]);
    }

    let knot = MeshBvh::new(positions(&fixture["knotPositions"]), None);
    let knot_mesh = BvhMesh {
        bvh: &knot,
        matrix_world: &Matrix4::IDENTITY,
    };
    let as_json = |r: CollisionResult| serde_json::json!({ "hit": r.hit, "point": r.point, "normal": r.normal, "distance": r.distance });
    for (i, c) in list(&fixture, "collision").iter().enumerate() {
        let (a, b, r): (Vec3, Vec3, f64) = (read(&c["a"]), read(&c["b"]), num(&c["r"]));
        let result = match c["kind"].as_str().unwrap() {
            "shaft" => check_shaft_collision(a, b, r, knot_mesh),
            "shortBridge" => check_short_bridge_collision(a, b, r, knot_mesh),
            _ => check_bezier_collision(
                a,
                read(&c["c1"]),
                read(&c["c2"]),
                b,
                r,
                knot_mesh,
                num(&c["res"]),
            ),
        };
        check(
            &format!("collision[{i}].{}", c["kind"]),
            as_json(result),
            &c["result"],
        );
    }
}

#[test]
fn settings_codec_and_grid_math_match_dragonfruit() {
    let fixture = fixture();
    for (i, c) in list(&fixture, "settings").iter().enumerate() {
        let raw: SupportSettings = read(&c["raw"]);
        let normalized = set_settings(&raw);
        check(
            &format!("settings[{i}].normalized"),
            &normalized,
            &c["normalized"],
        );
        let hex = encode_support_settings_hex(&normalized);
        assert_eq!(hex, c["hex"].as_str().unwrap(), "settings[{i}].hex");
        assert_eq!(
            encode_support_settings_hex(&raw),
            c["rawHex"].as_str().unwrap(),
            "settings[{i}].rawHex"
        );
        check(
            &format!("settings[{i}].decoded"),
            decode_support_settings_hex(&hex, Some(&normalized)),
            &c["decoded"],
        );
    }
    for (i, c) in list(&fixture, "grid").iter().enumerate() {
        let (x, y, s) = (num(&c["x"]), num(&c["y"]), num(&c["s"]));
        check(
            &format!("grid[{i}].idx"),
            snap_to_grid_index(x, s),
            &c["idx"],
        );
        let key = grid_node_key_from_xy(x, y, s);
        assert_eq!(key, c["key"].as_str().unwrap(), "grid[{i}].key");
        let (gx, gy) = grid_snapped_xy_from_key(&key, s);
        check(
            &format!("grid[{i}].xy"),
            serde_json::json!({ "x": gx, "y": gy }),
            &c["xy"],
        );
    }
}

fn trunk_input<'a>(c: &'a Value, mesh: Option<&'a dyn CollisionField>) -> TrunkBuildInput<'a> {
    let input = &c["input"];
    let o = &input["overrides"];
    let overrides = o.as_object().map(|_| TrunkOverrides {
        roots_diameter_mm: opt(&o["rootsDiameterMm"]),
        roots_disk_height_mm: opt(&o["rootsDiskHeightMm"]),
        roots_cone_height_mm: opt(&o["rootsConeHeightMm"]),
        shaft_diameter_mm: opt(&o["shaftDiameterMm"]),
        tip_contact_diameter_mm: opt(&o["tipContactDiameterMm"]),
        tip_body_diameter_mm: opt(&o["tipBodyDiameterMm"]),
        tip_length_mm: opt(&o["tipLengthMm"]),
        tip_disk_length_override_mm: opt(&o["tipDiskLengthOverrideMm"]),
        tip_penetration_mm: None,
    });
    TrunkBuildInput {
        tip_pos: read(&input["tipPos"]),
        tip_normal: read(&input["tipNormal"]),
        model_id: input["modelId"].as_str().unwrap(),
        mesh,
        is_preview: None,
        overrides,
        is_small_island: opt(&input["isSmallIsland"]),
        ignore_grid: None,
    }
}

#[test]
fn placement_and_builders_match_dragonfruit() {
    let fixture = fixture();
    let knot = Arc::new(MeshBvh::new(positions(&fixture["knotPositions"]), None));
    // DragonFruit's pooled field for the knot, shared by every build in call order.
    let field = SdfCache::new(knot, Matrix4::IDENTITY, Some(0.5));

    for (i, c) in list(&fixture, "placements").iter().enumerate() {
        let settings: SupportSettings = read(&c["settings"]);
        let input = &c["input"];
        let placement = calculate_standard_placement(
            &TrunkPlacementInput {
                tip_pos: read(&input["tipPos"]),
                tip_normal: read(&input["tipNormal"]),
                tip_profile: read::<SupportTipProfile>(&input["tipProfile"]),
                roots_top_z: num(&input["rootsTopZ"]),
                is_small_island: opt(&input["isSmallIsland"]),
            },
            &settings,
        );
        let mut expected = c["result"].clone();
        // `snappedNodeKey: null` is the port's absent key (both mean "no node").
        expected.as_object_mut().unwrap().remove("snappedNodeKey");
        check(&format!("placements[{i}]"), placement, &expected);
    }

    for (i, c) in list(&fixture, "trunks").iter().enumerate() {
        let settings: SupportSettings = read(&c["settings"]);
        let result = match c["op"].as_str().unwrap() {
            "buildTrunkData" => {
                build_trunk_data(&trunk_input(c, None), &settings, &mut counter_ids()).unwrap()
            }
            _ => {
                let mesh = c["withMesh"]
                    .as_bool()
                    .unwrap()
                    .then_some(&field as &dyn CollisionField);
                build_trunk_data_from_placement(
                    &trunk_input(c, mesh),
                    &read(&c["placement"]),
                    &settings,
                    &mut counter_ids(),
                )
            }
        };
        check(&format!("trunks[{i}].{}", c["op"]), result, &c["result"]);
    }

    for (i, c) in list(&fixture, "branches").iter().enumerate() {
        let live: SupportSettings = read(&c["live"]);
        let input = &c["input"];
        let own: Option<SupportSettings> = opt(&input["settings"]);
        let parent_knot: Knot = read(&input["parentKnot"]);
        let mesh = c["withMesh"]
            .as_bool()
            .unwrap()
            .then_some(&field as &dyn CollisionField);
        let mut build_input = BranchBuildInput {
            tip_pos: read(&input["tipPos"]),
            tip_normal: read(&input["tipNormal"]),
            model_id: input["modelId"].as_str().unwrap(),
            parent_knot: &parent_knot,
            mesh,
            shaft_diameter_mm: opt(&input["shaftDiameterMm"]),
            tip_contact_diameter_mm: opt(&input["tipContactDiameterMm"]),
            roots_diameter_mm: opt(&input["rootsDiameterMm"]),
            settings: own.as_ref(),
            tip_profile: opt(&input["tipProfile"]),
        };
        let result = build_branch_data(&build_input, &live, &mut counter_ids());
        check(&format!("branches[{i}].result"), &result, &c["result"]);
        let tip = build_input.tip_pos;
        build_input.tip_pos = Vec3::new(tip.x + 0.3, tip.y, tip.z);
        let rebuilt = build_branch_data(&build_input, &live, &mut counter_ids());
        check(&format!("branches[{i}].rebuilt"), &rebuilt, &c["rebuilt"]);
        check(
            &format!("branches[{i}].remapped"),
            remap_branch_geometry_ids(&rebuilt.branch, &result.branch),
            &c["remapped"],
        );
    }

    for (i, c) in list(&fixture, "leaves").iter().enumerate() {
        let settings: SupportSettings = read(&c["settings"]);
        let input = &c["input"];
        let parent_knot: Knot = read(&input["parentKnot"]);
        let mesh = c["withMesh"]
            .as_bool()
            .unwrap()
            .then_some(&field as &dyn CollisionField);
        let result = build_leaf_data(
            &LeafBuildInput {
                tip_pos: read(&input["tipPos"]),
                surface_normal: read(&input["surfaceNormal"]),
                model_id: input["modelId"].as_str().unwrap(),
                parent_knot: &parent_knot,
                host_diameter_mm: num(&input["hostDiameterMm"]),
                tip_contact_diameter_mm: opt(&input["tipContactDiameterMm"]),
                mesh,
            },
            &settings,
            &mut counter_ids(),
        );
        check(&format!("leaves[{i}]"), &result, &c["result"]);
    }
}
