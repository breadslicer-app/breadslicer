//! Timing report of the whole support job on real models (BS-231), as the
//! Supports tab runs it: the models centred on the plate and lifted 5 mm,
//! the GUI's default settings for a 0.05 mm layer on the M7 Pro, cold and
//! with a prescan.
//!
//! ```text
//! SC_BENCH_STL="C:/a.stl|C:/b.stl" [SC_BENCH_HEIGHT=mm] [SC_BENCH_RUNS=3] \
//!   cargo test --release -p support-gen-df-sc --test bench_support_job -- --ignored --nocapture
//! ```
//!
//! Several paths (`|`) make a plate, side by side 6 mm apart. Prints the
//! wall time of each run, the time the job's own timings leave out, and digests of
//! the support state and of every set's mesh, so an optimisation can show
//! it left the result unchanged.
//!
//! `same_supports_timings` (same variables) times telling a regenerated set
//! from the one it replaces (#304) against comparing their documents as text.

use std::time::{Duration, Instant};

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_scene::{LeafPayload, Scene};
use support_gen_df_sc::{
    DfSupportOutcome, DfSupportRequest, DfSupportSession, start_df_prescan, start_df_supports,
};

// The M7 Pro's pixel pitch.
const PITCH_MM: [f64; 2] = [0.016_800_030_048_076_92, 0.0248];

/// FNV-1a: enough to tell two results apart.
fn digest(bytes: impl IntoIterator<Item = u8>) -> u64 {
    bytes.into_iter().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    })
}

/// The support state with the run's id prefix (a timestamp) taken out.
fn state_digest(outcome: &DfSupportOutcome) -> u64 {
    let text = format!("{:?}", outcome.support.collections);
    let prefix = outcome
        .support
        .collections
        .roots
        .keys()
        .next()
        .and_then(|id| id.rfind('-').map(|k| id[..k].to_owned()));
    let text = match prefix {
        Some(prefix) => text.replace(&prefix, "run"),
        None => text,
    };
    digest(text.into_bytes())
}

/// Every set's mesh, in order: vertex bits and face indices.
fn mesh_digest(outcome: &DfSupportOutcome) -> u64 {
    let mut bytes = Vec::new();
    for set in &outcome.sets {
        bytes.extend_from_slice(&set.model.get().to_le_bytes());
        let Some(mesh) = &set.mesh else {
            bytes.push(0);
            continue;
        };
        for v in mesh.vertices() {
            for c in v.as_array() {
                bytes.extend_from_slice(&c.to_bits().to_le_bytes());
            }
        }
        for f in mesh.faces() {
            for i in f.vertices {
                bytes.extend_from_slice(&u32::from(i).to_le_bytes());
            }
        }
    }
    digest(bytes)
}

fn bounds(mesh: &IndexedMesh) -> ([f32; 3], [f32; 3]) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for v in mesh.vertices() {
        let p = v.as_array();
        for k in 0..3 {
            min[k] = min[k].min(p[k]);
            max[k] = max[k].max(p[k]);
        }
    }
    (min, max)
}

