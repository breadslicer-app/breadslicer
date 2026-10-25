//! Parity with three.js r181 for block 9b: `fixtures/df_extrude.json`
//! (`scripts/df-oracle/golden_extrude.ts`) records Earcut triangulations,
//! `ShapeUtils.triangulateShape`, bevel-less `ExtrudeGeometry` over shapes
//! built the two ways DragonFruit's raft code builds them, and indexed soups
//! through `computeVertexNormals`, `toNonIndexed`, `mergeGeometries` and
//! `translate`. Every index, and every float as its `f32`, must match.

use serde_json::{Value, json};

use support_gen_df::three::Vector2;
use support_gen_df::three::buffer_geometry::{BufferGeometry, merge_geometries};
use support_gen_df::three::earcut::earcut;
use support_gen_df::three::extrude_geometry::{ExtrudeOptions, extrude_geometry};
use support_gen_df::three::shape::{Path, Shape};
use support_gen_df::three::shape_utils::triangulate_shape;

fn load() -> Value {
    let path = format!(
        "{}/tests/fixtures/df_extrude.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn points(v: &Value) -> Vec<Vector2> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|p| Vector2::new(p["x"].as_f64().unwrap(), p["y"].as_f64().unwrap()))
        .collect()
}

fn wire_points(p: &[Vector2]) -> Value {
    Value::Array(p.iter().map(|q| json!({ "x": q.x, "y": q.y })).collect())
}

/// The oracle's `packF32`: up to 600 values in full, else length and FNV-1a.
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
            assert!(a.as_f64() == e.as_f64(), "{path}: port {a} vs three {e}");
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

#[test]
fn earcut_matches_three() {
    let fixture = load();
    let mut hashed = 0;
    for (k, case) in fixture["earcut"].as_array().unwrap().iter().enumerate() {
        let path = format!("earcut[{k}]");
        no_error(case, &path);
        let data: Vec<f64> = case["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        let holes: Vec<usize> = case["holeIndices"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as usize)
            .collect();
        if data.len() > 160 {
            hashed += 1;
        }
        assert_same(&json!(earcut(&data, &holes, 2)), &case["triangles"], &path);
    }
    assert!(hashed > 50, "the z-order path is exercised ({hashed})");
}

#[test]
fn triangulate_shape_matches_three() {
    let fixture = load();
    for (k, case) in fixture["triangulate"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        let path = format!("triangulate[{k}]");
        no_error(case, &path);
        let mut contour = points(&case["contour"]);
        let mut holes: Vec<Vec<Vector2>> = case["holes"]
            .as_array()
            .unwrap()
            .iter()
            .map(points)
            .collect();
        let faces = triangulate_shape(&mut contour, &mut holes);
        assert_same(&json!(faces), &case["faces"], &format!("{path}.faces"));
        assert_same(
            &wire_points(&contour),
            &case["contourAfter"],
            &format!("{path}.contour"),
        );
        let holes_after = Value::Array(holes.iter().map(|h| wire_points(h)).collect());
        assert_same(&holes_after, &case["holesAfter"], &format!("{path}.holes"));
    }
}

#[test]
fn extrude_geometry_matches_three() {
    let fixture = load();
    for (k, case) in fixture["extrude"].as_array().unwrap().iter().enumerate() {
        let path = format!("extrude[{k}] {}", case["style"].as_str().unwrap());
        no_error(case, &path);
        let set = case["style"] == "set";
        let shapes: Vec<Shape> = case["polys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|poly| {
                let outer = points(&poly["outer"]);
                let holes: Vec<Vec<Vector2>> = poly["holes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(points)
                    .collect();
                if set {
                    let mut shape = Shape::new(Some(&outer));
                    for h in &holes {
                        let mut p = Path::new(Some(h));
                        p.close_path();
                        shape.holes.push(p);
                    }
                    shape
                } else {
                    let mut shape = Shape::new(None);
                    shape.path.move_to(outer[0].x, outer[0].y);
                    for p in &outer[1..] {
                        shape.path.line_to(p.x, p.y);
                    }
                    shape.path.close_path();
                    for h in &holes {
                        let mut p = Path::new(None);
                        p.move_to(h[0].x, h[0].y);
                        for q in &h[1..] {
                            p.line_to(q.x, q.y);
                        }
                        p.close_path();
                        shape.holes.push(p);
                    }
                    shape
                }
            })
            .collect();
        let mut geom = extrude_geometry(
            &shapes,
            ExtrudeOptions {
                depth: case["depth"].as_f64().unwrap(),
                steps: 1,
            },
        );
        geom.compute_vertex_normals();
        if let Some(tz) = case["translateZ"].as_f64() {
            geom.translate(0.0, 0.0, tz);
        }
        assert_same(&dump(&geom), &case["geometry"], &path);
    }
}

#[test]
fn buffer_geometry_matches_three() {
    let fixture = load();
    for (k, case) in fixture["buffer"].as_array().unwrap().iter().enumerate() {
        let path = format!("buffer[{k}]");
        let geoms: Vec<BufferGeometry> = case["parts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|part| {
                let positions: Vec<f64> = part["positions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_f64().unwrap())
                    .collect();
                let mut g = BufferGeometry::default();
                g.set_position(&positions);
                g.index = Some(
                    part["index"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| v.as_u64().unwrap() as u32)
                        .collect(),
                );
                g.compute_vertex_normals();
                g
            })
            .collect();
        let indexed = Value::Array(geoms.iter().map(dump).collect());
        assert_same(&indexed, &case["indexed"], &format!("{path}.indexed"));
        let non_indexed: Vec<BufferGeometry> = geoms.iter().map(|g| g.to_non_indexed()).collect();
        let mut merged = merge_geometries(&non_indexed).unwrap();
        assert_same(&dump(&merged), &case["merged"], &format!("{path}.merged"));
        merged.compute_vertex_normals();
        merged.translate(0.25, -0.5, case["tz"].as_f64().unwrap());
        assert_same(&dump(&merged), &case["final"], &format!("{path}.final"));
    }
}
