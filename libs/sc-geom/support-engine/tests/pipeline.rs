//! the `pipeline` facade. Over scenes built from the block-8a meshes it
//! must give exactly what the stage-by-stage calls the parity tests use give
//! (scan, derivation, plan with bracing, support mesh), report its stages in
//! order, and stop with `Cancelled` and no output at every stage boundary.

use std::sync::Arc;

use serde_json::{Value, json};

use support_engine::bvh::MeshBvh;
use support_engine::geom::Vec3;
use support_engine::geom::buffer_geometry::BufferGeometry;
use support_engine::pipeline::{
    CancelToken, ModelInput, PipelineError, PipelineSettings, Stage, generate_supports,
    generate_supports_with, plan_supports_with, prescan_models, world_soup,
};
use support_engine::supports::rafts::crenelated::geometry::model_plate_footprint::ModelTransform;
use support_engine::supports::rafts::crenelated::raft_types::{
    DEFAULT_RAFT_SETTINGS, RaftBottomMode, RaftSettings,
};
use support_engine::supports::types::SupportState;

fn meshes() -> Value {
    let path = format!(
        "{}/tests/fixtures/native_islands.json",
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
        geometry: Arc::new(BufferGeometry {
            position: Some(soup.iter().map(|&v| v as f32).collect()),
            ..Default::default()
        }),
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
    sb.raft = RaftSettings {
        bottom_mode: RaftBottomMode::Line,
        wall_enabled: false,
        ..DEFAULT_RAFT_SETTINGS
    };
    vec![(a, sa), (b, sb)]
}

fn id_gen() -> impl FnMut() -> String {
    let mut next = 0u64;
    move || {
        next += 1;
        format!("id-{next}")
    }
}

fn triples(soup: &[f32]) -> Vec<[f32; 3]> {
    soup.as_chunks::<3>()
        .0
        .iter()
        .map(|c| [c[0], c[1], c[2]])
        .collect()
}

/// Everything a run hands back that a reused scan could change.
fn run_digest(out: &support_engine::pipeline::PipelineOutput) -> Value {
    let models: Vec<Value> = out
        .models
        .iter()
        .map(|m| {
            json!({
                "combined": format!("{:?}", m.combined_scan),
                "overhang": format!("{:?}", m.overhang_scan),
                "islands": format!("{:?}", m.islands),
                "result": m.plan.as_ref().map(|p| serde_json::to_value(&p.result).unwrap()),
            })
        })
        .collect();
    json!({
        "models": models,
        "support": serde_json::to_value(&out.support).unwrap(),
        "soup": out.support_soup,
        "parts": out.support_parts,
    })
}

#[test]
fn a_prescan_gives_the_same_run_and_misses_a_moved_model() {
    for (k, (models, settings)) in scenes().iter().enumerate() {
        let run = |prescanned| {
            let mut new_id = id_gen();
            generate_supports_with(
                models,
                &SupportState::default(),
                settings,
                prescanned,
                &mut new_id,
                &mut |_| {},
                &CancelToken::new(),
            )
            .unwrap()
        };
        let plain = run(None);
        let cache = prescan_models(models, settings, None, &CancelToken::new()).unwrap();
        assert_eq!(cache.len(), models.len(), "scene {k}: one entry per model");
        let reused = run(Some(&cache));
        assert_eq!(
            run_digest(&reused),
            run_digest(&plain),
            "scene {k}: same run"
        );
        assert!(
            reused
                .timings
                .entries
                .iter()
                .any(|e| e.label == "scan:prescanned"),
            "scene {k}: the scans were reused"
        );

        // Moved models: the cache keys on the world geometry, so nothing in
        // it applies and the run scans afresh, as if there were no cache.
        let moved: Vec<ModelInput> = models
            .iter()
            .map(|m| {
                let mut m = m.clone();
                m.transform.position.x += 3.0;
                m
            })
            .collect();
        let run_moved = |prescanned| {
            let mut new_id = id_gen();
            generate_supports_with(
                &moved,
                &SupportState::default(),
                settings,
                prescanned,
                &mut new_id,
                &mut |_| {},
                &CancelToken::new(),
            )
            .unwrap()
        };
        let stale = run_moved(Some(&cache));
        assert!(
            !stale
                .timings
                .entries
                .iter()
                .any(|e| e.label == "scan:prescanned"),
            "scene {k}: a moved model is not served from the cache"
        );
        assert_eq!(
            run_digest(&stale),
            run_digest(&run_moved(None)),
            "scene {k}: moved"
        );
    }
}

#[test]
fn planning_alone_gives_the_same_plans_without_drawing_them() {
    for (k, (models, settings)) in scenes().iter().enumerate() {
        let cache = prescan_models(models, settings, None, &CancelToken::new()).unwrap();
        for prescanned in [None, Some(&cache)] {
            let mut new_id = id_gen();
            let full = generate_supports_with(
                models,
                &SupportState::default(),
                settings,
                prescanned,
                &mut new_id,
                &mut |_| {},
                &CancelToken::new(),
            )
            .unwrap();
            let mut new_id = id_gen();
            let mut stages = Vec::new();
            let planned = plan_supports_with(
                models,
                &SupportState::default(),
                settings,
                prescanned,
                &mut new_id,
                &mut |p| stages.push(p.stage),
                &CancelToken::new(),
            )
            .unwrap();
            let without_drawing = |out: &support_engine::pipeline::PipelineOutput| {
                let mut digest = run_digest(out);
                digest["soup"] = Value::Null;
                digest["parts"] = Value::Null;
                digest
            };
            assert_eq!(
                without_drawing(&planned),
                without_drawing(&full),
                "scene {k}: same plans"
            );
            assert!(
                planned.support_soup.is_empty() && planned.support_parts.is_empty(),
                "scene {k}: nothing drawn"
            );
            assert!(
                !stages.contains(&Stage::Tessellate),
                "scene {k}: no drawing stage reported"
            );
            // The BVHs handed back are the ones the world soups build.
            assert_eq!(planned.bvhs.len(), models.len());
            for (m, bvh) in models.iter().zip(&planned.bvhs) {
                let fresh = MeshBvh::new(triples(&world_soup(m)), None);
                assert_eq!(format!("{bvh:?}"), format!("{fresh:?}"), "scene {k}: BVH");
            }
        }
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

/// Prints where a run spends its time: `cargo test --release --test pipeline
/// timings -- --ignored --nocapture`.
#[test]
#[ignore = "a report, not a check"]
fn timings_report() {
    for (k, (models, settings)) in scenes().iter().enumerate() {
        let mut new_id = id_gen();
        let out = generate_supports(
            models,
            &SupportState::default(),
            settings,
            &mut new_id,
            &mut |_| {},
            &CancelToken::new(),
        )
        .unwrap();
        eprintln!("scene {k}:\n{}", out.timings.report());
    }
}