/// The models side by side along X, 6 mm apart, each scaled to `height`
/// when given; the plate centred and lifted 5 mm.
fn plate(paths: &str, height: Option<f64>) -> (Scene, usize) {
    let mut scene = Scene::new();
    let mut x = 0.0f64;
    let mut triangles = 0;
    for path in paths.split('|') {
        let bytes = std::fs::read(path).expect("read STL");
        let (loaded, _) =
            sc_mesh_formats::stl::read_stl(&mut std::io::Cursor::new(bytes)).expect("an STL");
        let leaf = loaded
            .roots()
            .iter()
            .copied()
            .find(|id| loaded.leaf_mesh_arc(*id).is_some())
            .expect("an STL loads as one leaf root");
        let mesh = loaded.leaf_mesh_arc(leaf).unwrap().clone();
        triangles += mesh.faces().len();
        let (min, max) = bounds(&mesh);
        let scale = height.map_or(1.0, |h| h / f64::from(max[2] - min[2]));
        let width = f64::from(max[0] - min[0]) * scale;
        let place = AffineTransform3D::from_translation([x - f64::from(min[0]) * scale, 0.0, 0.0]);
        let transform = place.compose(&AffineTransform3D::from_uniform_scale(scale));
        let id = scene
            .add_leaf(
                None,
                Transform3D::Affine(transform),
                LeafPayload::Mesh(mesh),
            )
            .unwrap();
        scene.add_root(id).unwrap();
        x += width + 6.0;
    }
    scene.drop_at([0.0, 0.0], 5.0).unwrap();
    (scene, triangles)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

#[test]
#[ignore = "a report on local models, not a check"]
fn support_job_timings() {
    let Ok(paths) = std::env::var("SC_BENCH_STL") else {
        eprintln!("set SC_BENCH_STL to one or more binary STLs, joined by |");
        return;
    };
    let height = std::env::var("SC_BENCH_HEIGHT")
        .ok()
        .filter(|h| !h.is_empty())
        .map(|h| h.parse::<f64>().unwrap());
    let runs: usize = std::env::var("SC_BENCH_RUNS").map_or(3, |s| s.parse().unwrap());
    let (scene, triangles) = plate(&paths, height);
    let request = |prescanned| DfSupportRequest {
        layer_height_mm: 0.05,
        pixel_pitch_mm: Some(PITCH_MM),
        prescanned,
        ..Default::default()
    };
    let t = Instant::now();
    let prescan = start_df_prescan(&scene, request(None)).join().unwrap();
    eprintln!(
        "{paths}: {} model(s), {triangles} triangles; prescan {:.0} ms",
        prescan.models,
        ms(t.elapsed())
    );
    let mut best = [f64::INFINITY; 2];
    let mut untimed_best = [f64::INFINITY; 2];
    let mut digests = Vec::new();
    for run in 0..runs {
        for (k, warm) in [false, true].into_iter().enumerate() {
            let t = Instant::now();
            let outcome = start_df_supports(&scene, request(warm.then(|| prescan.scans.clone())))
                .unwrap()
                .join()
                .unwrap();
            let wall = ms(t.elapsed());
            let untimed = wall - ms(outcome.timings.total);
            best[k] = best[k].min(wall);
            untimed_best[k] = untimed_best[k].min(untimed);
            let d = (state_digest(&outcome), mesh_digest(&outcome));
            digests.push(d);
            eprintln!(
                "run {run} {}: wall {wall:.0} ms, timed {:.0} ms, untimed {untimed:.0} ms; \
                 roots {}, triangles {}; state digest {:016x}, mesh digest {:016x}",
                if warm { "prescanned" } else { "cold" },
                ms(outcome.timings.total),
                outcome.support.collections.roots.len(),
                outcome.triangles,
                d.0,
                d.1,
            );
            if run == 0 && warm {
                eprintln!("{}", outcome.timings.report());
            }
        }
    }
    eprintln!(
        "best: cold {:.0} ms (untimed {:.0} ms), prescanned {:.0} ms (untimed {:.0} ms)",
        best[0], untimed_best[0], best[1], untimed_best[1]
    );
    assert!(
        digests.windows(2).all(|w| w[0] == w[1]),
        "every run gives the same result"
    );
}

/// How #287 told two sessions apart before #304: their documents compared as
/// text once `fresh`'s id prefix reads as `kept`'s.
fn same_documents(kept: &DfSupportSession, fresh: &DfSupportSession) -> bool {
    let document = |session: &DfSupportSession| {
        let text = String::from_utf8(session.to_document()?).ok()?;
        let key = "\"idPrefix\":\"";
        let start = text.find(key)? + key.len();
        let prefix = text[start..].split('"').next()?.to_owned();
        (!prefix.is_empty()).then_some((prefix, text))
    };
    let (Some((kept_prefix, kept)), Some((fresh_prefix, fresh))) =
        (document(kept), document(fresh))
    else {
        return false;
    };
    fresh.replace(&fresh_prefix, &kept_prefix) == kept
}

#[test]
#[ignore = "a report on local models, not a check"]
fn same_supports_timings() {
    let Ok(paths) = std::env::var("SC_BENCH_STL") else {
        eprintln!("set SC_BENCH_STL to one or more binary STLs, joined by |");
        return;
    };
    let height = std::env::var("SC_BENCH_HEIGHT")
        .ok()
        .filter(|h| !h.is_empty())
        .map(|h| h.parse::<f64>().unwrap());
    let runs: usize = std::env::var("SC_BENCH_RUNS").map_or(20, |s| s.parse().unwrap());
    let (scene, _) = plate(&paths, height);
    let sessions = |layer_height_mm| {
        start_df_supports(
            &scene,
            DfSupportRequest {
                layer_height_mm,
                pixel_pitch_mm: Some(PITCH_MM),
                ..Default::default()
            },
        )
        .unwrap()
        .join()
        .unwrap()
        .sets
        .into_iter()
        .map(|set| set.session)
        .collect::<Vec<_>>()
    };
    let kept = sessions(0.05);
    let again = sessions(0.05);
    let other = sessions(0.1);
    // The first comparison makes each session's model prints; later ones,
    // and saves, reuse them.
    let t = Instant::now();
    for (k, f) in kept.iter().zip(&again) {
        assert!(k.same_supports_as(f), "the same run again is the same");
    }
    let first = ms(t.elapsed());
    let t = Instant::now();
    for (k, f) in kept.iter().zip(&other) {
        assert!(!k.same_supports_as(f), "other settings are other supports");
    }
    eprintln!(
        "first same_supports_as: the same run again {first:.2} ms, other settings {:.2} ms",
        ms(t.elapsed())
    );
    eprintln!(
        "{paths}: {} set(s), {} roots, document {} bytes",
        kept.len(),
        kept.iter()
            .map(|s| s.state().collections.roots.len())
            .sum::<usize>(),
        kept.iter()
            .map(|s| s.to_document().map_or(0, |d| d.len()))
            .sum::<usize>(),
    );
    for (name, fresh) in [("the same run again", &again), ("other settings", &other)] {
        let mut best = [f64::INFINITY; 2];
        for _ in 0..runs {
            let t = Instant::now();
            let text: Vec<bool> = kept
                .iter()
                .zip(fresh)
                .map(|(k, f)| same_documents(k, f))
                .collect();
            best[0] = best[0].min(ms(t.elapsed()));
            let t = Instant::now();
            let now: Vec<bool> = kept
                .iter()
                .zip(fresh)
                .map(|(k, f)| k.same_supports_as(f))
                .collect();
            best[1] = best[1].min(ms(t.elapsed()));
            assert_eq!(text, now, "{name}: the same verdict");
        }
        eprintln!(
            "{name}: documents as text {:.2} ms, same_supports_as {:.2} ms (best of {runs})",
            best[0], best[1]
        );
    }
}
