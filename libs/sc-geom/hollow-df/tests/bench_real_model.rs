//! Timing report on a real model: session, preview, apply, apply with a
//! drain, as the GUI would run them.
//!
//! ```text
//! SC_BENCH_STL=C:/path/model.stl [SC_BENCH_HEIGHT=90] [SC_BENCH_VOXEL=0.65] \
//!   [SC_BENCH_MAX_RES=192] [SC_BENCH_WALL=2.0] \
//!   cargo test --release -p hollow-df --test bench_real_model -- --ignored --nocapture
//! ```
//!
//! The model is welded, centred and scaled to `SC_BENCH_HEIGHT` mm (as the
//! parity test does). The drain goes in from below, straight up, under the
//! cavity's lowest point.

use std::time::Instant;

use hollow_df::bvh::Bvh;
use hollow_df::mesh::{IndexedMesh, Vec3};
use hollow_df::shapes::read_binary_stl;
use hollow_df::topology::{face_shells, is_edge_manifold};
use hollow_df::{Control, DrainHole, GridRequest, HollowSettings, Session};

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
    };
    let settings = HollowSettings {
        wall_thickness_mm: env_or("SC_BENCH_WALL", 2.0f32),
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
    let session = Session::build(mesh, &request, None, Control::none()).unwrap();
    let session_ms = ms(t);
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
    let preview = session.preview(&settings, Control::none()).unwrap();
    let preview_ms = ms(t);

    let t = Instant::now();
    let applied = session.apply(&settings, &[], Control::none());
    let apply_ms = ms(t);

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
    let t = Instant::now();
    let drained = session.apply(&settings, &[drain], Control::none());
    let drained_ms = ms(t);

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
            "apply: {} triangles, edge-manifold {}, source {:.2} ml -> {:.2} ml (removed {:.2} ml)",
            a.mesh.triangle_count(),
            is_edge_manifold(&a.mesh),
            a.stats.source_volume_mm3 / 1000.0,
            a.stats.output_volume_mm3.unwrap() / 1000.0,
            a.stats.removed_volume_mm3.unwrap() / 1000.0,
        ),
        Err(e) => eprintln!("apply failed: {e}"),
    }
    match &drained {
        Ok(a) => eprintln!(
            "apply + drain: {} triangles, edge-manifold {}, shells {}, drain {:?}",
            a.mesh.triangle_count(),
            is_edge_manifold(&a.mesh),
            face_shells(&a.mesh).1,
            a.drains[0]
        ),
        Err(e) => eprintln!("apply + drain failed: {e}"),
    }
}
