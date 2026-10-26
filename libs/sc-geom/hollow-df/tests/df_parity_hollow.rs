//! Parity with DragonFruit's own hollowing engine.
//!
//! `tests/fixtures/df_hollow.json` holds digests of DragonFruit's outputs
//! (its Rust crate, default features), written by
//! `scripts/df-oracle/run_hollow_harness.sh`. Each case below builds the same
//! input with the same options; voxels, reports and meshes must match bit for
//! bit (positions and indices hashed).
//!
//! The real-model case reads `.local/df-oracle/df_hollow_real.json` (a private
//! model, not committed) and runs with `SC_BENCH_STL` set:
//!
//! ```text
//! SC_BENCH_STL=C:/Users/dnaon/Downloads/617_repaired.stl \
//!   cargo test --release -p hollow-df --test df_parity_hollow -- --ignored --nocapture
//! ```

use hollow_df::hollowing::{
    DrainHoleSpec, HollowMode, HollowOptions, HollowOutcome, HollowSession, InfillMode,
    hollow_voxel,
};
use hollow_df::mesh::{IndexedMesh, Vec3};
use hollow_df::shapes::{cube, icosphere, merged, read_binary_stl};
use serde_json::{Value, json};

fn fnv(words: impl IntoIterator<Item = u32>) -> String {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for w in words {
        for b in w.to_le_bytes() {
            h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{h:016x}")
}

fn mesh_digest(mesh: &IndexedMesh) -> Value {
    json!({
        "vertices": mesh.positions.len(),
        "triangles": mesh.triangles.len(),
        "positions": fnv(mesh.positions.iter().flat_map(|p| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()])),
        "indices": fnv(mesh.triangles.iter().flat_map(|t| *t)),
    })
}

fn outcome_digest(name: &str, out: &HollowOutcome) -> Value {
    let r = &out.report;
    json!({
        "name": name,
        "report": {
            "voxel_resolution": r.voxel_resolution,
            "voxel_size_mm_bits": r.voxel_size_mm.to_bits(),
            "grid_size": r.grid_size,
            "source_triangle_count": r.source_triangle_count,
            "output_triangle_count": r.output_triangle_count,
            "occupied_voxels": r.occupied_voxels,
            "shell_voxels": r.shell_voxels,
            "removed_voxels": r.removed_voxels,
        },
        "removed_indices": { "len": out.removed_voxel_indices.len(), "hash": fnv(out.removed_voxel_indices.iter().copied()) },
        "removed_centers": fnv(out.removed_voxel_centers.iter().map(|v| v.to_bits())),
        "blocked_indices": out.blocked_voxel_indices,
        "blocked_centers": fnv(out.blocked_voxel_centers.iter().map(|v| v.to_bits())),
        "mesh": mesh_digest(&out.mesh),
        "cavity": out.cavity_mesh.as_ref().map(mesh_digest),
        "infill": out.preview_infill_mesh.as_ref().map(mesh_digest),
    })
}

fn preview(options: HollowOptions) -> HollowOptions {
    HollowOptions {
        preview_cavity_only: true,
        preview_surface_only: true,
        ..options
    }
}

/// The twins of the harness's cases, in its order.
fn port_cases() -> Vec<Value> {
    let mut cases = Vec::new();
    let sphere3 = icosphere(20.0, 3);
    let sphere2 = icosphere(20.0, 2);
    let base = HollowOptions::default();

    let session = HollowSession::new(sphere3.clone(), 64);
    cases.push(outcome_digest(
        "sphere64_preview_surface",
        &session.run(&preview(HollowOptions {
            voxel_resolution: 64,
            ..base.clone()
        })),
    ));
    cases.push(outcome_digest(
        "sphere64_session_final",
        &session.run(&HollowOptions {
            voxel_resolution: 64,
            ..base.clone()
        }),
    ));
    cases.push(outcome_digest(
        "sphere64_hollow_voxel_final",
        &hollow_voxel(
            sphere3.clone(),
            &HollowOptions {
                voxel_resolution: 64,
                ..base.clone()
            },
        ),
    ));
    cases.push(outcome_digest(
        "sphere48_cavity_only",
        &hollow_voxel(
            sphere2.clone(),
            &HollowOptions {
                voxel_resolution: 48,
                shell_thickness_mm: 1.2,
                preview_cavity_only: true,
                ..base.clone()
            },
        ),
    ));
    cases.push(outcome_digest(
        "cube_lattice_chamfer",
        &hollow_voxel(
            cube(0.0, 10.0, false),
            &HollowOptions {
                mode: HollowMode::Infill,
                voxel_resolution: 64,
                shell_thickness_mm: 1.6,
                smooth_internal_surfaces: false,
                internal_chamfer_passes: 2,
                ..base.clone()
            },
        ),
    ));
    cases.push(outcome_digest(
        "cube_pillar_preview",
        &HollowSession::new(cube(0.0, 10.0, false), 56).run(&HollowOptions {
            mode: HollowMode::Infill,
            infill_mode: InfillMode::Pillar,
            voxel_resolution: 56,
            shell_thickness_mm: 1.5,
            preview_cavity_only: true,
            ..base.clone()
        }),
    ));
    cases.push(outcome_digest(
        "nested_box",
        &hollow_voxel(
            merged(&[cube(0.0, 30.0, false), cube(10.0, 20.0, true)]),
            &HollowOptions {
                voxel_resolution: 40,
                shell_thickness_mm: 1.0,
                ..base.clone()
            },
        ),
    ));
    cases.push(outcome_digest(
        "rotated_sphere_preview",
        &hollow_voxel(
            icosphere(15.0, 2),
            &preview(HollowOptions {
                voxel_resolution: 50,
                rotation_quat: [0.0, 0.382_683_43, 0.0, 0.923_879_5],
                ..base.clone()
            }),
        ),
    ));
    let session48 = HollowSession::new(sphere2.clone(), 48);
    let unblocked = session48.run(&preview(HollowOptions {
        voxel_resolution: 48,
        ..base.clone()
    }));
    let mut blocked: Vec<usize> = unblocked
        .removed_voxel_indices
        .iter()
        .take(50)
        .map(|&i| i as usize)
        .collect();
    blocked.push(0);
    blocked.push(1_000_000_000);
    cases.push(outcome_digest(
        "blocked_sphere",
        &session48.run(&preview(HollowOptions {
            voxel_resolution: 48,
            blocked_voxel_indices: blocked,
            ..base.clone()
        })),
    ));
    cases.push(outcome_digest(
        "drain_corridor",
        &hollow_voxel(
            sphere3.clone(),
            &HollowOptions {
                voxel_resolution: 64,
                drain_holes: vec![DrainHoleSpec {
                    center_norm: [0.5, 0.5, 0.0],
                    radius_mm: 2.0,
                    direction: None,
                    length_mm: None,
                }],
                ..base.clone()
            },
        ),
    ));
    cases.push(outcome_digest(
        "chamfer_thick_wall",
        &hollow_voxel(
            sphere2.clone(),
            &HollowOptions {
                voxel_resolution: 40,
                shell_thickness_mm: 3.0,
                smooth_internal_surfaces: false,
                internal_chamfer_passes: 2,
                ..base.clone()
            },
        ),
    ));
    cases
}

fn compare(expected: &[Value], actual: &[Value]) {
    assert_eq!(expected.len(), actual.len(), "case count");
    let mut failures = Vec::new();
    for (e, a) in expected.iter().zip(actual) {
        let name = e["name"].as_str().unwrap_or("?");
        assert_eq!(e["name"], a["name"], "case order");
        for field in [
            "report",
            "removed_indices",
            "removed_centers",
            "blocked_indices",
            "blocked_centers",
            "mesh",
            "cavity",
            "infill",
        ] {
            if e[field] != a[field] {
                failures.push(format!(
                    "{name}.{field}:\n  DragonFruit {}\n  port        {}",
                    e[field], a[field]
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn matches_dragonfruit_on_the_fixture_cases() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/df_hollow.json")).expect("fixture JSON");
    let expected = fixture["cases"].as_array().expect("cases");
    compare(expected, &port_cases());
}

/// Centred on its bbox and scaled to `height` mm along Z (the harness's `posed`).
fn posed(mut mesh: IndexedMesh, height: f32) -> IndexedMesh {
    let (mut min, mut max) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in &mesh.positions {
        let p = [p.x, p.y, p.z];
        for k in 0..3 {
            min[k] = min[k].min(p[k]);
            max[k] = max[k].max(p[k]);
        }
    }
    let centre: Vec<f32> = (0..3).map(|k| (min[k] + max[k]) / 2.0).collect();
    let scale = height / (max[2] - min[2]);
    for p in &mut mesh.positions {
        *p = Vec3::new(
            (p.x - centre[0]) * scale,
            (p.y - centre[1]) * scale,
            (p.z - centre[2]) * scale,
        );
    }
    mesh
}

#[test]
#[ignore = "needs the private real model and its DragonFruit fixture in .local/"]
fn matches_dragonfruit_on_the_real_model() {
    let Ok(stl) = std::env::var("SC_BENCH_STL") else {
        eprintln!("set SC_BENCH_STL");
        return;
    };
    let fixture_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../.local/df-oracle/df_hollow_real.json"
    );
    let Ok(text) = std::fs::read_to_string(fixture_path) else {
        eprintln!(
            "no {fixture_path}: run scripts/df-oracle/run_hollow_harness.sh with SC_BENCH_STL"
        );
        return;
    };
    let fixture: Value = serde_json::from_str(&text).unwrap();
    let height = fixture["height_mm"].as_f64().unwrap() as f32;
    let res = fixture["resolution"].as_u64().unwrap() as u16;
    let mesh = posed(
        read_binary_stl(&std::fs::read(stl).unwrap()).unwrap(),
        height,
    );
    let base = HollowOptions::default();
    let t = std::time::Instant::now();
    let session = HollowSession::new(mesh, res);
    let session_ms = t.elapsed().as_secs_f64() * 1e3;
    let t = std::time::Instant::now();
    let surface = session.run(&preview(HollowOptions {
        voxel_resolution: res,
        ..base.clone()
    }));
    let preview_ms = t.elapsed().as_secs_f64() * 1e3;
    let t = std::time::Instant::now();
    let final_mesh = session.run(&HollowOptions {
        voxel_resolution: res,
        ..base.clone()
    });
    let final_ms = t.elapsed().as_secs_f64() * 1e3;
    eprintln!(
        "port: session {session_ms:.0} ms, preview {preview_ms:.0} ms, final {final_ms:.0} ms; DragonFruit: {}",
        fixture["timings_ms"]
    );
    let actual = [
        outcome_digest("real_preview_surface", &surface),
        outcome_digest("real_session_final", &final_mesh),
    ];
    compare(fixture["cases"].as_array().unwrap(), &actual);
}
