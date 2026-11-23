//! Timing report of the whole support job on real models (BS-231), as the
//! Supports tab runs it: the models centred on the plate and lifted 5 mm,
//! the GUI's default settings for a 0.05 mm layer on the M7 Pro, cold and
//! with a prescan.
//!
//! ```text
//! SC_BENCH_STL="C:/a.stl|C:/b.stl" [SC_BENCH_HEIGHT=mm] [SC_BENCH_RUNS=3] \
//!   cargo test --release -p bread-support-session --test bench_support_job -- --ignored --nocapture
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

use bread_support_session::{
    SupportOutcome, SupportRequest, SupportSession, start_prescan, start_supports,
};
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_scene::{LeafPayload, Scene};

// The M7 Pro's pixel pitch.
const PITCH_MM: [f64; 2] = [0.016_800_030_048_076_92, 0.0248];

/// FNV-1a: enough to tell two results apart.
fn digest(bytes: impl IntoIterator<Item = u8>) -> u64 {
    bytes.into_iter().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    })
}

/// The support state with the run's id prefix (a timestamp) taken out.
fn state_digest(outcome: &SupportOutcome) -> u64 {
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
fn mesh_digest(outcome: &SupportOutcome) -> u64 {
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
    let request = |prescanned| SupportRequest {
        layer_height_mm: 0.05,
        pixel_pitch_mm: Some(PITCH_MM),
        prescanned,
        ..Default::default()
    };
    let t = Instant::now();
    let prescan = start_prescan(&scene, request(None)).join().unwrap();
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
            let outcome = start_supports(&scene, request(warm.then(|| prescan.scans.clone())))
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
fn same_documents(kept: &SupportSession, fresh: &SupportSession) -> bool {
    let document = |session: &SupportSession| {
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
        start_supports(
            &scene,
            SupportRequest {
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

/// What the beam frame of #244 costs against generating the supports: the
/// sets of a real plate generated with the Miniatures preset, read as a
/// frame and checked on 12 layers spread over their tips, each body pulled
/// with 5 N, the layers in parallel as the app runs them.
///
/// ```text
/// SC_BENCH_STL="C:/a.stl" [SC_BENCH_HEIGHT=mm] \
///   cargo test --release -p bread-support-session --test main support_frame_timings -- --ignored --nocapture
/// ```
#[test]
#[ignore = "needs SC_BENCH_STL"]
fn support_frame_timings() {
    use bread_structure::{BodyLoad, CheckOptions, Material, check_layer};
    use rayon::prelude::*;
    let Ok(paths) = std::env::var("SC_BENCH_STL") else {
        return;
    };
    let height = std::env::var("SC_BENCH_HEIGHT")
        .ok()
        .and_then(|h| h.parse().ok());
    let (scene, triangles) = plate(&paths, height);
    let request = SupportRequest {
        options: bread_support_session::SupportOptions {
            zones: Some(bread_support_session::zone_presets::minis_30()),
            ..Default::default()
        },
        ..SupportRequest::default()
    };
    let t = Instant::now();
    let outcome = start_supports(&scene, request).unwrap().join().unwrap();
    let generate = t.elapsed();
    let t = Instant::now();
    let graphs: Vec<_> = outcome
        .sets
        .par_iter()
        .map(|set| set.session.support_graph())
        .collect();
    let read = t.elapsed();
    let tips: Vec<[f64; 3]> = graphs
        .iter()
        .flat_map(|g| g.contacts().iter().map(|c| c.tip))
        .collect();
    let (lo, hi) = tips.iter().fold((f64::INFINITY, 0.0f64), |(lo, hi), p| {
        (lo.min(p[2]), hi.max(p[2]))
    });
    let options = CheckOptions {
        material: Material::GREEN_RESIN,
        // The tips' 8 MPa of the peel model, without the neck's concentration.
        strength_mpa: 16.0,
        lateral_share: std::env::var("SC_BENCH_LATERAL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.05),
        directions: 8,
    };
    let layers: Vec<f64> = (1..=12)
        .map(|k| lo + (hi - lo + 0.5) * f64::from(k) / 12.0)
        .collect();
    let t = Instant::now();
    let checks: Vec<_> = layers
        .par_iter()
        .map(|&z| {
            let held: Vec<[f64; 3]> = tips.iter().copied().filter(|p| p[2] <= z).collect();
            let n = held.len().max(1) as f64;
            let c = held
                .iter()
                .fold([0.0; 3], |a, p| [a[0] + p[0], a[1] + p[1], 0.0]);
            let body = BodyLoad {
                force_n: 5.0,
                at: [c[0] / n, c[1] / n, z],
                held: false,
                tips: held,
                feet: graphs
                    .iter()
                    .flat_map(|g| g.feet().iter().map(|f| f.at))
                    .collect(),
            };
            let t = Instant::now();
            let check = check_layer(&graphs, z, &[body], &options).unwrap();
            (z, t.elapsed(), check)
        })
        .collect();
    let frame = t.elapsed();
    {
        let mut necks: Vec<(f64, f64)> = graphs
            .iter()
            .flat_map(|g| {
                g.frame()
                    .members()
                    .iter()
                    .enumerate()
                    .filter(|(_, m)| m.kt > 1.0)
                    .map(|(k, m)| (m.d_a, g.frame().length(k)))
                    .collect::<Vec<_>>()
            })
            .collect();
        necks.sort_by(|a, b| a.0.total_cmp(&b.0));
        let contacts: Vec<_> = outcome.sets[0]
            .session
            .pieces()
            .into_iter()
            .flat_map(|p| p.contacts)
            .take(3)
            .collect();
        println!(
            "  necks {}: d min {:.3} median {:.3} max {:.3}, length median {:.3}; contacts {:?}",
            necks.len(),
            necks[0].0,
            necks[necks.len() / 2].0,
            necks[necks.len() - 1].0,
            {
                let mut l: Vec<f64> = necks.iter().map(|n| n.1).collect();
                l.sort_by(f64::total_cmp);
                l[l.len() / 2]
            },
            contacts
        );
    }
    let nodes: usize = graphs.iter().map(|g| g.frame().nodes().len()).sum();
    let beams: usize = graphs.iter().map(|g| g.frame().members().len()).sum();
    let feet: usize = graphs.iter().map(|g| g.feet().len()).sum();
    println!(
        "{triangles} triangles, {} sets: generate {:.0} ms; graph {:.1} ms ({nodes} nodes, {beams} beams, {} tips, {feet} feet); 12 layers {:.0} ms = {:.1} % of generate",
        outcome.sets.len(),
        ms(generate),
        ms(read),
        tips.len(),
        ms(frame),
        100.0 * ms(frame + read) / ms(generate)
    );
    if let Some((_, _, check)) = checks.last() {
        let mut top: Vec<(f64, &str)> = graphs
            .iter()
            .zip(&check.owners)
            .flat_map(|(g, owners)| {
                g.owners()
                    .iter()
                    .map(String::as_str)
                    .zip(owners.iter().copied())
            })
            .map(|(id, u)| (u, id))
            .collect();
        top.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut sorted: Vec<f64> = top.iter().map(|t| t.0).filter(|u| *u > 0.0).collect();
        sorted.reverse();
        let pct = |p: f64| sorted[((sorted.len() - 1) as f64 * p) as usize];
        let c = &outcome.sets[0].session.state().collections;
        let kind = |id: &str| {
            if c.trunks.contains_key(id) {
                "trunk"
            } else if c.branches.contains_key(id) {
                "branch"
            } else if c.leaves.contains_key(id) {
                "leaf"
            } else if c.twigs.contains_key(id) {
                "twig"
            } else if c.sticks.contains_key(id) {
                "stick"
            } else if c.stumps.contains_key(id) {
                "stump"
            } else if c.braces.contains_key(id) {
                "brace"
            } else if c.kickstands.contains_key(id) {
                "kickstand"
            } else {
                "?"
            }
        };
        let mut by_kind: std::collections::BTreeMap<&str, (usize, f64, f64)> = Default::default();
        for (u, id) in &top {
            let e = by_kind.entry(kind(id)).or_default();
            e.0 += 1;
            e.1 += u;
            e.2 = e.2.max(*u);
        }
        println!(
            "  top layer: median {:.2}, p90 {:.2}, worst {:?}",
            pct(0.5),
            pct(0.9),
            top[..8.min(top.len())]
                .iter()
                .map(|(u, id)| (kind(id), (u * 100.0).round() / 100.0))
                .collect::<Vec<_>>()
        );
        for (k, (n, sum, max)) in by_kind {
            println!("    {k}: {n}, mean {:.2}, max {:.2}", sum / n as f64, max);
        }
    }
    for (z, took, check) in &checks {
        println!(
            "  z {z:7.2}: {:7.1} ms, tips {}/{}, unheld {}, worst {:.2}, residual {:.1e}",
            ms(*took),
            check.tips_found,
            check.tips_found + check.tips_missed,
            check.bodies_unheld,
            check.worst,
            check.residual
        );
    }
}

/// Writes the supports of the models in `SC_BENCH_STL` as frames, with a
/// load on every tip and this crate's solution, to `SC_FRAME_EXPORT` (a
/// folder): one JSON per set, to solve again with a program outside (#242).
/// Each solved with and without shear (`shear` and `slender`).
#[test]
#[ignore = "writes frames for an outside check; set SC_BENCH_STL and SC_FRAME_EXPORT"]
fn export_frames_for_an_outside_check() {
    use bread_structure::{Hold, Load, Material, Target, solve};
    use std::fmt::Write as _;
    let (Ok(paths), Ok(out)) = (
        std::env::var("SC_BENCH_STL"),
        std::env::var("SC_FRAME_EXPORT"),
    ) else {
        return;
    };
    let height = std::env::var("SC_BENCH_HEIGHT")
        .ok()
        .and_then(|h| h.parse().ok());
    let (scene, _) = plate(&paths, height);
    let request = SupportRequest {
        options: bread_support_session::SupportOptions {
            zones: Some(bread_support_session::zone_presets::minis_30()),
            ..Default::default()
        },
        ..SupportRequest::default()
    };
    let outcome = start_supports(&scene, request).unwrap().join().unwrap();
    std::fs::create_dir_all(&out).unwrap();
    for (k, set) in outcome.sets.iter().enumerate() {
        let graph = set.session.support_graph();
        let frame = graph.frame();
        // Each tip pulled up and a little aside, varied from tip to tip.
        let loads: Vec<Load> = graph
            .contacts()
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let t = i as f64 * 0.7;
                Load {
                    target: Target::Node(c.node),
                    at: frame.nodes()[c.node],
                    force: [0.01 * t.cos(), 0.01 * t.sin(), 0.05],
                }
            })
            .collect();
        let mut json = String::from("{");
        let _ = write!(json, "\"nodes\":[");
        for (i, p) in frame.nodes().iter().enumerate() {
            let hold = match frame.holds()[i] {
                Hold::Free => "free".to_string(),
                Hold::Fixed => "fixed".to_string(),
                Hold::Body(b) => format!("body{b}"),
            };
            let _ = write!(
                json,
                "{}[{},{},{},\"{hold}\"]",
                if i > 0 { "," } else { "" },
                p[0],
                p[1],
                p[2]
            );
        }
        let _ = write!(json, "],\"members\":[");
        for (i, m) in frame.members().iter().enumerate() {
            let _ = write!(
                json,
                "{}[{},{},{},{}]",
                if i > 0 { "," } else { "" },
                m.a,
                m.b,
                m.d_a,
                m.d_b
            );
        }
        let _ = write!(json, "],\"loads\":[");
        for (i, l) in loads.iter().enumerate() {
            let Target::Node(n) = l.target else { continue };
            let _ = write!(
                json,
                "{}[{},{},{},{}]",
                if i > 0 { "," } else { "" },
                n,
                l.force[0],
                l.force[1],
                l.force[2]
            );
        }
        let _ = write!(json, "],\"material\":[{},{}]", 1000.0, 0.35);
        for (name, shear) in [("shear", true), ("slender", false)] {
            let material = Material {
                shear,
                ..Material::GREEN_RESIN
            };
            let solved = solve(frame, &material, std::slice::from_ref(&loads)).expect("solved");
            let _ = write!(
                json,
                ",\"{name}\":{{\"left_out\":{},\"residual\":{},\"displacements\":[",
                solved.left_out, solved.residual
            );
            for n in 0..frame.nodes().len() {
                let d = solved.displacement(0, n).unwrap_or([0.0; 6]);
                let _ = write!(
                    json,
                    "{}[{}]",
                    if n > 0 { "," } else { "" },
                    d.map(|v| v.to_string()).join(",")
                );
            }
            let _ = write!(json, "],\"forces\":[");
            for (i, f) in solved.forces[0].iter().enumerate() {
                let row = f.map_or("null".to_string(), |f| {
                    format!("[{}]", f.0.map(|v| v.to_string()).join(","))
                });
                let _ = write!(json, "{}{row}", if i > 0 { "," } else { "" });
            }
            let _ = write!(json, "]}}");
        }
        json.push('}');
        let path = format!("{out}/set{k}.json");
        std::fs::write(&path, json).unwrap();
        eprintln!(
            "{path}: {} nodes, {} members, {} tips",
            frame.nodes().len(),
            frame.members().len(),
            graph.contacts().len()
        );
    }
}
