//! Parity with DragonFruit for block 4: `fixtures/df_v3.json` records
//! DragonFruit's own answers (`scripts/df-oracle/golden_v3.ts`, DragonFruit
//! f331fc55, `uuid` replaced by a counter) for the EscapeJointSearch walks
//! against the analytic box model its tests use, the search utilities, the
//! grid ring keys, and `calculateSmartPlacementV3` / `buildTrunkData` with a
//! mesh over five scenes, replayed in call order against one pooled distance
//! field per mesh, with the router counters compared after every call.
//!
//! DragonFruit's `smartPlacementV3.test.ts` is ported at the end.

use std::collections::HashMap;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::Value;

use support_gen_df::supports::placement_logic::collision_avoidance::CollisionField;
use support_gen_df::supports::placement_logic::cone_axis_policy::cone_keeps_contact_highest;
use support_gen_df::supports::placement_logic::grid::nearest_candidate_node_keys::build_nearest_candidate_node_keys;
use support_gen_df::supports::placement_logic::pathfinding::sdf_cache::SdfCache;
use support_gen_df::supports::placement_logic::smart_placement_search_utils as su;
use support_gen_df::supports::placement_logic::standard_placement::{
    TrunkPlacementInput, TrunkPlacementResult,
};
use support_gen_df::supports::placement_logic_v3::escape_joint_search::*;
use support_gen_df::supports::placement_logic_v3::smart_placement_v3::*;
use support_gen_df::supports::settings::types::SupportSettings;
use support_gen_df::supports::support_types::trunk::trunk_builder::*;
use support_gen_df::supports::types::Vec3;
use support_gen_df::three::Matrix4;
use support_gen_df::three::math_utils::{js_hypot, js_max, js_min};
use support_gen_df::three_mesh_bvh::MeshBvh;

