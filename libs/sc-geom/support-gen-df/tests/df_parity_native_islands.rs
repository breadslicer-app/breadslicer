//! Parity with DragonFruit for block 8a: `fixtures/df_native_islands.json`
//! records DragonFruit's own desktop island scanners (`.local/df-oracle/islands-harness`:
//! `src-tauri/src/{overhang,mesh_minima}.rs` compiled unchanged with its
//! `dragonfruit-islands` and `dragonfruit-mesh-core` crates, DragonFruit
//! f331fc55) on seven procedural scenes under four scan settings: the combined
//! scan (voxel islands snapped to the surface, mesh minima, overhang regions)
//! and the soup scan the frontend runs (overhang regions and the pose
//! stability report).
//!
//! DragonFruit returns voxel islands in std `HashMap` order and walks its
//! perimeter edges in one, both random per process. SoulCrafted fixes both
//! orders: voxel islands are compared as a set (their `v{n}` ids follow the
//! order), and the oracle walks perimeter edges in key order, as SoulCrafted
//! does (its `build_perimeter_loops` gets ordered local containers).
//! Everything else must be identical, in the IPC form the frontend reads.

use serde_json::Value;

use support_gen_df::native::mesh_core::mesh::IndexedMesh;
use support_gen_df::native::mesh_minima::scan_islands;
use support_gen_df::native::overhang::overhang_and_stability_from_soup;

fn fixture() -> Value {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/df_native_islands.json"
    ))
    .unwrap();
    serde_json::from_str(&text).unwrap()
}

/// Exact JSON equality.
fn assert_same(actual: &Value, expected: &Value, path: &str) {
    match (actual, expected) {
        (Value::Object(a), Value::Object(e)) => {
            let mut ak: Vec<&String> = a.keys().collect();
            let mut ek: Vec<&String> = e.keys().collect();
            ak.sort();
            ek.sort();
            assert_eq!(ak, ek, "{path}: fields differ");
            for (k, ev) in e {
                assert_same(&a[k], ev, &format!("{path}.{k}"));
            }
        }
        (Value::Array(a), Value::Array(e)) => {
            assert_eq!(a.len(), e.len(), "{path}: lengths differ");
            for (i, (a, e)) in a.iter().zip(e).enumerate() {
                assert_same(a, e, &format!("{path}[{i}]"));
            }
        }
        (Value::Number(a), Value::Number(e)) => {
            assert!(
                a.as_f64() == e.as_f64(),
                "{path}: port {a} vs DragonFruit {e}"
            );
        }
        _ => assert_eq!(actual, expected, "{path}"),
    }
}

/// Voxel islands as DragonFruit's set: sorted by span and contact, ids dropped
/// (and checked to be `v0..`).
fn voxel_set(v: &Value) -> Vec<Value> {
    let list = v.as_array().unwrap();
    for (i, island) in list.iter().enumerate() {
        assert_eq!(island["id"], Value::String(format!("v{i}")));
    }
    let mut out: Vec<Value> = list
        .iter()
        .map(|island| {
            let mut island = island.clone();
            island.as_object_mut().unwrap().remove("id");
            island
        })
        .collect();
    // By value, not by serialisation: the oracle's JSON keys are sorted.
    let key = |v: &Value| {
        let f = |x: &Value| x.as_f64().unwrap();
        [
            f(&v["layerSpan"][0]),
            f(&v["layerSpan"][1]),
            f(&v["contact"]["x"]),
            f(&v["contact"]["y"]),
            f(&v["contact"]["z"]),
            f(&v["areaMm2"]),
            f(&v["baseZ"]),
        ]
    };
    out.sort_by(|a, b| key(a).partial_cmp(&key(b)).unwrap());
    out
}

/// A value as DragonFruit's frontend receives it over IPC (serde_json text).
fn ipc<T: serde::Serialize>(value: &T) -> Value {
    serde_json::from_str(&serde_json::to_string(value).unwrap()).unwrap()
}

#[test]
fn native_island_scans_match_dragonfruit() {
    let fixture = fixture();
    let soups: std::collections::HashMap<String, Vec<f32>> = fixture["meshes"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, s)| {
            let soup: Vec<f64> = serde_json::from_value(s.clone()).unwrap();
            (name.clone(), soup.into_iter().map(|v| v as f32).collect())
        })
        .collect();
    for (r, run) in fixture["runs"].as_array().unwrap().iter().enumerate() {
        let path = format!("runs[{r}] {}", run["scene"].as_str().unwrap());
        let soup = &soups[run["scene"].as_str().unwrap()];
        let num = |k: &str| run[k].as_f64().unwrap();
        let mesh = IndexedMesh::from_triangle_soup(soup, 1e-5);
        let combined = scan_islands(
            &mesh,
            num("layerHeightMm"),
            num("pxMm"),
            num("supportBufferMm"),
            run["connectivity"].as_u64().unwrap() as u8,
            run["k"].as_u64().map(|k| k as usize),
            Some(num("angleDeg") as f32),
            Some(num("overhangPxMm") as f32),
        );
        let actual = ipc(&combined);
        let expected = run["combined"].clone();
        let (av, ev) = (
            voxel_set(&actual["voxelIslands"]),
            voxel_set(&expected["voxelIslands"]),
        );
        assert_same(
            &Value::Array(av),
            &Value::Array(ev),
            &format!("{path}.voxelIslands"),
        );
        assert_same(
            &actual["overhangIslands"],
            &expected["overhangIslands"],
            &format!("{path}.overhangIslands"),
        );
        assert_same(
            &actual["minimaIslands"],
            &expected["minimaIslands"],
            &format!("{path}.minimaIslands"),
        );

        let (regions, stability) = overhang_and_stability_from_soup(
            soup,
            num("angleDeg") as f32,
            num("overhangPxMm") as f32,
            run["hasRaft"].as_bool().unwrap(),
        );
        let soup_scan = ipc(&support_gen_df::native::overhang::OverhangScan {
            regions,
            stability,
            steep_regions: Vec::new(),
        });
        assert_same(&soup_scan, &run["soupScan"], &format!("{path}.soupScan"));
    }
}

/// SoulCrafted's pipeline skips the scan's own overhang classification; the
/// voxel islands and minima must not change for it.
#[test]
fn skipping_the_scans_overhangs_leaves_islands_and_minima_untouched() {
    use support_gen_df::native::mesh_minima::scan_islands_without_overhangs;
    let fixture = fixture();
    for (name, s) in fixture["meshes"].as_object().unwrap() {
        let soup: Vec<f64> = serde_json::from_value(s.clone()).unwrap();
        let soup: Vec<f32> = soup.into_iter().map(|v| v as f32).collect();
        let mesh = IndexedMesh::from_triangle_soup(&soup, 1e-5);
        let full = scan_islands(&mesh, 0.05, 0.1, 0.25, 4, Some(2), Some(45.0), Some(0.25));
        let lean = scan_islands_without_overhangs(&mesh, 0.05, 0.1, 0.25, 4, Some(2));
        assert_eq!(
            ipc(&full.voxel_islands),
            ipc(&lean.voxel_islands),
            "{name}: voxel islands"
        );
        assert_eq!(
            ipc(&full.minima_islands),
            ipc(&lean.minima_islands),
            "{name}: minima"
        );
        assert!(lean.overhang_islands.is_empty());
    }
}
