//! Parity with DragonFruit for block 2: `fixtures/df_sdf.json` records, in call
//! order, what DragonFruit's own three-mesh-bvh, SDFCache, ColumnClearanceMap,
//! constraint solvers, curveUtils, moveJoint and CollisionAvoidance answered
//! (`scripts/df-oracle/golden_sdf.ts`, DragonFruit f331fc55). The port replays
//! the same calls in the same order — the SDF cache makes order matter — and
//! must agree bit for bit, including the BVH's reordered triangle index.

use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::Value;

use support_gen_df::supports::curves::curve_utils::*;
use support_gen_df::supports::placement_logic::collision_avoidance::*;
use support_gen_df::supports::placement_logic::joint_constraint_solver::{
    JointStructure, solve_joint_constraint,
};
use support_gen_df::supports::placement_logic::pathfinding::column_clearance_map::{
    ColumnClearanceMap, ColumnVerdict,
};
use support_gen_df::supports::placement_logic::pathfinding::sdf_cache::SdfCache;
use support_gen_df::supports::placement_logic::shaft_angle_constraint::clamp_shaft_angle;
use support_gen_df::supports::support_primitives::joint::joint_utils::{
    MoveJointContext, move_joint,
};
use support_gen_df::supports::types::*;
use support_gen_df::three::{Matrix4, Vec3};
use support_gen_df::three_mesh_bvh::MeshBvh;