fn fixture() -> Value {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/df_v3.json"
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

fn counter_ids() -> impl FnMut() -> String {
    let mut next = 0;
    move || {
        next += 1;
        format!("id-{next}")
    }
}

// ---------- the analytic model DragonFruit's tests search against ----------

#[derive(Debug, Clone, Copy, serde::Deserialize)]
struct BoxShape {
    min: [f64; 3],
    max: [f64; 3],
}

fn signed_distance_to_box(p: Vec3, b: &BoxShape) -> f64 {
    let qx = js_max(b.min[0] - p.x, p.x - b.max[0]);
    let qy = js_max(b.min[1] - p.y, p.y - b.max[1]);
    let qz = js_max(b.min[2] - p.z, p.z - b.max[2]);
    js_hypot(&[js_max(qx, 0.0), js_max(qy, 0.0), js_max(qz, 0.0)])
        + js_min(js_max(js_max(qx, qy), qz), 0.0)
}

struct BoxField(Vec<BoxShape>);

impl BoxField {
    fn distance_at(&self, p: Vec3) -> f64 {
        self.0.iter().fold(f64::INFINITY, |best, b| {
            js_min(best, signed_distance_to_box(p, b))
        })
    }
}

impl CollisionField for BoxField {
    fn cell_size(&self) -> f64 {
        0.5
    }

    fn segment_blocked(&self, a: Vec3, b: Vec3, clearance: f64) -> bool {
        let len = js_hypot(&[b.x - a.x, b.y - a.y, b.z - a.z]);
        let steps = js_max(1.0, (len / 0.25).ceil());
        let mut i = 0.0;
        while i <= steps {
            let t = i / steps;
            let p = Vec3::new(
                a.x + (b.x - a.x) * t,
                a.y + (b.y - a.y) * t,
                a.z + (b.z - a.z) * t,
            );
            if self.distance_at(p) < clearance {
                return true;
            }
            i += 1.0;
        }
        false
    }

    fn exact_signed_distance_at(&self, point: Vec3) -> f64 {
        self.distance_at(point)
    }
}

/// The oracle's deterministic, sparse refusal, so the base gate is exercised.
fn sparse_base_fits(x: f64, y: f64) -> bool {
    ((x * 7.0).floor() + (y * 3.0).floor()) % 5.0 != 0.0
}

fn base_fits_for(sparse: bool) -> impl FnMut(f64, f64) -> bool {
    move |x, y| !sparse || sparse_base_fits(x, y)
}

#[test]
fn escape_and_grid_joint_searches_match_dragonfruit() {
    let fixture = fixture();
    for (i, c) in list(&fixture, "escape").iter().enumerate() {
        let field = BoxField(read(&c["boxes"]));
        let socket: Vec3 = read(&c["socket"]);
        let root_top_z = num(&c["rootTopZ"]);
        let o = &c["options"];
        let mut fits = base_fits_for(o["baseFitsAt"].as_bool().unwrap());
        let result = if c["kind"] == "grid" {
            find_grid_joint(
                &field,
                socket,
                root_top_z,
                GridJointSearchOptions {
                    leans_deg: &[],
                    clearance_mm: num(&o["clearanceMm"]),
                    spacing_mm: num(&o["spacingMm"]),
                    max_lateral_mm: num(&o["maxLateralMm"]),
                    lean_from_vertical_deg: num(&o["leanFromVerticalDeg"]),
                    min_vertical_leg_mm: num(&o["minVerticalLegMm"]),
                    max_node_count: read(&o["maxNodeCount"]),
                    preferred_direction: opt(&o["preferredDirection"]),
                    base_fits_at: &mut fits,
                },
            )
        } else {
            find_escape_joint(
                &field,
                socket,
                root_top_z,
                EscapeJointSearchOptions {
                    clearance_mm: num(&o["clearanceMm"]),
                    step_mm: num(&o["stepMm"]),
                    max_lateral_mm: num(&o["maxLateralMm"]),
                    lean_from_vertical_deg: num(&o["leanFromVerticalDeg"]),
                    min_vertical_leg_mm: num(&o["minVerticalLegMm"]),
                    directions: read(&o["directions"]),
                    base_fits_at: &mut fits,
                },
            )
        };
        check(&format!("escape[{i}].{}", c["kind"]), result, &c["result"]);
    }

    for (i, c) in list(&fixture, "fans").iter().enumerate() {
        let fan = build_direction_fan(opt(&c["preferred"]), read(&c["count"]));
        check(&format!("fans[{i}]"), fan, &c["result"]);
    }
}

#[test]
fn search_utils_and_node_keys_match_dragonfruit() {
    let fixture = fixture();
    for (i, c) in list(&fixture, "nodeKeys").iter().enumerate() {
        let keys = build_nearest_candidate_node_keys(c["key"].as_str().unwrap(), read(&c["rings"]));
        check(&format!("nodeKeys[{i}]"), keys, &c["result"]);
    }

    for (i, c) in list(&fixture, "searchUtils").iter().enumerate() {
        let a: Vec3 = read(&c["a"]);
        let b: Vec3 = read(&c["b"]);
        let (len, base) = (num(&c["len"]), num(&c["base"]));
        let floor: Option<f64> = c.get("floor").and_then(opt);
        let actual = serde_json::json!({
            "distanceXY": su::distance_xy(a, b),
            "distance3D": su::distance_3d(a, b),
            // JSON writes ±Infinity as null, and so does serde_json.
            "fromHorizontal": su::segment_angle_from_horizontal_deg(a, b),
            "fromVertical": su::segment_angle_from_vertical_deg(a, b),
            "departure": su::member_departure_angle_from_vertical_deg(a, b),
            "spanLean": su::span_lean_from_vertical_deg(a, b),
            "lengthAware": su::get_length_aware_max_angle_from_vertical_deg(len, base, floor),
            "satisfiesLengthAware":
                su::segment_satisfies_length_aware_max_angle_from_vertical(a, b, base, floor),
            "socketElbow": su::first_segment_satisfies_socket_elbow_max_angle(a, b, base),
            "positionKey": su::position_key(a),
        });
        let mut expected = c.clone();
        let e = expected.as_object_mut().unwrap();
        // The chain's third point was drawn at random and not recorded.
        e.remove("chain");
        for key in ["a", "b", "len", "base", "floor"] {
            e.remove(key);
        }
        check(&format!("searchUtils[{i}]"), actual, &expected);
    }
}

// ---------- the router ----------

struct Scene {
    bvh: Arc<MeshBvh>,
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
            let index: Option<Vec<u32>> = opt(&m["index"]);
            let bvh = Arc::new(MeshBvh::new(positions, index));
            (name.clone(), Scene { bvh })
        })
        .collect()
}

