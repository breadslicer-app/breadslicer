//! Parity with DragonFruit for block 6a: `fixtures/df_autosupport_a.json`
//! records DragonFruit's own answers (`scripts/df-oracle/golden_autosupport_a.ts`,
//! DragonFruit f331fc55) for the auto-support passes: parameter sizing, the
//! load budget, the contact tip cap, coverage and gap fill, candidate
//! generation and deduplication, minima reinforcement, pose stability,
//! stabilization anchors and the density grid, over six meshes (posed ones
//! included, two with support-blocker masks). Calls that touch the pooled
//! distance field replay in DragonFruit's order. Bit for bit.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use support_gen_df::supports::auto_support::candidate_generation::*;
use support_gen_df::supports::auto_support::contact_tip_cap::*;
use support_gen_df::supports::auto_support::coverage::*;
use support_gen_df::supports::auto_support::grid_placement::*;
use support_gen_df::supports::auto_support::load_budget::*;
use support_gen_df::supports::auto_support::minima_reinforcement::*;
use support_gen_df::supports::auto_support::parameter_sizing::*;
use support_gen_df::supports::auto_support::pose_stability::*;
use support_gen_df::supports::auto_support::settings::AutoSupportSettings;
use support_gen_df::supports::auto_support::stabilization::*;
use support_gen_df::supports::auto_support::types::{AutoSupportMesh, CandidatePoint};
use support_gen_df::supports::model_mesh::ModelMesh;
use support_gen_df::supports::placement_logic::pathfinding::sdf_cache::SdfCache;
use support_gen_df::supports::settings::types::SupportSettings;
use support_gen_df::supports::types::Vec3;
use support_gen_df::three::Matrix4;
use support_gen_df::three_mesh_bvh::{BvhMesh, MeshBvh, Side};
use support_gen_df::volume_analysis::islands::types::DetectedIsland;

fn fixture() -> Value {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/df_autosupport_a.json"
    ))
    .unwrap();
    serde_json::from_str(&text).unwrap()
}

fn read<T: DeserializeOwned>(value: &Value) -> T {
    serde_json::from_value(value.clone()).unwrap_or_else(|e| panic!("{e}: {value}"))
}

fn field<T: DeserializeOwned>(value: &Value, key: &str) -> Option<T> {
    value.get(key).filter(|v| !v.is_null()).map(read)
}

fn num(value: &Value) -> f64 {
    match value {
        Value::String(s) if s == "Infinity" => f64::INFINITY,
        Value::String(s) if s == "-Infinity" => f64::NEG_INFINITY,
        _ => value
            .as_f64()
            .unwrap_or_else(|| panic!("not a number: {value}")),
    }
}

/// Compares two JSON graphs exactly (numbers as f64 bits). DragonFruit's
/// ±Infinity arrive as strings; serde_json writes a non-finite f64 as null, so
/// the fields that can be infinite are also checked directly by the caller.
fn assert_same(actual: &Value, expected: &Value, path: &str) {
    match (actual, expected) {
        (Value::Number(a), Value::Number(e)) => {
            let (a, e) = (a.as_f64().unwrap(), e.as_f64().unwrap());
            assert!(a == e, "{path}: port {a:?} vs DragonFruit {e:?}");
        }
        (Value::Null, Value::String(e)) if e.ends_with("Infinity") => {}
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

fn check_f64(path: &str, actual: f64, expected: &Value) {
    let e = num(expected);
    assert!(
        actual == e || (actual.is_nan() && e.is_nan()),
        "{path}: port {actual:?} vs DragonFruit {e:?}"
    );
}

struct Scene {
    bvh: Arc<MeshBvh>,
    matrix: Matrix4,
    side: Side,
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
            let side = match m["side"].as_str().unwrap() {
                "front" => Side::Front,
                "back" => Side::Back,
                _ => Side::Double,
            };
            let scene = Scene {
                bvh: Arc::new(MeshBvh::new(positions, index)),
                matrix: Matrix4::from_column_major(e.try_into().unwrap()),
                side,
            };
            (name.clone(), scene)
        })
        .collect()
}

