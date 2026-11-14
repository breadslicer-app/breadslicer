//! Timing report on a real model: session, preview, apply, apply with a
//! drain, as the GUI would run them.
//!
//! ```text
//! SC_BENCH_STL=C:/path/model.stl [SC_BENCH_HEIGHT=90] [SC_BENCH_VOXEL=0.65] \
//!   [SC_BENCH_MAX_RES=192] [SC_BENCH_WALL=2.0] [SC_BENCH_FILL=lattice|pillar] \
//!   [SC_BENCH_REPEAT=1] \
//!   cargo test --release -p hollow-df --test bench_real_model -- --ignored --nocapture
//! ```
//!
//! The model is welded, centred and scaled to `SC_BENCH_HEIGHT` mm (as the
//! parity test does). The drain goes in from below, straight up, under the
//! cavity's lowest point; then `SC_BENCH_MANY` (8) drains that open, spread
//! over the model, are applied together. With `SC_BENCH_FILL` the hollow has that infill
//! (6 mm cells, 0.6 mm beams). Applies run `SC_BENCH_REPEAT` times (the
//! fastest is reported), and each result's fingerprint is printed, to tell
//! two builds' meshes apart.

use std::time::Instant;

use hollow_df::bvh::Bvh;
use hollow_df::mesh::{IndexedMesh, Vec3};
use hollow_df::shapes::read_binary_stl;
use hollow_df::topology::{face_shells, is_edge_manifold};
use hollow_df::{
    Control, DrainHole, GridRequest, HollowApplied, HollowError, HollowFill, HollowSettings,
    InfillMode, Session,
};

fn env_or<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

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

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

/// When each stage was first reported, to print how long each took.
struct StageClock {
    start: Instant,
    marks: std::sync::Mutex<Vec<(hollow_df::Stage, Instant)>>,
}