fn matrix(value: &Value) -> Matrix4 {
    let e: Vec<f64> = read(value);
    Matrix4::from_column_major(e.try_into().unwrap())
}

/// `snappedNodeKey: null` is the port's absent key (both mean "no node").
fn without_null_node_key(mut value: Value) -> Value {
    if let Some(o) = value.as_object_mut()
        && o.get("snappedNodeKey") == Some(&Value::Null)
    {
        o.remove("snappedNodeKey");
    }
    value
}

fn v3_input(input: &Value) -> SmartPlacementV3Input<'_> {
    SmartPlacementV3Input {
        placement: TrunkPlacementInput {
            tip_pos: read(&input["tipPos"]),
            tip_normal: read(&input["tipNormal"]),
            tip_profile: read(&input["tipProfile"]),
            roots_top_z: num(&input["rootsTopZ"]),
            is_small_island: input.get("isSmallIsland").and_then(opt),
        },
        model_id: input["modelId"].as_str().unwrap(),
        is_preview: input.get("isPreview").and_then(opt),
    }
}

fn trunk_input(input: &Value) -> TrunkBuildInput<'_> {
    let overrides = input.get("overrides").map(|o| TrunkOverrides {
        roots_diameter_mm: o.get("rootsDiameterMm").and_then(opt),
        roots_disk_height_mm: o.get("rootsDiskHeightMm").and_then(opt),
        roots_cone_height_mm: o.get("rootsConeHeightMm").and_then(opt),
        shaft_diameter_mm: o.get("shaftDiameterMm").and_then(opt),
        tip_contact_diameter_mm: o.get("tipContactDiameterMm").and_then(opt),
        tip_body_diameter_mm: o.get("tipBodyDiameterMm").and_then(opt),
        tip_length_mm: o.get("tipLengthMm").and_then(opt),
        tip_disk_length_override_mm: o.get("tipDiskLengthOverrideMm").and_then(opt),
        tip_penetration_mm: None,
    });
    TrunkBuildInput {
        tip_pos: read(&input["tipPos"]),
        tip_normal: read(&input["tipNormal"]),
        model_id: input["modelId"].as_str().unwrap(),
        mesh: None,
        is_preview: input.get("isPreview").and_then(opt),
        overrides,
        is_small_island: input.get("isSmallIsland").and_then(opt),
        ignore_grid: input.get("ignoreGrid").and_then(opt),
    }
}

