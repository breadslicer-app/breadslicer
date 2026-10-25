//! SC-78: the `pipeline` facade. Over scenes built from the block-8a meshes it
//! must give exactly what the stage-by-stage calls the parity tests use give
//! (scan, derivation, plan with bracing, export rafts), report its stages in
//! order, and stop with `Cancelled` and no output at every stage boundary.

use std::cell::RefCell;
use std::sync::Arc;

use serde_json::{Value, json};

use support_gen_df::native::mesh_core::mesh::IndexedMesh;
use support_gen_df::native::mesh_minima::scan_islands;
use support_gen_df::native::overhang::{OverhangScan, overhang_and_stability_from_soup};
use support_gen_df::pipeline::{
    CancelToken, ModelInput, PipelineError, PipelineSettings, Stage, build_rafts,
    generate_supports, world_soup,
};
use support_gen_df::supports::auto_bracing::mesh_clearance::{BraceMesh, BraceMeshes};
use support_gen_df::supports::auto_support::auto_place::helpers::{PlanCtx, PlanMesh};
use support_gen_df::supports::auto_support::auto_place::plan::{
    PlanInput, compute_auto_support_plan,
};
use support_gen_df::supports::auto_support::parameter_sizing::SizingBands;
use support_gen_df::supports::placement_logic::pathfinding::sdf_cache::SdfCache;
use support_gen_df::supports::rafts::crenelated::geometry::model_plate_footprint::ModelTransform;
use support_gen_df::supports::rafts::crenelated::raft_export::RaftGroup;
use support_gen_df::supports::rafts::crenelated::raft_types::{
    DEFAULT_RAFT_SETTINGS, RaftBottomMode, RaftSettings,
};
use support_gen_df::supports::types::SupportState;
use support_gen_df::three::buffer_geometry::BufferGeometry;
use support_gen_df::three::{Matrix4, Vec3};
use support_gen_df::three_mesh_bvh::{MeshBvh, Side};
use support_gen_df::volume_analysis::islands::use_islands::derive_islands;

