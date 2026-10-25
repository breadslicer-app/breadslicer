//! Parity with DragonFruit's crenelated raft generator for block 9c:
//! `fixtures/df_rafts.json` (`scripts/df-oracle/golden_rafts.ts`) records the
//! polygon-set booleans, the raster region, the model plate footprint, the
//! ring helpers, the line-raft graph and meshes, and the raft step of
//! `ExportManager.exportScene` over generated scenes. Every polygon point and
//! every mesh float (as its `f32`) must match.

use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use support_gen_df::supports::rafts::crenelated::geometry::build_line_raft_edge_pairs::{
    BuildLineRaftEdgePairsOptions, build_line_raft_edge_pairs,
};
use support_gen_df::supports::rafts::crenelated::geometry::generate_chamfered_beam::generate_chamfered_beam;
use support_gen_df::supports::rafts::crenelated::geometry::generate_unioned_line_raft_mesh::{
    filter_line_raft_edges, generate_unioned_line_raft_mesh,
};
use support_gen_df::supports::rafts::crenelated::geometry::model_plate_footprint::{
    ModelTransform, PlateFootprintSource, collect_model_plate_footprint,
};
use support_gen_df::supports::rafts::crenelated::geometry::polygon_raster_2d::rasterize_polygons_to_region;
use support_gen_df::supports::rafts::crenelated::geometry::polygon_set_2d::{
    PolygonWithHoles, difference_polygon_sets, offset_polygon_set, union_polygon_sets,
};
use support_gen_df::supports::rafts::crenelated::geometry::small::{
    compute_footprint, compute_raft_outer_boundary, inset_convex_polygon,
};
use support_gen_df::supports::rafts::crenelated::raft_export::{
    RaftRoot, RaftSettingsStore, build_export_rafts,
};
use support_gen_df::supports::rafts::crenelated::raft_types::{RaftSettings, SupportBaseCircle};
use support_gen_df::three::buffer_geometry::BufferGeometry;
use support_gen_df::three::{Vec3, Vector2};

fn load() -> Value {
    let path = format!(
        "{}/tests/fixtures/df_rafts.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap()
}

fn pt(v: &Value) -> Vector2 {
    Vector2::new(f(&v["x"]), f(&v["y"]))
}

fn points(v: &Value) -> Vec<Vector2> {
    v.as_array().unwrap().iter().map(pt).collect()
}

fn set(v: &Value) -> Vec<PolygonWithHoles> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|p| PolygonWithHoles {
            outer: points(&p["outer"]),
            holes: p["holes"].as_array().unwrap().iter().map(points).collect(),
        })
        .collect()
}

fn wire_points(p: &[Vector2]) -> Value {
    Value::Array(p.iter().map(|q| json!({ "x": q.x, "y": q.y })).collect())
}