/// Every recorded router call, replayed in order, with the router counters
/// after each. Returns the contract cases' placements by name.
fn replay(fixture: &Value) -> HashMap<String, (TrunkPlacementResult, Value)> {
    let scenes = scenes(fixture);
    // DragonFruit's pool: one field per mesh, built on first use.
    let mut pool: HashMap<String, SdfCache> = HashMap::new();
    let mut stats = RouterStats::default();
    let mut contract = HashMap::new();

    for (i, c) in list(fixture, "ops").iter().enumerate() {
        let scene = c["scene"].as_str().unwrap();
        let matrix_world = matrix(&c["matrix"]);
        let sdf = pool
            .entry(scene.to_owned())
            .or_insert_with(|| SdfCache::new(scenes[scene].bvh.clone(), matrix_world, Some(0.5)));
        let settings: SupportSettings = read(&c["settings"]);
        let input = &c["input"];
        match c["op"].as_str().unwrap() {
            "v3" => {
                let ignore_grid = c.get("ignoreGrid").and_then(Value::as_bool) == Some(true);
                let result = calculate_smart_placement_v3(
                    &v3_input(input),
                    sdf,
                    matrix_world,
                    &mut stats,
                    &settings,
                    SmartPlacementV3Context { ignore_grid },
                );
                check(
                    &format!("ops[{i}].v3"),
                    &result,
                    &without_null_node_key(c["result"].clone()),
                );
                if let Some(name) = c.get("name").and_then(Value::as_str) {
                    contract.insert(name.to_owned(), (result, c.clone()));
                }
            }
            _ => {
                let result = build_trunk_data_routed(
                    &trunk_input(input),
                    &mut TrunkRouter {
                        sdf,
                        matrix_world,
                        stats: &mut stats,
                    },
                    &settings,
                    &mut counter_ids(),
                );
                check(&format!("ops[{i}].buildTrunkData"), &result, &c["result"]);
            }
        }
        check(&format!("ops[{i}].stats"), &stats, &c["stats"]);
    }
    contract
}

#[test]
fn router_and_routed_trunks_match_dragonfruit() {
    let fixture = fixture();
    let contract = replay(&fixture);
    assert_eq!(contract.len(), 3);
}

// ---------- DragonFruit's smartPlacementV3.test.ts ----------

fn search(boxes: &[BoxShape], socket: Vec3) -> EscapeJointSearchResult {
    let field = BoxField(boxes.to_vec());
    find_escape_joint(
        &field,
        socket,
        0.0,
        EscapeJointSearchOptions {
            clearance_mm: 0.98,
            step_mm: 0.5,
            max_lateral_mm: 48.0,
            lean_from_vertical_deg: 45.0,
            min_vertical_leg_mm: 1.0,
            directions: vec![
                Xy { x: 1.0, y: 0.0 },
                Xy { x: -1.0, y: 0.0 },
                Xy { x: 0.0, y: 1.0 },
                Xy { x: 0.0, y: -1.0 },
            ],
            base_fits_at: &mut |_, _| true,
        },
    )
}

fn boxed(min: [f64; 3], max: [f64; 3]) -> BoxShape {
    BoxShape { min, max }
}

#[test]
fn escape_takes_the_closest_column_that_clears_at_45_degrees() {
    // Body under the socket spanning x ∈ [-3, 5]: clearing it costs 6 mm of
    // travel to +x but only 4 mm to -x.
    let socket = Vec3::new(0.0, 0.0, 20.0);
    let result = search(&[boxed([-3.0, -5.0, 0.0], [5.0, 5.0, 12.0])], socket);
    assert_eq!(result.outcome, EscapeOutcome::Found);
    let joint = result.joint.unwrap();
    assert!((joint.lateral_mm - (socket.z - joint.joint.z)).abs() < 1e-9);
    assert_eq!(joint.lean_from_vertical_deg, 45.0);
    // The nearer column wins over the first direction tried.
    assert!(
        joint.lateral_mm > 3.9 && joint.lateral_mm < 4.4,
        "{}",
        joint.lateral_mm
    );
    assert!(joint.joint.x < 0.0);
}

#[test]
fn escape_does_not_flatten_the_trunk_over_a_lip() {
    let result = search(
        &[
            boxed([-5.0, -100.0, 0.0], [5.0, 100.0, 12.0]),
            boxed([3.0, -100.0, 0.0], [5.0, 100.0, 15.0]),
            boxed([-5.0, -100.0, 0.0], [-3.0, 100.0, 15.0]),
        ],
        Vec3::new(0.0, 0.0, 20.0),
    );
    assert_eq!(result.joint, None, "no flat leg, no joint");
    assert_eq!(result.outcome, EscapeOutcome::NeverCleared);
}