fn fixture() -> Value {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/df_sdf.json"
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

/// `JSON.stringify` writes Infinity and NaN as null.
fn json_number(x: f64) -> Value {
    if x.is_finite() {
        Value::from(x)
    } else {
        Value::Null
    }
}

/// Compares two JSON graphs exactly (numbers as f64 bits).
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

fn matrix(value: &Value) -> Matrix4 {
    let e: Vec<f64> = read(value);
    Matrix4::from_column_major(e.try_into().unwrap())
}

#[test]
fn bvh_and_sdf_cache_match_dragonfruit() {
    let fixture = fixture();
    for mesh in fixture["meshes"].as_array().unwrap() {
        let name = mesh["name"].as_str().unwrap();
        let bvh = Arc::new(MeshBvh::new(positions(&mesh["positions"]), None));

        // The build reorders the index exactly as three-mesh-bvh does.
        let index: Vec<u32> = read(&mesh["index"]);
        assert_eq!(bvh.index(), index.as_slice(), "{name}: reordered index");

        for (i, c) in mesh["closest"].as_array().unwrap().iter().enumerate() {
            let max_t = opt::<f64>(&c["maxT"]).unwrap_or(f64::INFINITY);
            let result = bvh.closest_point_to_point(read(&c["p"]), num(&c["minT"]), max_t).map(|r| {
                serde_json::json!({ "point": r.point, "distance": r.distance, "faceIndex": r.face_index })
            });
            check(&format!("{name}.closest[{i}]"), result, &c["result"]);
        }

        let mut sdf = SdfCache::new(bvh.clone(), matrix(&mesh["matrix"]), Some(0.5));
        for (i, op) in mesh["ops"].as_array().unwrap().iter().enumerate() {
            let path = format!("{name}.ops[{i}].{}", op["op"].as_str().unwrap());
            let p: Vec3 = if op["p"].is_null() {
                Vec3::ZERO
            } else {
                read(&op["p"])
            };
            let result = match op["op"].as_str().unwrap() {
                "exactSignedDistanceAt" => json_number(sdf.exact_signed_distance_at(p.x, p.y, p.z)),
                "distanceAt" => json_number(sdf.distance_at(p.x, p.y, p.z)),
                "boundedDistanceAt" => {
                    json_number(sdf.bounded_distance_at(p.x, p.y, p.z, num(&op["b"])))
                }
                "distanceAtTrilinear" => json_number(sdf.distance_at_trilinear(p.x, p.y, p.z)),
                "distanceAndGradientAt" => {
                    let m = opt::<f64>(&op["m"]).unwrap_or(f64::INFINITY);
                    let (distance, gradient) = sdf.distance_and_gradient_at(p.x, p.y, p.z, m);
                    serde_json::json!({ "distance": json_number(distance), "gradient": gradient })
                }
                "distanceAtWithin" => {
                    json_number(sdf.distance_at_within(p.x, p.y, p.z, num(&op["m"])))
                }
                "enableColumnMap" => {
                    Value::Bool(sdf.enable_column_map(num(&op["c"]), num(&op["cell"])))
                }
                "segmentBlocked" => {
                    let (a, b): (Vec3, Vec3) = (read(&op["a"]), read(&op["b"]));
                    Value::Bool(sdf.segment_blocked_xyz(
                        a.x,
                        a.y,
                        a.z,
                        b.x,
                        b.y,
                        b.z,
                        num(&op["c"]),
                    ))
                }
                other => panic!("unknown op {other}"),
            };
            assert_same(&result, &op["result"], &path);
        }
        // The cache did the same work: same reads, same BVH queries, same cells.
        let stats = sdf.stats();
        assert_eq!(
            stats.cell_reads as f64,
            num(&mesh["stats"]["cellReads"]),
            "{name}: cell reads"
        );
        assert_eq!(
            stats.bvh_queries as f64,
            num(&mesh["stats"]["bvhQueries"]),
            "{name}: BVH queries"
        );
        assert_eq!(
            sdf.size() as f64,
            num(&mesh["size"]),
            "{name}: cached cells"
        );

        if let Some(column) = mesh["column"].as_object() {
            let map =
                ColumnClearanceMap::build(bvh.positions(), &matrix(&mesh["matrix"]), 0.6, 0.25)
                    .unwrap();
            assert_eq!(
                map.stats.width as f64,
                num(&column["width"]),
                "{name}: column width"
            );
            assert_eq!(
                map.stats.height as f64,
                num(&column["height"]),
                "{name}: column height"
            );
            assert_eq!(
                map.stats.radius_mm,
                num(&column["radiusMm"]),
                "{name}: column radius"
            );
            for (i, s) in column["samples"].as_array().unwrap().iter().enumerate() {
                let p: Vec3 = read(&s["p"]);
                check(
                    &format!("{name}.column[{i}].lowest"),
                    map.lowest_blocked_z(p.x, p.y),
                    &s["lowest"],
                );
                let verdict = match map.column_verdict(p.x, p.y, p.z, num(&s["zb"])) {
                    ColumnVerdict::Blocked => "blocked",
                    ColumnVerdict::Unknown => "unknown",
                };
                assert_eq!(
                    verdict,
                    s["verdict"].as_str().unwrap(),
                    "{name}.column[{i}].verdict"
                );
            }
        }
    }
}

#[test]
fn solvers_curves_move_joint_and_collision_match_dragonfruit() {
    let fixture = fixture();
    let solvers = &fixture["solvers"];
    let knot_bvh = Arc::new(MeshBvh::new(positions(&fixture["knotPositions"]), None));
    // DragonFruit's pooled field for the knot mesh, shared by moveJoint and the gates.
    let field = SdfCache::new(knot_bvh, Matrix4::IDENTITY, Some(0.5));
    let no_knots = indexmap::IndexMap::new();

    for (i, c) in solvers["clampShaftAngle"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        let r = clamp_shaft_angle(
            read(&c["start"]),
            read(&c["end"]),
            num(&c["maxA"]),
            read(&c["axis"]),
        );
        let result = serde_json::json!({ "clampedPos": r.clamped_pos, "isClamped": r.is_clamped, "angleDeg": r.angle_deg });
        check(&format!("clampShaftAngle[{i}]"), result, &c["result"]);
    }

    for (i, c) in solvers["solveJointConstraint"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        let trunk: Trunk = read(&c["trunk"]);
        let root: Roots = read(&c["root"]);
        let result = solve_joint_constraint(
            JointStructure::Trunk(&trunk),
            c["jointId"].as_str().unwrap(),
            read(&c["target"]),
            num(&c["maxA"]),
            c["giveRoot"].as_bool().unwrap().then_some(&root),
            opt(&c["parentStart"]),
            &no_knots,
        );
        check(&format!("solveJointConstraint[{i}]"), result, &c["result"]);
    }

    for (i, c) in solvers["curves"].as_array().unwrap().iter().enumerate() {
        let trunk: Trunk = read(&c["trunk"]);
        let root: Roots = read(&c["root"]);
        let joint_id = c["jointId"].as_str().unwrap();
        let seg_id = c["segId"].as_str().unwrap();
        let result = match c["op"].as_u64().unwrap() {
            0 => update_curves_at_joint(&trunk, joint_id, &root, c["force"].as_bool().unwrap()),
            1 => update_segment_tension(&trunk, seg_id, num(&c["tension"]), &root),
            2 => update_segment_bias(&trunk, seg_id, num(&c["bias"]), &root),
            3 => remove_segment_curve(&trunk, seg_id),
            4 => update_curve_tension(&trunk, joint_id, num(&c["tension"]), &root),
            5 => remove_curve_at_joint(&trunk, joint_id),
            _ => {
                let first = trunk
                    .segments
                    .first()
                    .cloned()
                    .unwrap_or_else(|| Segment::straight("x", 1.0));
                Trunk {
                    segments: vec![ensure_bezier_segment(&first)],
                    ..trunk.clone()
                }
            }
        };
        check(&format!("curves[{i}] op {}", c["op"]), result, &c["result"]);
    }

    let moves = solvers["moveJoint"].as_array().unwrap();
    let gates = solvers["collision"].as_array().unwrap();
    assert_eq!(moves.len(), gates.len());
    // Interleaved as DragonFruit called them: the pooled field caches across both.
    for (i, (c, g)) in moves.iter().zip(gates).enumerate() {
        let trunk: Trunk = read(&c["trunk"]);
        let root: Roots = read(&c["root"]);
        let result = move_joint(
            &trunk,
            c["jointId"].as_str().unwrap(),
            read(&c["target"]),
            MoveJointContext {
                mesh: c["withMesh"]
                    .as_bool()
                    .unwrap()
                    .then_some(&field as &dyn CollisionField),
                is_curve_mode: c["curveMode"].as_bool().unwrap(),
                root: c["giveRoot"].as_bool().unwrap().then_some(&root),
                parent_start_pos: opt(&c["parentStart"]),
                skip_contact_cone_solve: c["skipCone"].as_bool().unwrap(),
                max_angle_deg: opt(&c["maxAngle"]),
                knots: None,
            },
        );
        check(&format!("moveJoint[{i}]"), result, &c["result"]);

        let result = match g["op"].as_str().unwrap() {
            "frustum" => is_collision_frustum_blocked(
                read(&g["a"]),
                read(&g["b"]),
                num(&g["r0"]),
                num(&g["r1"]),
                &field,
                opt(&g["n"]),
            )
            .into(),
            "safeOffset" => {
                let frustum = g["frustum"].as_object().map(|f| CollisionFrustumProfile {
                    start_radius: num(&f["startRadius"]),
                    end_radius: num(&f["endRadius"]),
                    segment_count: None,
                });
                json_number(calculate_safe_offset(
                    read(&g["a"]),
                    read(&g["n"]),
                    read(&g["b"]),
                    num(&g["radius"]),
                    &field,
                    num(&g["minO"]),
                    num(&g["maxO"]),
                    0.2,
                    frustum,
                ))
            }
            "contactCone" => {
                let cone = &g["cone"];
                is_contact_cone_blocked(
                    &field,
                    ContactConeSweep {
                        start: read(&cone["start"]),
                        end: read(&cone["end"]),
                        start_radius: num(&cone["startRadius"]),
                        end_radius: num(&cone["endRadius"]),
                    },
                )
                .into()
            }
            other => panic!("unknown gate {other}"),
        };
        assert_same(
            &result,
            &g["result"],
            &format!("collision[{i}].{}", g["op"]),
        );
    }
}