fn meshes() -> Value {
    let path = format!(
        "{}/tests/fixtures/df_native_islands.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str::<Value>(&std::fs::read_to_string(path).unwrap()).unwrap()["meshes"]
        .clone()
}

fn model(
    meshes: &Value,
    name: &str,
    id: &str,
    position: Vec3,
    rotation: Vec3,
    visible: bool,
) -> ModelInput {
    let soup: Vec<f64> = serde_json::from_value(meshes[name].clone()).unwrap();
    ModelInput {
        id: id.to_owned(),
        geometry: BufferGeometry {
            position: Some(soup.iter().map(|&v| v as f32).collect()),
            ..Default::default()
        },
        transform: ModelTransform {
            position,
            rotation,
            scale: Vec3::new(1.0, 1.0, 1.0),
        },
        visible,
    }
}

/// Two scenes: a table on the plate beside floating cones (solid raft), and
/// a turned shell and a lifted knot with a per-model line raft.
fn scenes() -> Vec<(Vec<ModelInput>, PipelineSettings)> {
    let m = meshes();
    let a = vec![
        model(
            &m,
            "cones",
            "cones",
            Vec3::new(0.0, 0.0, 10.0),
            Vec3::ZERO,
            true,
        ),
        model(
            &m,
            "table",
            "table",
            Vec3::new(30.0, 0.0, 7.5),
            Vec3::ZERO,
            true,
        ),
    ];
    let b = vec![
        model(
            &m,
            "shell",
            "shell",
            Vec3::new(-5.0, 4.0, 12.0),
            Vec3::new(0.3, -0.2, 0.7),
            true,
        ),
        model(
            &m,
            "knot",
            "knot",
            Vec3::new(25.0, -10.0, 16.0),
            Vec3::new(0.0, 0.0, 1.2),
            false,
        ),
    ];
    let sa = PipelineSettings::defaults(0.05);
    let mut sb = PipelineSettings::defaults(0.05);
    sb.rafts.per_model.insert(
        "knot".to_owned(),
        RaftSettings {
            bottom_mode: RaftBottomMode::Line,
            wall_enabled: false,
            ..DEFAULT_RAFT_SETTINGS
        },
    );
    vec![(a, sa), (b, sb)]
}

fn id_gen() -> impl FnMut() -> String {
    let mut next = 0u64;
    move || {
        next += 1;
        format!("id-{next}")
    }
}

fn wire_rafts(rafts: &[RaftGroup]) -> Value {
    Value::Array(
        rafts
            .iter()
            .map(|g| {
                json!({
                    "model": g.model_key,
                    "meshes": g.meshes.iter().map(|m| json!({
                        "position": m.position, "normal": m.normal, "index": m.index,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}

fn triples(soup: &[f32]) -> Vec<[f32; 3]> {
    soup.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect()
}

/// The stages called one by one, as the parity tests chain them.
fn by_stage(
    models: &[ModelInput],
    settings: &PipelineSettings,
) -> (Vec<Value>, SupportState, Value) {
    let soups: Vec<Vec<f32>> = models.iter().map(world_soup).collect();
    let bvhs: Vec<Arc<MeshBvh>> = soups
        .iter()
        .map(|s| Arc::new(MeshBvh::new(triples(s), None)))
        .collect();
    let brace_meshes: BraceMeshes<'_> = models
        .iter()
        .zip(&bvhs)
        .map(|(m, bvh)| {
            (
                m.id.clone(),
                BraceMesh {
                    bvh,
                    matrix: Matrix4::IDENTITY,
                },
            )
        })
        .collect();
    let bands = SizingBands::factory();
    let mut new_id = id_gen();
    let angle = settings
        .supports
        .auto_support
        .overhang_self_support_angle_deg as f32;
    let mut state = SupportState::default();
    let mut per_model = Vec::new();
    for (i, m) in models.iter().enumerate() {
        let soup = &soups[i];
        let has_raft = settings.rafts.for_model(Some(&m.id)).bottom_mode != RaftBottomMode::Off;
        let mesh = IndexedMesh::from_triangle_soup(soup, 1e-5);
        let combined = scan_islands(&mesh, 0.05, 0.05, 0.25, 4, Some(2), Some(angle), Some(0.25));
        let (regions, stability) = overhang_and_stability_from_soup(soup, angle, 0.25, has_raft);
        let derived = derive_islands(
            &combined,
            &OverhangScan { regions, stability },
            &[],
            &settings.panel,
        );
        let mut g = BufferGeometry {
            position: Some(soup.clone()),
            ..Default::default()
        };
        g.compute_vertex_normals();
        let normals = triples(g.normal.as_ref().unwrap());
        let sdf = RefCell::new(SdfCache::new(bvhs[i].clone(), Matrix4::IDENTITY, Some(0.5)));
        let ctx = PlanCtx::new(
            &settings.supports,
            &bands,
            Some(PlanMesh {
                bvh: &bvhs[i],
                matrix: Matrix4::IDENTITY,
                side: Side::Front,
                normals: Some(&normals),
                sdf: &sdf,
                blocked: None,
            }),
            &mut new_id,
        );
        let plan = compute_auto_support_plan(
            &PlanInput {
                islands: &derived.filtered,
                model_id: &m.id,
                settings_override: Some(&settings.supports.auto_support),
                base_state: &state,
                committed: &state,
                has_raft,
                brace_meshes: &brace_meshes,
            },
            &ctx,
        );
        drop(ctx);
        let ids: Vec<&str> = derived.filtered.iter().map(|i| i.id.as_str()).collect();
        per_model.push(json!({
            "islands": ids,
            "result": plan.as_ref().map(|p| serde_json::to_value(&p.result).unwrap()),
        }));
        if let Some(p) = plan {
            state = p.support;
        }
    }
    let rafts = build_rafts(&state, models, &settings.rafts).unwrap();
    (per_model, state, wire_rafts(&rafts))
}

#[test]
fn facade_matches_the_stages_called_one_by_one() {
    for (k, (models, settings)) in scenes().iter().enumerate() {
        let mut new_id = id_gen();
        let mut stages = Vec::new();
        let out = generate_supports(
            models,
            &SupportState::default(),
            settings,
            &mut new_id,
            &mut |p| stages.push((p.stage, p.model)),
            &CancelToken::new(),
        )
        .unwrap();
        let (per_model, state, rafts) = by_stage(models, settings);
        let facade_models: Vec<Value> = out
            .models
            .iter()
            .map(|m| {
                let ids: Vec<&str> = m.islands.filtered.iter().map(|i| i.id.as_str()).collect();
                json!({
                    "islands": ids,
                    "result": m.plan.as_ref().map(|p| serde_json::to_value(&p.result).unwrap()),
                })
            })
            .collect();
        assert_eq!(
            Value::Array(facade_models),
            Value::Array(per_model),
            "scene {k}: plans"
        );
        assert_eq!(
            serde_json::to_value(&out.support).unwrap(),
            serde_json::to_value(&state).unwrap(),
            "scene {k}: support state"
        );
        assert_eq!(wire_rafts(&out.rafts), rafts, "scene {k}: rafts");
        assert!(
            !out.support.collections.roots.is_empty(),
            "scene {k}: supports were placed"
        );
        assert!(!out.rafts.is_empty(), "scene {k}: rafts were built");
        let keys: Vec<&str> = out.rafts.iter().map(|g| g.model_key.as_str()).collect();
        // Scene 1: the shell rests on itself (sticks and stumps, no roots);
        // the knot's per-model line raft is built although the knot is hidden.
        if k == 1 {
            assert_eq!(keys, ["knot"], "scene 1: raft groups");
        }

        // Stage reports: geometry for every model, then scan/derive/plan per
        // model, then the rafts.
        let n = models.len();
        let mut expected: Vec<(Stage, usize)> = (0..n).map(|i| (Stage::WorldGeometry, i)).collect();
        for i in 0..n {
            expected.extend([
                (Stage::ScanIslands, i),
                (Stage::DeriveIslands, i),
                (Stage::PlanSupports, i),
            ]);
        }
        expected.push((Stage::BuildRafts, n));
        assert_eq!(stages, expected, "scene {k}: progress");
    }
}

#[test]
fn cancelling_at_any_stage_returns_cancelled() {
    let (models, settings) = scenes().swap_remove(0);
    let total = 3 * models.len() + models.len() + 1;
    for at in 0..total {
        let cancel = CancelToken::new();
        let mut seen = 0;
        let mut new_id = id_gen();
        let result = generate_supports(
            &models,
            &SupportState::default(),
            &settings,
            &mut new_id,
            &mut |_| {
                if seen == at {
                    cancel.cancel();
                }
                seen += 1;
            },
            &cancel,
        );
        assert!(
            matches!(result, Err(PipelineError::Cancelled)),
            "cancel at report {at}: expected Cancelled"
        );
        assert_eq!(
            seen,
            at + 1,
            "cancel at report {at}: stopped at the next boundary"
        );
    }
}