fn wire_set(polys: &[PolygonWithHoles]) -> Value {
    Value::Array(
        polys
            .iter()
            .map(|p| {
                json!({
                    "outer": wire_points(&p.outer),
                    "holes": p.holes.iter().map(|h| wire_points(h)).collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}

fn pack_f32(a: &[f32]) -> Value {
    if a.len() <= 600 {
        return Value::Array(a.iter().map(|&v| json!(v as f64)).collect());
    }
    let mut h: u32 = 2166136261;
    for v in a {
        h ^= v.to_bits();
        h = h.wrapping_mul(16777619);
    }
    json!({ "len": a.len(), "fnv": h })
}

fn dump(g: &BufferGeometry) -> Value {
    let attr = |a: &Option<Vec<f32>>| a.as_deref().map_or(Value::Null, pack_f32);
    json!({
        "position": attr(&g.position),
        "normal": attr(&g.normal),
        "uv": attr(&g.uv),
        "index": g.index.as_ref().map_or(Value::Null, |i| json!(i)),
        "groups": g.groups.iter().map(|gr| json!({
            "start": gr.start, "count": gr.count, "materialIndex": gr.material_index,
        })).collect::<Vec<_>>(),
    })
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
            for (k, ev) in e {
                assert_same(a.get(k).unwrap_or(&Value::Null), ev, &format!("{path}.{k}"));
            }
            assert_eq!(a.len(), e.len(), "{path}: fields differ");
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

fn no_error(case: &Value, path: &str) {
    assert!(
        case.get("error").is_none(),
        "{path}: the fixture holds no throwing case"
    );
}

fn v3(v: &Value) -> Vec3 {
    Vec3::new(f(&v[0]), f(&v[1]), f(&v[2]))
}

/// A model record's geometry (its stored floats) and placement.
fn model_geometry(m: &Value) -> BufferGeometry {
    BufferGeometry {
        position: Some(
            m["positions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| f(v) as f32)
                .collect(),
        ),
        index: m["index"]
            .as_array()
            .map(|i| i.iter().map(|v| v.as_u64().unwrap() as u32).collect()),
        ..Default::default()
    }
}

fn sources<'a>(models: &Value, geoms: &'a [BufferGeometry]) -> Vec<PlateFootprintSource<'a>> {
    models
        .as_array()
        .unwrap()
        .iter()
        .zip(geoms)
        .map(|(m, g)| PlateFootprintSource {
            geometry: g,
            center: v3(&m["center"]),
            transform: ModelTransform {
                position: v3(&m["transform"]["position"]),
                rotation: v3(&m["transform"]["rotation"]),
                scale: v3(&m["transform"]["scale"]),
            },
            visible: m["visible"].as_bool(),
        })
        .collect()
}

#[test]
fn polygon_sets_match_dragonfruit() {
    let fixture = load();
    for (k, case) in fixture["sets"].as_array().unwrap().iter().enumerate() {
        let path = format!("sets[{k}] {}", case["op"].as_str().unwrap());
        no_error(case, &path);
        let subject = set(&case["subject"]);
        let result = match case["op"].as_str().unwrap() {
            "union" => union_polygon_sets(&subject),
            "difference" => difference_polygon_sets(&subject, &set(&case["clip"])),
            _ => offset_polygon_set(&subject, f(&case["delta"])),
        }
        .unwrap();
        assert_same(&wire_set(&result), &case["result"], &path);
    }
}

#[test]
fn raster_region_matches_dragonfruit() {
    let fixture = load();
    for (k, case) in fixture["raster"].as_array().unwrap().iter().enumerate() {
        let soup: Vec<Vec<Vector2>> = case["soup"]
            .as_array()
            .unwrap()
            .iter()
            .map(points)
            .collect();
        assert_same(
            &wire_set(&rasterize_polygons_to_region(&soup)),
            &case["result"],
            &format!("raster[{k}]"),
        );
    }
}

#[test]
fn plate_footprint_matches_dragonfruit() {
    let fixture = load();
    for (k, case) in fixture["plate"].as_array().unwrap().iter().enumerate() {
        let path = format!("plate[{k}]");
        no_error(case, &path);
        let geoms: Vec<BufferGeometry> = case["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(model_geometry)
            .collect();
        let srcs = sources(&case["models"], &geoms);
        let result = collect_model_plate_footprint(&srcs, f(&case["band"])).unwrap();
        assert_same(&wire_set(&result), &case["result"], &path);
    }
}

#[test]
fn ring_helpers_match_dragonfruit() {
    let fixture = load();
    for (k, case) in fixture["rings"].as_array().unwrap().iter().enumerate() {
        let path = format!("rings[{k}]");
        let ring = points(&case["ring"]);
        let settings: RaftSettings = serde_json::from_value(case["settings"].clone()).unwrap();
        assert_same(
            &wire_points(&inset_convex_polygon(&ring, f(&case["d"]))),
            &case["inset"],
            &format!("{path}.inset"),
        );
        assert_same(
            &wire_points(&compute_raft_outer_boundary(&ring, &settings)),
            &case["outerBoundary"],
            &format!("{path}.outerBoundary"),
        );
        let circles: Vec<SupportBaseCircle> =
            serde_json::from_value(case["circles"].clone()).unwrap();
        assert_same(
            &wire_points(&compute_footprint(
                &circles,
                case["margin"].as_f64(),
                case["samples"].as_f64(),
            )),
            &case["footprint"],
            &format!("{path}.footprint"),
        );
    }
}

#[test]
fn line_raft_edge_pairs_match_dragonfruit() {
    let fixture = load();
    for (k, case) in fixture["edges"].as_array().unwrap().iter().enumerate() {
        let o = &case["options"];
        let options = BuildLineRaftEdgePairsOptions {
            has_border_ring: o["hasBorderRing"].as_bool().unwrap(),
            keep_factor: o["keepFactor"].as_f64(),
            abs_max_len: o["absMaxLen"].as_f64(),
            enforce_connected: o["enforceConnected"].as_bool(),
            include_spanning_tree_backbone: o["includeSpanningTreeBackbone"].as_bool(),
        };
        let pairs = build_line_raft_edge_pairs(&points(&case["nodes"]), options);
        let wire: Vec<[usize; 2]> = pairs.iter().map(|&(a, b)| [a, b]).collect();
        assert_same(&json!(wire), &case["result"], &format!("edges[{k}]"));
    }
}

#[test]
fn line_raft_meshes_match_dragonfruit() {
    let fixture = load();
    for (k, case) in fixture["lines"].as_array().unwrap().iter().enumerate() {
        let path = format!("lines[{k}]");
        no_error(case, &path);
        let edges: Vec<(Vector2, Vector2)> = case["edges"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| (pt(&e[0]), pt(&e[1])))
            .collect();
        let (width, height) = (f(&case["widthMm"]), f(&case["heightMm"]));
        let clearance = case["clearance"]
            .as_array()
            .map(|_| set(&case["clearance"]));
        let filtered = filter_line_raft_edges(&edges, clearance.as_deref(), width);
        let wire: Vec<Value> = filtered
            .iter()
            .map(|(a, b)| json!([{ "x": a.x, "y": a.y }, { "x": b.x, "y": b.y }]))
            .collect();
        assert_same(
            &Value::Array(wire),
            &case["filtered"],
            &format!("{path}.filtered"),
        );
        let border = case["border"].as_array().map(|_| points(&case["border"]));
        let chamfer = match &case["chamfer"] {
            Value::Null => None,
            Value::String(_) => Some(f64::NAN),
            v => Some(f(v)),
        };
        let mesh =
            generate_unioned_line_raft_mesh(&edges, width, height, border.as_deref(), chamfer)
                .unwrap();
        assert_same(&dump(&mesh), &case["mesh"], &format!("{path}.mesh"));
        let b = &case["beamArgs"];
        let z = f(&b["z"]);
        let angle = chamfer.filter(|a| !a.is_nan()).unwrap_or(90.0);
        let beam = generate_chamfered_beam(
            Vec3::new(f(&b["a"]["x"]), f(&b["a"]["y"]), z),
            Vec3::new(f(&b["b"]["x"]), f(&b["b"]["y"]), -z),
            width,
            height,
            angle,
        );
        assert_same(&dump(&beam), &case["beam"], &format!("{path}.beam"));
    }
}

#[test]
fn export_rafts_match_dragonfruit() {
    let fixture = load();
    let mut trimmed = 0;
    let mut line = 0;
    for (k, case) in fixture["export"].as_array().unwrap().iter().enumerate() {
        let path = format!("export[{k}]");
        no_error(case, &path);
        let roots: Vec<RaftRoot> = case["roots"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| RaftRoot {
                model_id: r["modelId"].as_str().map(str::to_owned),
                x: f(&r["transform"]["pos"]["x"]),
                y: f(&r["transform"]["pos"]["y"]),
                diameter: f(&r["diameter"]),
            })
            .collect();
        let scoped: Option<HashSet<String>> = case["scoped"]
            .as_array()
            .map(|s| s.iter().map(|v| v.as_str().unwrap().to_owned()).collect());
        let store = RaftSettingsStore {
            global: serde_json::from_value(case["global"].clone()).unwrap(),
            per_model: serde_json::from_value::<HashMap<String, RaftSettings>>(
                case["perModel"].clone(),
            )
            .unwrap(),
            ..Default::default()
        };
        let geoms: Vec<BufferGeometry> = case["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(model_geometry)
            .collect();
        let srcs = sources(&case["models"], &geoms);
        let groups = build_export_rafts(&roots, scoped.as_ref(), &store, &srcs).unwrap();
        let expected = case["groups"].as_array().unwrap();
        assert_eq!(groups.len(), expected.len(), "{path}: group count");
        for (g, e) in groups.iter().zip(expected) {
            let gp = format!("{path}.{}", g.model_key);
            assert_eq!(json!(g.model_key), e["modelKey"], "{gp}");
            let fp = set(&e["footprint"]);
            if fp.len() > 1 || fp.iter().any(|p| !p.holes.is_empty()) {
                trimmed += 1;
            }
            if e["meshes"].as_array().unwrap().len() > 2 {
                line += 1;
            }
            let meshes = Value::Array(g.meshes.iter().map(dump).collect());
            assert_same(&meshes, &e["meshes"], &format!("{gp}.meshes"));
        }
    }
    assert!(trimmed > 5, "trimmed footprints are exercised ({trimmed})");
    let _ = line;
}