impl StageClock {
    fn new() -> Self {
        Self {
            start: Instant::now(),
            marks: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn mark(&self, stage: hollow_df::Stage) {
        let mut marks = self.marks.lock().unwrap();
        if !marks.iter().any(|(s, _)| *s == stage) {
            marks.push((stage, Instant::now()));
        }
    }

    /// Each stage and how long it ran, up to the next one or to now.
    fn report(&self, what: &str) {
        let end = Instant::now();
        let marks = self.marks.lock().unwrap();
        let mut line = format!("{what} stages:");
        let mut before = self.start;
        if let Some((_, first)) = marks.first() {
            line += &format!(" [setup] {:.0}", (*first - before).as_secs_f64() * 1e3);
        }
        for (k, (stage, at)) in marks.iter().enumerate() {
            let next = marks.get(k + 1).map_or(end, |(_, t)| *t);
            line += &format!(" | {stage:?} {:.0}", (next - *at).as_secs_f64() * 1e3);
            before = next;
        }
        let _ = before;
        eprintln!("{line} ms");
    }
}

/// The hollow `SC_BENCH_FILL` asks for.
fn bench_fill() -> HollowFill {
    let pattern = match std::env::var("SC_BENCH_FILL").as_deref() {
        Ok("lattice") => InfillMode::Lattice,
        Ok("pillar") => InfillMode::Pillar,
        _ => return HollowFill::Cavity,
    };
    HollowFill::Infill {
        pattern,
        cell_mm: 6.0,
        beam_radius_mm: 0.6,
    }
}

/// FNV-1a over the positions' bits and the triangles.
fn fingerprint(mesh: &IndexedMesh) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut eat = |x: u32| {
        for b in x.to_le_bytes() {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    };
    for p in &mesh.positions {
        for c in [p.x, p.y, p.z] {
            eat(c.to_bits());
        }
    }
    for &v in mesh.triangles.iter().flatten() {
        eat(v);
    }
    h
}

/// The fastest of `SC_BENCH_REPEAT` applies, and its result.
fn timed_apply(
    session: &Session,
    settings: &HollowSettings,
    drains: &[DrainHole],
) -> (f64, Result<HollowApplied, HollowError>) {
    let mut best = f64::INFINITY;
    let mut result = None;
    for _ in 0..env_or("SC_BENCH_REPEAT", 1usize).max(1) {
        let t = Instant::now();
        let out = session.apply(settings, drains, Control::none());
        best = best.min(ms(t));
        result = Some(out);
    }
    (best, result.unwrap())
}

#[test]
#[ignore = "a report on a local model, not a check"]
fn real_model_hollow_timings() {
    let Ok(path) = std::env::var("SC_BENCH_STL") else {
        eprintln!("set SC_BENCH_STL to a binary STL");
        return;
    };
    let height = env_or("SC_BENCH_HEIGHT", 90.0f32);
    let request = GridRequest {
        voxel_size_mm: env_or("SC_BENCH_VOXEL", 0.65f32),
        max_resolution: env_or("SC_BENCH_MAX_RES", 192u16),
        ..GridRequest::default()
    };
    let settings = HollowSettings {
        wall_thickness_mm: env_or("SC_BENCH_WALL", 2.0f32),
        fill: bench_fill(),
        ..HollowSettings::default()
    };

    let t = Instant::now();
    let mesh = posed(
        read_binary_stl(&std::fs::read(&path).unwrap()).expect("binary STL"),
        height,
    );
    let load_ms = ms(t);
    let source_manifold = is_edge_manifold(&mesh);
    eprintln!(
        "model: {} triangles, {} vertices, edge-manifold {source_manifold}, loaded in {load_ms:.0} ms",
        mesh.triangle_count(),
        mesh.vertex_count()
    );

    let t = Instant::now();
    let clock = StageClock::new();
    let mark = |stage| clock.mark(stage);
    let session = Session::build(mesh, &request, None, Control::new(None, Some(&mark))).unwrap();
    let session_ms = ms(t);
    clock.report("session");
    let grid = session.grid();
    eprintln!(
        "grid: requested {:.3} mm, used {:.4} mm ({} voxels on the largest extent, bound {:?}), dims {:?}, ~{} MB",
        grid.requested_voxel_mm,
        grid.voxel_mm,
        grid.resolution,
        grid.bound,
        grid.dims,
        grid.memory_estimate_bytes / (1 << 20)
    );

    let t = Instant::now();
    let clock = StageClock::new();
    let mark = |stage| clock.mark(stage);
    let preview = session
        .preview(&settings, Control::new(None, Some(&mark)))
        .unwrap();
    let preview_ms = ms(t);
    clock.report("preview");

    let (apply_ms, applied) = timed_apply(&session, &settings, &[]);

    // Under the cavity's lowest point: the first surface straight down.
    let floor = *preview
        .cavity
        .positions
        .iter()
        .min_by(|a, b| a.z.total_cmp(&b.z))
        .unwrap();
    let source = session.source_mesh();
    let down = Vec3::new(0.0, 0.0, -1.0);
    let skin = Bvh::build(source).ray_hit_distances(source, floor, down)[0];
    let entry = floor.add(down.scale(skin));
    let drain = DrainHole::new(entry.to_array(), [0.0, 0.0, 1.0], 1.5);
    let (drained_ms, drained) = timed_apply(&session, &settings, &[drain]);

    // Spread over the model, into it against each triangle's normal, at
    // least 10 mm apart: the drains the report says open.
    let many = env_or("SC_BENCH_MANY", 8usize);
    let mut spread: Vec<DrainHole> = Vec::new();
    let step = (source.triangle_count() / (many * 50)).max(1);
    for i in (0..source.triangle_count()).step_by(step) {
        if spread.len() == many {
            break;
        }
        let [a, b, c] = source.triangles[i].map(|v| source.positions[v as usize]);
        let center = a.add(b).add(c).scale(1.0 / 3.0);
        let n = b.sub(a).cross(c.sub(a));
        let len = n.dot(n).sqrt().max(1e-12);
        let hole = DrainHole::new(center.to_array(), n.scale(-1.0 / len).to_array(), 1.5);
        let apart = spread.iter().all(|d| {
            let d = Vec3::new(d.center[0], d.center[1], d.center[2]).sub(center);
            d.dot(d) > 100.0
        });
        if apart
            && session
                .drain_report(&settings, &hole)
                .is_some_and(|r| r.reaches_cavity)
        {
            spread.push(hole);
        }
    }
    let (spread_ms, spread_out) = timed_apply(&session, &settings, &spread);

    eprintln!("fill: {:?}", settings.fill);
    eprintln!(
        "session {session_ms:.0} ms | preview {preview_ms:.0} ms | apply {apply_ms:.0} ms | apply + 1 drain {drained_ms:.0} ms"
    );
    eprintln!(
        "preview: cavity {} triangles, {:.2} ml, {} removed voxels ({:.2} ml by voxels)",
        preview.cavity.triangle_count(),
        preview.stats.cavity_volume_mm3 / 1000.0,
        preview.stats.removed_voxels,
        preview.stats.voxel_removed_volume_mm3 / 1000.0
    );
    match &applied {
        Ok(a) => eprintln!(
            "apply: {} triangles, edge-manifold {}, fingerprint {:016x}, source {:.2} ml -> {:.2} ml (removed {:.2} ml)",
            a.mesh.triangle_count(),
            is_edge_manifold(&a.mesh),
            fingerprint(&a.mesh),
            a.stats.source_volume_mm3 / 1000.0,
            a.stats.output_volume_mm3.unwrap() / 1000.0,
            a.stats.removed_volume_mm3.unwrap() / 1000.0,
        ),
        Err(e) => eprintln!("apply failed: {e}"),
    }
    match &drained {
        Ok(a) => eprintln!(
            "apply + drain: {} triangles, edge-manifold {}, fingerprint {:016x}, shells {}, drain {:?}",
            a.mesh.triangle_count(),
            is_edge_manifold(&a.mesh),
            fingerprint(&a.mesh),
            face_shells(&a.mesh).1,
            a.drains[0]
        ),
        Err(e) => eprintln!("apply + drain failed: {e}"),
    }
    match &spread_out {
        Ok(a) => eprintln!(
            "apply + {} drains {spread_ms:.0} ms: {} triangles, edge-manifold {}, fingerprint {:016x}, all cut {}",
            spread.len(),
            a.mesh.triangle_count(),
            is_edge_manifold(&a.mesh),
            fingerprint(&a.mesh),
            a.drains.iter().all(|d| d.reaches_cavity),
        ),
        Err(e) => eprintln!(
            "apply + {} drains {spread_ms:.0} ms: failed: {e}",
            spread.len()
        ),
    }
}

/// What a hover with the drain tool costs (#302): building what drains are
/// measured against, once per cavity, then one [`Session::drain_report`]
/// per surface point, on `SC_BENCH_DRAINS` points spread over the model
/// (into it against each triangle's normal). A few of them are applied
/// one by one, to check the report is what apply does.
#[test]
#[ignore = "a report on a local model, not a check"]
fn real_model_drain_report_timings() {
    let Ok(path) = std::env::var("SC_BENCH_STL") else {
        eprintln!("set SC_BENCH_STL to a binary STL");
        return;
    };
    let height = env_or("SC_BENCH_HEIGHT", 90.0f32);
    let request = GridRequest {
        voxel_size_mm: env_or("SC_BENCH_VOXEL", 0.65f32),
        max_resolution: env_or("SC_BENCH_MAX_RES", 192u16),
        ..GridRequest::default()
    };
    let settings = HollowSettings {
        wall_thickness_mm: env_or("SC_BENCH_WALL", 2.0f32),
        ..HollowSettings::default()
    };
    let count = env_or("SC_BENCH_DRAINS", 5000usize);
    let mesh = posed(
        read_binary_stl(&std::fs::read(&path).unwrap()).expect("binary STL"),
        height,
    );
    let session = Session::build(mesh, &request, None, Control::none()).unwrap();
    let t = Instant::now();
    session.preview(&settings, Control::none()).unwrap();
    let preview_ms = ms(t);

    let drain = |source: &IndexedMesh, i: usize| {
        let tri = source.triangles[i];
        let [a, b, c] = tri.map(|v| source.positions[v as usize]);
        let center = a.add(b).add(c).scale(1.0 / 3.0);
        let n = b.sub(a).cross(c.sub(a));
        let len = n.dot(n).sqrt().max(1e-12);
        DrainHole::new(center.to_array(), n.scale(-1.0 / len).to_array(), 1.5)
    };
    let source = session.source_mesh();
    let step = (source.triangle_count() / count).max(1);
    let holes: Vec<DrainHole> = (0..source.triangle_count())
        .step_by(step)
        .map(|i| drain(source, i))
        .collect();

    let t = Instant::now();
    assert!(session.prepare_drain_reports(&settings));
    let prepare_ms = ms(t);
    let mut times = Vec::with_capacity(holes.len());
    let mut opens = 0;
    for hole in &holes {
        let t = Instant::now();
        let report = session.drain_report(&settings, hole).unwrap();
        times.push(ms(t));
        opens += usize::from(report.reaches_cavity);
    }
    times.sort_by(f64::total_cmp);
    let at = |q: f64| times[((times.len() - 1) as f64 * q) as usize];
    eprintln!(
        "preview {preview_ms:.0} ms | prepare (BVH of the cavity) {prepare_ms:.1} ms | {} reports: median {:.4} ms, p99 {:.4} ms, max {:.4} ms | {opens} open, {} miss",
        holes.len(),
        at(0.5),
        at(0.99),
        at(1.0),
        holes.len() - opens
    );

    // Some of each, applied alone: the report is what apply does.
    let sample: Vec<&DrainHole> = holes
        .iter()
        .filter(|h| session.drain_report(&settings, h).unwrap().reaches_cavity)
        .step_by((opens / 5).max(1))
        .take(5)
        .chain(
            holes
                .iter()
                .filter(|h| !session.drain_report(&settings, h).unwrap().reaches_cavity)
                .step_by(((holes.len() - opens) / 5).max(1))
                .take(5),
        )
        .collect();
    for hole in sample {
        let asked = session.drain_report(&settings, hole).unwrap();
        match session.apply(&settings, &[*hole], Control::none()) {
            Ok(applied) => {
                assert_eq!(applied.drains[0], asked, "{hole:?}");
                eprintln!(
                    "applied at {:?}: reaches {}, depth {:.1} mm, shells {}",
                    hole.center,
                    asked.reaches_cavity,
                    asked.depth_mm,
                    face_shells(&applied.mesh).1
                );
            }
            Err(e) => eprintln!("apply at {:?} failed: {e}", hole.center),
        }
    }
}