fn islands(value: &Value) -> Vec<DetectedIsland> {
    read(value)
}

fn hit_json(hit: Option<SurfaceHit>) -> Value {
    match hit {
        None => Value::Null,
        Some(SurfaceHit {
            z,
            face_index: None,
        }) => json!({ "z": z }),
        Some(SurfaceHit {
            z,
            face_index: Some(f),
        }) => json!({ "z": z, "faceIndex": f }),
    }
}

fn point_json(p: &SurfacePoint) -> Value {
    match p.face_index {
        Some(f) => json!({ "x": p.x, "y": p.y, "z": p.z, "faceIndex": f }),
        None => json!({ "x": p.x, "y": p.y, "z": p.z }),
    }
}

#[test]
fn auto_support_passes_match_dragonfruit() {
    let fixture = fixture();
    let scenes = scenes(&fixture);
    let blocked: HashMap<String, HashSet<usize>> = fixture["blocked"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), read::<Vec<usize>>(v).into_iter().collect()))
        .collect();
    let bands = SizingBands::factory();
    // DragonFruit's pool: one field per mesh, built on first use.
    let mut pool: HashMap<String, SdfCache> = HashMap::new();
    let mut counts: HashMap<String, usize> = HashMap::new();

    for (i, c) in fixture["ops"].as_array().unwrap().iter().enumerate() {
        let op = c["op"].as_str().unwrap();
        *counts.entry(op.to_owned()).or_default() += 1;
        let path = format!("ops[{i}].{op}");
        let name = c["scene"].as_str().unwrap();
        let scene = &scenes[name];
        let rays = BvhMesh {
            bvh: &scene.bvh,
            matrix_world: &scene.matrix,
        };
        let settings: SupportSettings = field(c, "settings").unwrap_or_default();
        let auto: AutoSupportSettings = field(c, "auto").unwrap_or_default();
        let input = &c["input"];
        let expected = &c["result"];
        let uses_field = matches!(op, "tipCap" | "gen" | "grid");
        if uses_field {
            pool.entry(name.to_owned())
                .or_insert_with(|| SdfCache::new(scene.bvh.clone(), scene.matrix, Some(0.5)));
        }
        let sdf = pool.get(name);
        let auto_mesh = |model_id: Option<&str>| {
            sdf.map(|sdf| AutoSupportMesh {
                mesh: ModelMesh { field: sdf, rays },
                side: scene.side,
                blocked: model_id.and_then(|m| blocked.get(m)),
            })
        };

        match op {
            "size" => {
                let candidate: CandidatePoint = read(&input["candidate"]);
                let ctx: Option<ModelSizingContext> = field(input, "ctx");
                let size_scale = num(&input["sizeScale"]);
                check(
                    &path,
                    json!({
                        "overrides": size_parameters(&candidate, size_scale, ctx.as_ref(), &settings, &bands),
                        "factors": model_sizing_factors(ctx.as_ref(), &settings),
                        "preset": preset_for_area(candidate.island_area_mm2),
                    }),
                    expected,
                );
            }
            "tipCap" => {
                let mut cs: Vec<CandidatePoint> = read(&input["candidates"]);
                let sdf = sdf.unwrap();
                let widths: Vec<f64> = cs
                    .iter()
                    .map(|c| {
                        local_free_width_mm(
                            sdf,
                            c.tip_pos.x,
                            c.tip_pos.y,
                            c.tip_pos.z,
                            c.tip_normal,
                        )
                    })
                    .collect();
                apply_contact_tip_caps(&mut cs, Some(sdf), &settings, &bands);
                check(
                    &path,
                    json!({ "widths": widths, "candidates": cs }),
                    expected,
                );
            }
            "coverage" => {
                let isl = islands(&input["islands"]);
                let tips: Vec<Vec3> = read(&input["tips"]);
                let radius = field::<f64>(input, "radius");
                let r = radius.unwrap_or(TIP_COVERAGE_RADIUS_MM);
                check(
                    &path,
                    json!({
                        "radii": isl.iter().map(|i| coverage_radius_for_area(i.area_mm2.unwrap_or(0.0), TIP_COVERAGE_RADIUS_MM)).collect::<Vec<_>>(),
                        "coverage": isl.iter().map(|i| compute_region_coverage(i, &tips, r)).collect::<Vec<_>>(),
                        "clusters": isl.iter().map(|i| find_uncovered_clusters(i, &tips, r, MIN_GAP_CLUSTER_MM2)).collect::<Vec<_>>(),
                        "gap": build_gap_fill_candidates(&isl, &auto, &tips),
                    }),
                    expected,
                );
            }
            "gen" => {
                let isl = islands(&input["islands"]);
                let with_mesh = input["withMesh"].as_bool().unwrap();
                let model_id: Option<String> = field(input, "modelId");
                let mesh = if with_mesh {
                    auto_mesh(model_id.as_deref())
                } else {
                    None
                };
                check(
                    &path,
                    json!({
                        "perIsland": isl.iter().map(|i| candidates_from_island(i, &bands)).collect::<Vec<_>>(),
                        "candidates": generate_candidates(&isl, &auto, mesh, &settings, &bands),
                    }),
                    expected,
                );
            }
            "dedup" => {
                let cs: Vec<CandidatePoint> = read(&input["candidates"]);
                check(&path, deduplicate_candidates(&cs, &auto), expected);
            }
            "minima" => {
                let isl = islands(&input["islands"]);
                check(
                    &path,
                    compute_minima_reinforcement_points(&isl, rays, scene.side, &auto),
                    expected,
                );
            }
            "pose" => {
                let positions = posed_positions(scene.bvh.positions(), &scene.matrix);
                let index = scene.bvh.index();
                let details: Option<Vec<f64>> = input["terms"].get("details").map(read);
                let terms_blocked: Option<HashSet<usize>> = input["terms"]
                    .get("blocked")
                    .filter(|b| !b.is_null())
                    .map(|b| read::<Vec<usize>>(b).into_iter().collect());
                let empty: Vec<f64> = Vec::new();
                let terms = details.as_ref().map(|d| SupportTerms {
                    normals: &empty,
                    areas: &empty,
                    details: d,
                    blocked: terms_blocked.as_ref(),
                    threshold: num(&input["terms"]["threshold"]),
                    cup_cos: num(&input["terms"]["cupCos"]),
                });
                let s = measure_pose_stability(
                    &positions,
                    Some(index),
                    num(&input["rx"]),
                    num(&input["ry"]),
                    terms.as_ref(),
                    input["hasRaft"].as_bool().unwrap(),
                );
                let posed: Vec<f64> = positions.iter().take(30).map(|&v| f64::from(v)).collect();
                check(&format!("{path}.posed"), &posed, &expected["posed"]);
                check(&format!("{path}.stability"), s, &expected["stability"]);
                check_f64(
                    &format!("{path}.margin"),
                    s.margin_mm,
                    &expected["stability"]["marginMm"],
                );
                check_f64(
                    &format!("{path}.adhesion"),
                    s.adhesion_ratio,
                    &expected["stability"]["adhesionRatio"],
                );
                check(
                    &format!("{path}.verdicts"),
                    json!([
                        is_slender_part(s.height_mm, s.volume_mm3, s.bearing_area_mm2),
                        needs_topple_coverage(&s)
                    ]),
                    &json!([expected["slender"], expected["topple"]]),
                );
            }
            "grid" => {
                let isl = islands(&input["islands"]);
                let with_mesh = input["withMesh"].as_bool().unwrap();
                let model_id: Option<String> = field(input, "modelId");
                let mesh = if with_mesh {
                    auto_mesh(model_id.as_deref())
                } else {
                    None
                };
                let result = generate_grid_candidates(
                    &isl,
                    &auto,
                    mesh,
                    input["slender"].as_bool().unwrap(),
                    num(&input["thickness"]),
                    &settings,
                    &bands,
                );
                check(&path, result, expected);
            }
            "gridParts" => {
                let isl: DetectedIsland = read(&input["island"]);
                let spacing = num(&input["spacing"]);
                let probes: Vec<Value> = read(&input["probes"]);
                let pts = isl
                    .contact_voxels
                    .as_ref()
                    .map(|v| v.to_points())
                    .unwrap_or_default();
                let mut voxel_at = SurfaceSampler::voxel(&pts, spacing.max(1.0), isl.base_z);
                let mut tri_at = SurfaceSampler::triangle(&isl, Some(rays));
                let at = |s: &mut SurfaceSampler<'_>| {
                    probes
                        .iter()
                        .map(|p| hit_json(s.at(num(&p["x"]), num(&p["y"]))))
                        .collect::<Vec<_>>()
                };
                let voxel_samples = at(&mut voxel_at);
                let tri_samples = tri_at.as_mut().map(at);
                let ring2d = match tri_at.as_mut() {
                    Some(t) => sample_boundary_2d(isl.perimeter_loops.as_deref(), spacing, t),
                    None => {
                        sample_boundary_2d(isl.perimeter_loops.as_deref(), spacing, &mut voxel_at)
                    }
                };
                let band_tops = [
                    steep_flat_anchor_band_top(&isl, false),
                    steep_flat_anchor_band_top(&isl, true),
                ];
                for (k, top) in band_tops.iter().enumerate() {
                    check_f64(
                        &format!("{path}.bandTop[{k}]"),
                        *top,
                        &expected["bandTop"][k],
                    );
                }
                check(
                    &path,
                    json!({
                        "spacing": compute_region_spacing(&isl, &auto),
                        "useGrid": should_use_density_grid(&isl, &auto),
                        "mult": [steep_flat_spacing_multiplier(isl.steep_flat, spacing, 0.0), expected["mult"][1]],
                        "bandTop": expected["bandTop"],
                        "eroded": erode_footprint(&pts).len(),
                        "boundary": build_boundary_points(&pts, spacing, isl.base_z).iter().map(|p| json!({"x": p.x, "y": p.y, "z": p.z})).collect::<Vec<_>>(),
                        "voxelSamples": voxel_samples,
                        "triSamples": tri_samples,
                        "ring2d": ring2d.map(|r| r.iter().map(point_json).collect::<Vec<_>>()),
                        "perimeter": sample_perimeter_loops(isl.perimeter_loops.as_deref().unwrap_or(&[]), spacing, isl.base_z),
                    }),
                    expected,
                );
            }
            "budget" => {
                let isl = islands(&input["islands"]);
                let contacts: Vec<PlacedContact> = read(&input["contacts"]);
                let report = compute_load_budget(
                    &LoadBudgetInput {
                        islands: &isl,
                        contacts: &contacts,
                        area_per_support_mm2: num(&input["areaPerSupportMm2"]),
                        pose_drag_moment_mm3: field(input, "poseDragMomentMm3"),
                        topple_coverage_needed: input["toppleCoverageNeeded"].as_bool().unwrap(),
                    },
                    &bands,
                );
                check(&path, report, expected);
            }
            "stabilization" => {
                let anchors = compute_stabilization_anchors(
                    scene.bvh.positions(),
                    Some(scene.bvh.index()),
                    &scene.matrix,
                    input["hasRaft"].as_bool().unwrap(),
                );
                check(&path, anchors, expected);
            }
            other => panic!("unknown op {other}"),
        }
    }
    for op in [
        "size",
        "tipCap",
        "coverage",
        "gen",
        "dedup",
        "minima",
        "pose",
        "grid",
        "gridParts",
        "budget",
        "stabilization",
    ] {
        assert!(counts.get(op).copied().unwrap_or(0) > 0, "no {op} cases");
    }
}
