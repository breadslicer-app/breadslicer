//! Parity with DragonFruit for block 10, the support tessellation the slicer
//! rasterises: `fixtures/df_support_triangles.json`
//! (`scripts/df-oracle/golden_support_triangles.ts`) runs
//! `buildSupportAndRaftWorldTriangles` over the support states of blocks 8c
//! and 7 (some with curved shafts and braces), varying visibility, tip
//! shrink, the global raft and the plate clearance. The port must emit the
//! same triangle soup: same length, same FNV-1a over the `f32` bits, and the
//! same first 90 floats.

use std::collections::HashSet;

use serde_json::Value;

use support_gen_df::features::slicing::support_world_triangles::{
    SupportSliceInput, TriangleSink, append_contact_cone_primitive, append_contact_disk_primitive,
    build_support_and_raft_world_triangles,
};
use support_gen_df::supports::rafts::crenelated::geometry::model_plate_footprint::{
    ModelTransform, PlateFootprintSource,
};
use support_gen_df::supports::rafts::crenelated::raft_types::{RaftBottomMode, RaftSettings};
use support_gen_df::supports::support_primitives::contact_cone::types::ContactCone;
use support_gen_df::supports::types::ContactDisk;
use support_gen_df::supports::types::SupportCollections;
use support_gen_df::three::Vec3;
use support_gen_df::three::buffer_geometry::BufferGeometry;

fn load(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn fnv(values: &[f32]) -> u32 {
    let mut h: u32 = 2166136261;
    for v in values {
        h ^= v.to_bits();
        h = h.wrapping_mul(16777619);
    }
    h
}

#[test]
fn support_triangles_match_dragonfruit() {
    let fixture = load("df_support_triangles.json");
    let islands_c = load("df_islands_c.json");
    let autobrace = load("df_autobrace.json");
    let native = load("df_native_islands.json");
    let mut curved = 0;
    for (k, case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
        let path = format!(
            "cases[{k}] {} {}",
            case["from"].as_str().unwrap(),
            case["scene"].as_str().unwrap()
        );
        assert!(
            case.get("error").is_none(),
            "{path}: the fixture holds no throwing case"
        );
        let index = case["index"].as_u64().unwrap() as usize;
        let state_json = match case.get("state") {
            Some(s) => {
                curved += 1;
                s.clone()
            }
            None if case["from"] == "islands_c" => {
                islands_c["runs"][index]["result"]["support"].clone()
            }
            None => autobrace["runs"][index]["result"]["support"].clone(),
        };
        let state: SupportCollections = serde_json::from_value(state_json).unwrap();
        let visible: HashSet<String> = case["visible"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        let raft: RaftSettings = serde_json::from_value(case["raft"].clone()).unwrap();
        // Line rafts and their roots' feet are drawn our own way (#518).
        if raft.bottom_mode == RaftBottomMode::Line {
            continue;
        }
        let soup_geometry = case["clearance"].as_bool().unwrap().then(|| {
            let soup: Vec<f64> =
                serde_json::from_value(native["meshes"][case["scene"].as_str().unwrap()].clone())
                    .unwrap();
            BufferGeometry {
                position: Some(soup.iter().map(|&v| v as f32).collect()),
                ..Default::default()
            }
        });
        let models: Vec<PlateFootprintSource<'_>> = soup_geometry
            .iter()
            .map(|g| PlateFootprintSource {
                geometry: g,
                center: Vec3::ZERO,
                transform: ModelTransform {
                    position: Vec3::ZERO,
                    rotation: Vec3::ZERO,
                    scale: Vec3::new(1.0, 1.0, 1.0),
                },
                visible: Some(true),
            })
            .collect();
        let soup = build_support_and_raft_world_triangles(&SupportSliceInput {
            state: &state,
            visible_model_ids: &visible,
            raft: &raft,
            plate_clearance_models: &models,
            support_tip_shrink_percent: case["shrink"].as_f64().unwrap(),
            tip_penetration_mm: 0.0,
            tip_drawing: Default::default(),
            shaft_style: Default::default(),
            robust_raft_geometry: false,
            draw_rafts: true,
        })
        .unwrap();
        let expected = &case["triangles"];
        let head: Vec<f64> = serde_json::from_value(expected["head"].clone()).unwrap();
        let got_head: Vec<f64> = soup.iter().take(90).map(|&v| v as f64).collect();
        assert_eq!(got_head, head, "{path}: first floats");
        assert_eq!(
            soup.len() as u64,
            expected["len"].as_u64().unwrap(),
            "{path}: float count"
        );
        assert_eq!(
            fnv(&soup) as u64,
            expected["fnv"].as_u64().unwrap(),
            "{path}: digest"
        );
    }
    assert!(curved > 10, "curved shafts are exercised ({curved})");
}

#[test]
fn contact_primitives_with_tip_penetration_match_dragonfruit() {
    let fixture = load("df_support_triangles.json");
    let mut penetrating = 0;
    for (k, case) in fixture["primitives"].as_array().unwrap().iter().enumerate() {
        let path = format!("primitives[{k}]");
        let (segments, penetration, scale) = (
            case["segments"].as_u64().unwrap() as u32,
            case["penetration"].as_f64().unwrap(),
            case["scale"].as_f64().unwrap(),
        );
        if penetration > 0.0 {
            penetrating += 1;
        }
        let mut soup = TriangleSink::default();
        if case["disk"].as_bool().unwrap() {
            let disk: ContactDisk = serde_json::from_value(case["contact"].clone()).unwrap();
            append_contact_disk_primitive(&mut soup, &disk, segments, penetration, scale);
        } else {
            let cone: ContactCone = serde_json::from_value(case["contact"].clone()).unwrap();
            append_contact_cone_primitive(&mut soup, &cone, segments, penetration, scale);
        }
        let soup = soup.floats;
        let expected = &case["triangles"];
        let head: Vec<f64> = serde_json::from_value(expected["head"].clone()).unwrap();
        let got: Vec<f64> = soup.iter().take(90).map(|&v| v as f64).collect();
        assert_eq!(got, head, "{path}: first floats");
        assert_eq!(
            soup.len() as u64,
            expected["len"].as_u64().unwrap(),
            "{path}: float count"
        );
        assert_eq!(
            fnv(&soup) as u64,
            expected["fnv"].as_u64().unwrap(),
            "{path}: digest"
        );
    }
    assert!(
        penetrating > 50,
        "penetrating tips are exercised ({penetrating})"
    );
}