#[test]
fn escape_gives_up_instead_of_growing_an_unbounded_search() {
    let result = search(
        &[boxed([-100.0, -100.0, 0.0], [100.0, 100.0, 19.0])],
        Vec3::new(0.0, 0.0, 20.0),
    );
    assert_eq!(result.joint, None);
    assert!(result.probes > 0);
}

#[test]
fn grid_joint_keeps_heading_the_way_the_cone_points() {
    let field = BoxField(vec![boxed([-2.0, -5.0, 0.0], [2.0, 5.0, 15.0])]);
    let socket = Vec3::new(0.0, 0.0, 20.0);
    let run = |preferred: Xy| {
        find_grid_joint(
            &field,
            socket,
            0.0,
            GridJointSearchOptions {
                leans_deg: &[],
                clearance_mm: 0.98,
                spacing_mm: 4.0,
                max_lateral_mm: 48.0,
                lean_from_vertical_deg: 45.0,
                min_vertical_leg_mm: 1.0,
                max_node_count: 24,
                preferred_direction: Some(preferred),
                base_fits_at: &mut |_, _| true,
            },
        )
    };
    let right = run(Xy { x: 1.0, y: 0.0 }).joint.unwrap().joint;
    assert_eq!(right.x, 4.0, "goes right when the cone points right");
    assert_eq!(right.z, 16.0, "lands on the node at a 45 degree drop");
    let left = run(Xy { x: -1.0, y: 0.0 }).joint.unwrap().joint;
    assert_eq!(left.x, -4.0, "goes left when the cone points left");
}

fn angle_from_vertical_deg(a: Vec3, b: Vec3) -> f64 {
    (js_hypot(&[b.x - a.x, b.y - a.y]).atan2((b.z - a.z).abs()) * 180.0) / std::f64::consts::PI
}

#[test]
fn placement_contract_holds() {
    let fixture = fixture();
    let contract = replay(&fixture);

    // A routed trunk is one diagonal and one vertical drop.
    let (jaw, _) = &contract["jaw"];
    assert_eq!(jaw.error, None);
    let joints = jaw.joints.as_ref().unwrap();
    assert_eq!(joints.len(), 1);
    let joint = joints[0];
    assert!(angle_from_vertical_deg(jaw.socket_pos, joint) <= 45.05);
    assert!(js_hypot(&[jaw.base_pos.x - joint.x, jaw.base_pos.y - joint.y]) < 0.2);
    assert!(joint.z > 10.0);

    // With the grid on, the joint is the node and the drop stays vertical.
    let (grid, _) = &contract["jawGrid"];
    assert_eq!(grid.error, None);
    let joint = grid.joints.as_ref().unwrap()[0];
    assert_eq!(grid.joints.as_ref().unwrap().len(), 1);
    assert!(grid.snapped_node_key.is_some());
    assert_eq!(grid.base_pos.x, joint.x);
    assert_eq!(grid.base_pos.y, joint.y);
    for v in [joint.x, joint.y] {
        let cells = v / 2.0;
        assert!((cells - cells.round()).abs() < 1e-9);
    }

    // A cone swung off a wall keeps its socket end under the contact.
    let (wall, case) = &contract["wall"];
    assert_eq!(wall.error, None);
    let tip = Vec3::new(0.0, 0.0, 15.0);
    let axis = wall.cone_axis.unwrap();
    let body: f64 = num(&case["input"]["tipProfile"]["bodyDiameterMm"]);
    assert!(cone_keeps_contact_highest(axis, wall.socket_pos, tip, body));
    assert!(axis.z < 0.0);
    assert!(tip.z - wall.socket_pos.z > 0.5);
}
