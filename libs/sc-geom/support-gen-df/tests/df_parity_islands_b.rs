//! Parity with DragonFruit for block 8b: `fixtures/df_islands_b.json` records
//! `useIslands`' derivation (`scripts/df-oracle/golden_islands_b.ts`, DragonFruit
//! f331fc55) over every native scan in `df_native_islands.json`, three panel
//! settings and support-tip sets each: the annotated islands, the filtered list
//! auto-support runs on, the classification statistics and the pose verdict.
//! The port derives from the same scan results: the combined scan read back
//! from its IPC form (voxel islands in DragonFruit's order), the soup scan
//! recomputed by the port (identical to DragonFruit's, block 8a).
//!
//! Footprints are compared by count and an FNV-1a over their `f32` bits; the
//! filtered list by ids (the filter keeps the annotated order); `null` and an
//! absent field are the same.

use serde_json::Value;

use support_gen_df::native::mesh_minima::CombinedIslandScanResult;
use support_gen_df::native::overhang::{OverhangScan, overhang_and_stability_from_soup};
use support_gen_df::supports::types::Vec3;
use support_gen_df::volume_analysis::islands::filtering::{IslandFilterToggles, TipInfo};
use support_gen_df::volume_analysis::islands::types::DetectedIsland;
use support_gen_df::volume_analysis::islands::use_islands::{
    IslandPanelSettings, consolidate_voxel_islands, derive_islands, determine_contour_threshold,
};
use support_gen_df::volume_analysis::islands::voxel_footprint::VoxelFootprint;

fn load(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Drop `null` fields, recursively.
fn strip_nulls(v: &mut Value) {
    match v {
        Value::Object(map) => {
            map.retain(|_, x| !x.is_null());
            map.values_mut().for_each(strip_nulls);
        }
        Value::Array(items) => items.iter_mut().for_each(strip_nulls),
        _ => {}
    }
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
        _ => assert_eq!(actual, expected, "{path}"),
    }
}

/// The oracle's footprint digest: count and FNV-1a over the f32 bits.
fn digest(f: &VoxelFootprint) -> Value {
    let mut h: u32 = 2166136261;
    let mut eat = |values: &[f32]| {
        for v in values {
            h ^= v.to_bits();
            h = h.wrapping_mul(16777619);
        }
    };
    eat(&f.xy[..f.count * 2]);
    if let Some(z) = &f.z {
        eat(&z[..f.count]);
    }
    serde_json::json!({ "count": f.count, "withZ": f.z.is_some(), "fnv": h })
}

fn wire(island: &DetectedIsland) -> Value {
    let mut v = serde_json::to_value(island).unwrap();
    if let Some(f) = &island.contact_voxels {
        v["contactVoxels"] = digest(f);
    }
    if let Some(members) = &island.members {
        v["members"] = Value::Array(members.iter().map(wire).collect());
    }
    v
}

#[test]
fn island_derivation_matches_dragonfruit() {
    let native = load("df_native_islands.json");
    let fixture = load("df_islands_b.json");
    for (c, case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
        let run = &native["runs"][case["run"].as_u64().unwrap() as usize];
        let path = format!(
            "cases[{c}] {} run {}",
            run["scene"].as_str().unwrap(),
            case["run"]
        );
        let combined: CombinedIslandScanResult =
            serde_json::from_value(run["combined"].clone()).unwrap();
        // The soup scan straight from the port, which matches DragonFruit's
        // exactly (block 8a): its report can hold a NaN that crosses IPC as null.
        let soup: Vec<f64> =
            serde_json::from_value(native["meshes"][run["scene"].as_str().unwrap()].clone())
                .unwrap();
        let soup: Vec<f32> = soup.into_iter().map(|v| v as f32).collect();
        let (regions, stability) = overhang_and_stability_from_soup(
            &soup,
            run["angleDeg"].as_f64().unwrap() as f32,
            run["overhangPxMm"].as_f64().unwrap() as f32,
            run["hasRaft"].as_bool().unwrap(),
        );
        let soup_scan = OverhangScan { regions, stability };
        let s = &case["settings"];
        let num = |k: &str| s[k].as_f64().unwrap();
        let settings = IslandPanelSettings {
            px_mm: num("pxMm"),
            min_area_mm2: num("minAreaMm2"),
            consolidate_voxel: s["consolidateVoxel"].as_bool().unwrap(),
            consolidation_distance: num("consolidationDistance"),
            enable_contour_regions: s["enableContourRegions"].as_bool().unwrap(),
            max_contour_regions: s["maxContourRegions"].as_u64().unwrap() as usize,
            area_per_support: num("areaPerSupport"),
            layer_height_mm: run["layerHeightMm"].as_f64().unwrap(),
            plate_z: num("plateZ"),
            filter_toggles: IslandFilterToggles {
                show_already_supported: s["filterToggles"]["showAlreadySupported"]
                    .as_bool()
                    .unwrap(),
                show_plate_contact: s["filterToggles"]["showPlateContact"].as_bool().unwrap(),
            },
        };
        let tips: Vec<TipInfo> = case["supportTips"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| TipInfo {
                pos: Vec3::new(
                    t["pos"]["x"].as_f64().unwrap(),
                    t["pos"]["y"].as_f64().unwrap(),
                    t["pos"]["z"].as_f64().unwrap(),
                ),
                diameter_mm: t["diameterMm"].as_f64().unwrap(),
            })
            .collect();

        let derived = derive_islands(&combined, &soup_scan, &tips, &settings);
        let mut actual = serde_json::json!({
            "filtered": derived.filtered.iter().map(|i| i.id.clone()).collect::<Vec<_>>(),
            "annotated": derived.annotated.iter().map(wire).collect::<Vec<_>>(),
            "stats": derived.stats,
            "toppleCoverage": derived.topple_coverage,
            "dragTotalMm3": derived.drag_total_mm3,
        });
        let mut expected = case["result"].clone();
        strip_nulls(&mut actual);
        strip_nulls(&mut expected);
        assert_same(&actual, &expected, &path);
    }
}

/// `consolidateVoxelIslands` and `determineContourThreshold` on synthetic
/// islands with footprints, which the native chain never produces.
#[test]
fn consolidation_and_contours_match_dragonfruit() {
    let fixture = load("df_islands_b.json");
    for (c, case) in fixture["direct"].as_array().unwrap().iter().enumerate() {
        let islands: Vec<DetectedIsland> = serde_json::from_value(case["islands"].clone()).unwrap();
        let px = case["px"].as_f64().unwrap();
        let eps = case["eps"].as_f64().unwrap();
        let max_contour = case["maxContour"].as_u64().unwrap() as usize;
        let mut actual = Value::Array(
            consolidate_voxel_islands(&islands, eps, px)
                .iter()
                .map(wire)
                .collect(),
        );
        let mut expected = case["consolidated"].clone();
        strip_nulls(&mut actual);
        strip_nulls(&mut expected);
        assert_same(&actual, &expected, &format!("direct[{c}].consolidated"));
        let mut contoured: Vec<String> = determine_contour_threshold(&islands, max_contour)
            .into_iter()
            .collect();
        // JavaScript's default sort: UTF-16 code units.
        contoured.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
        let expected: Vec<String> = serde_json::from_value(case["contoured"].clone()).unwrap();
        assert_eq!(contoured, expected, "direct[{c}].contoured");
    }
}
