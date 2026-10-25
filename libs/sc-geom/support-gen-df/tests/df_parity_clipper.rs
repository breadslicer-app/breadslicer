//! Parity with clipper-lib 6.4.2.2 for block 9a: `fixtures/df_clipper.json`
//! (`scripts/df-oracle/golden_clipper.ts`) records the JavaScript library's
//! PolyTree unions and differences (NonZero, StrictlySimple on and off) and
//! miter offsets over 700 generated path sets. The port must return the same
//! trees (contours, hole flags, children, the GetFirst/GetNext walk) and the
//! same offset paths, point for point.

use serde_json::{Value, json};

use support_gen_df::clipper::{
    ClipType, Clipper, ClipperOffset, EndType, IntPoint, JoinType, Path, PolyFillType, PolyTree,
    PolyType,
};

fn load() -> Value {
    let path = format!(
        "{}/tests/fixtures/df_clipper.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn paths(v: &Value) -> Vec<Path> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|p| {
            p.as_array()
                .unwrap()
                .iter()
                .map(|q| IntPoint::new(q["X"].as_f64().unwrap(), q["Y"].as_f64().unwrap()))
                .collect()
        })
        .collect()
}

fn wire_path(p: &[IntPoint]) -> Value {
    Value::Array(p.iter().map(|q| json!({ "X": q.x, "Y": q.y })).collect())
}

/// The oracle's flattening: nodes in preorder from the roots, and the walk.
fn flatten(tree: &PolyTree) -> Value {
    let mut nodes: Vec<Value> = Vec::new();
    let mut ids = vec![usize::MAX; tree.nodes.len()];
    fn visit(tree: &PolyTree, n: usize, nodes: &mut Vec<Value>, ids: &mut Vec<usize>) -> usize {
        let id = nodes.len();
        ids[n] = id;
        nodes.push(json!({ "contour": wire_path(&tree.nodes[n].polygon), "isHole": tree.is_hole(n), "childs": [] }));
        let childs: Vec<usize> = tree.nodes[n]
            .childs
            .iter()
            .map(|&c| visit(tree, c, nodes, ids))
            .collect();
        nodes[id]["childs"] = json!(childs);
        id
    }
    let roots: Vec<usize> = tree.nodes[0]
        .childs
        .iter()
        .map(|&c| visit(tree, c, &mut nodes, &mut ids))
        .collect();
    let mut walk = Vec::new();
    let mut node = tree.get_first();
    while let Some(n) = node {
        walk.push(ids[n]);
        node = tree.get_next(n);
    }
    json!({ "nodes": nodes, "roots": roots, "walk": walk })
}

fn assert_same(actual: &Value, expected: &Value, path: &str) {
    match (actual, expected) {
        (Value::Number(a), Value::Number(e)) => {
            assert!(
                a.as_f64() == e.as_f64(),
                "{path}: port {a} vs clipper-lib {e}"
            );
        }
        (Value::Object(a), Value::Object(e)) => {
            for (k, ev) in e {
                assert_same(&a[k], ev, &format!("{path}.{k}"));
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

#[test]
fn clipper_matches_clipper_lib() {
    let fixture = load();
    for (k, case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
        let path = format!("cases[{k}] {}", case["op"].as_str().unwrap());
        assert!(
            case.get("error").is_none(),
            "{path}: the fixture holds no throwing case"
        );
        let subject = paths(&case["subject"]);
        let clip = paths(&case["clip"]);
        match case["op"].as_str().unwrap() {
            "offset" => {
                let mut co = ClipperOffset::new(2.0, 2.0);
                co.add_paths(&subject, JoinType::Miter, EndType::ClosedPolygon);
                let mut solution = Vec::new();
                co.execute(&mut solution, case["delta"].as_f64().unwrap())
                    .unwrap();
                let actual = Value::Array(solution.iter().map(|p| wire_path(p)).collect());
                assert_same(&actual, &case["result"], &path);
            }
            op => {
                let mut c = Clipper::new();
                c.strictly_simple = case["strictlySimple"].as_bool().unwrap();
                c.add_paths(&subject, PolyType::Subject, true).unwrap();
                if op == "difference" {
                    c.add_paths(&clip, PolyType::Clip, true).unwrap();
                }
                let mut tree = PolyTree::default();
                let clip_type = if op == "union" {
                    ClipType::Union
                } else {
                    ClipType::Difference
                };
                let ok = c
                    .execute_tree(
                        clip_type,
                        &mut tree,
                        PolyFillType::NonZero,
                        PolyFillType::NonZero,
                    )
                    .unwrap();
                assert_eq!(Value::Bool(ok), case["ok"], "{path}.ok");
                assert_same(&flatten(&tree), &case["tree"], &format!("{path}.tree"));
            }
        }
    }
}
