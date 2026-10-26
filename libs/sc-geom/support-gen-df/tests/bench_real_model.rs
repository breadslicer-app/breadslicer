//! Timing report on a real model (SC-82), as the GUI runs it: the model
//! centred, lifted 5 mm off the plate, smooth tubes and robust rafts.
//!
//! ```text
//! SC_BENCH_STL=C:/path/model.stl [SC_BENCH_HEIGHT=mm] [SC_BENCH_ROT=rx,ry,rz] [SC_BENCH_RUNS=2] \
//!   cargo test --release --test bench_real_model -- --ignored --nocapture
//! ```
//!
//! `SC_BENCH_HEIGHT` scales the model to that height (an STL in other
//! units). Binary STL only. Prints each run's report and a digest of the support
//! state, so an optimisation can show it left the result unchanged.

use support_gen_df::features::slicing::smooth_shafts::{ShaftStyle, SmoothShaftParams};
use support_gen_df::pipeline::{CancelToken, ModelInput, PipelineSettings, generate_supports};
use support_gen_df::supports::rafts::crenelated::geometry::model_plate_footprint::ModelTransform;
use support_gen_df::supports::types::SupportState;
use support_gen_df::three::Vec3;
use support_gen_df::three::buffer_geometry::BufferGeometry;

fn read_binary_stl(path: &str) -> Vec<f32> {
    let bytes = std::fs::read(path).expect("read STL");
    let count = u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize;
    assert_eq!(bytes.len(), 84 + count * 50, "not a binary STL");
    let mut soup = Vec::with_capacity(count * 9);
    for t in 0..count {
        let rec = &bytes[84 + t * 50..84 + t * 50 + 50];
        for k in 0..9 {
            let o = 12 + k * 4;
            soup.push(f32::from_le_bytes(rec[o..o + 4].try_into().unwrap()));
        }
    }
    soup
}

/// FNV-1a over a debug rendering: enough to tell two results apart.
fn digest(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    })
}

#[test]
#[ignore = "a report on a local model, not a check"]
fn real_model_timings() {
    let Ok(path) = std::env::var("SC_BENCH_STL") else {
        eprintln!("set SC_BENCH_STL to a binary STL");
        return;
    };
    let rot: Vec<f64> = std::env::var("SC_BENCH_ROT")
        .map(|s| s.split(',').map(|v| v.trim().parse().unwrap()).collect())
        .unwrap_or_else(|_| vec![0.0, 0.0, 0.0]);
    let runs: usize = std::env::var("SC_BENCH_RUNS").map_or(2, |s| s.parse().unwrap());
    let mut soup = read_binary_stl(&path);
    let (mut min, mut max) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in soup.chunks_exact(3) {
        for k in 0..3 {
            min[k] = min[k].min(p[k]);
            max[k] = max[k].max(p[k]);
        }
    }
    let centre: Vec<f32> = (0..3).map(|k| (min[k] + max[k]) / 2.0).collect();
    let scale: f32 = std::env::var("SC_BENCH_HEIGHT")
        .ok()
        .filter(|h| !h.is_empty())
        .map_or(1.0, |h| h.parse::<f32>().unwrap() / (max[2] - min[2]));
    for p in soup.chunks_exact_mut(3) {
        for k in 0..3 {
            p[k] = (p[k] - centre[k]) * scale;
        }
    }
    for k in 0..3 {
        (min[k], max[k]) = ((min[k] - centre[k]) * scale, (max[k] - centre[k]) * scale);
    }
    // The pipeline drops the posed model onto the plate and lifts it by its
    // own rule only through the transform: put its lowest point 5 mm up for
    // the unrotated pose, which is what the GUI tests used.
    let half_height = ((max[2] - min[2]) / 2.0) as f64;
    let model = ModelInput {
        id: "bench".into(),
        geometry: BufferGeometry {
            position: Some(soup.clone()),
            ..Default::default()
        },
        transform: ModelTransform {
            position: Vec3::new(0.0, 0.0, half_height + 5.0),
            rotation: Vec3::new(rot[0], rot[1], rot[2]),
            scale: Vec3::new(1.0, 1.0, 1.0),
        },
        visible: true,
    };
    let mut settings = PipelineSettings::defaults(0.05);
    settings.shaft_style = ShaftStyle::Smooth(SmoothShaftParams::default());
    settings.robust_raft_geometry = true;
    eprintln!(
        "{path}: {} triangles, {:.1} x {:.1} x {:.1} mm",
        soup.len() / 9,
        max[0] - min[0],
        max[1] - min[1],
        max[2] - min[2]
    );
    for run in 0..runs {
        let mut n = 0u64;
        let mut id = || {
            n += 1;
            format!("i{n}")
        };
        let out = generate_supports(
            std::slice::from_ref(&model),
            &SupportState::default(),
            &settings,
            &mut id,
            &mut |_| {},
            &CancelToken::new(),
        )
        .unwrap();
        let state = format!("{:?}", out.support.collections);
        eprintln!(
            "run {run}: roots {}, support triangles {}, state digest {:016x}\n{}",
            out.support.collections.roots.len(),
            out.support_soup.len() / 9,
            digest(&state),
            out.timings.report()
        );
    }
}
