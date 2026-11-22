//! What a hollow does to real models, for comparing two builds of the
//! engine: how thick the wall is where it is thinnest and on the whole, how
//! much is carved, whether the result closes, and how long it takes.
//!
//! ```text
//! SC_QUALITY_STLS="a.stl;b.stl" [SC_QUALITY_HEIGHT=90] [SC_QUALITY_WALL=2.0] \
//!   cargo test --release -p bread-hollow-engine --test main hollow_quality -- --ignored --nocapture
//! ```
//!
//! One line per model, tab separated, for a spreadsheet.

use std::time::Instant;

use bread_hollow_engine::bvh::Bvh;
use bread_hollow_engine::mesh::{IndexedMesh, Vec3};
use bread_hollow_engine::shapes::read_binary_stl;
use bread_hollow_engine::{Control, GridRequest, HollowSettings, Session};

fn posed(mut mesh: IndexedMesh, height: f32) -> IndexedMesh {
    let bbox = mesh.bbox();
    let centre = bbox.center();
    let scale = height / (bbox.max.z - bbox.min.z);
    for p in &mut mesh.positions {
        *p = Vec3::new(
            (p.x - centre.x) * scale,
            (p.y - centre.y) * scale,
            (p.z - centre.z) * scale,
        );
    }
    mesh
}

#[test]
#[ignore = "a report on local models, not a check"]
fn hollow_quality() {
    let Ok(list) = std::env::var("SC_QUALITY_STLS") else {
        eprintln!("set SC_QUALITY_STLS to binary STLs, separated by ;");
        return;
    };
    let height: f32 = std::env::var("SC_QUALITY_HEIGHT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(90.0);
    let wall: f32 = std::env::var("SC_QUALITY_WALL")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2.0);
    println!(
        "model\tvoxel\tsession_ms\tpreview_ms\tapply_ms\tcavity_ml\twall_min\twall_p1\twall_median\tunder_wall_pct\tcavity_tris\tclosed"
    );
    for path in list.split(';').filter(|p| !p.is_empty()) {
        let mesh = posed(
            read_binary_stl(&std::fs::read(path).expect("the STL")).expect("binary"),
            height,
        );
        let name = std::path::Path::new(path)
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let t = Instant::now();
        let session = match Session::build(mesh, &GridRequest::default(), None, Control::none()) {
            Ok(s) => s,
            Err(e) => {
                println!("{name}\tsession failed: {e}");
                continue;
            }
        };
        let session_ms = t.elapsed().as_secs_f64() * 1e3;
        let settings = HollowSettings {
            wall_thickness_mm: wall,
            ..HollowSettings::default()
        };
        let t = Instant::now();
        let preview = session.preview(&settings, Control::none()).unwrap();
        let preview_ms = t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        let applied = session.apply(&settings, &[], Control::none());
        let apply_ms = t.elapsed().as_secs_f64() * 1e3;
        let source = session.source_mesh();
        let bvh = Bvh::build(source);
        let mut thickness: Vec<f32> = preview
            .cavity
            .positions
            .iter()
            .filter_map(|&p| bvh.nearest_distance(source, p, 1e9))
            .collect();
        thickness.sort_by(f32::total_cmp);
        let at = |q: f64| thickness[((thickness.len() - 1) as f64 * q) as usize];
        let under = thickness.iter().filter(|&&t| t < wall - 0.1).count() as f64
            / thickness.len().max(1) as f64
            * 100.0;
        println!(
            "{name}\t{:.4}\t{session_ms:.0}\t{preview_ms:.0}\t{apply_ms:.0}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{under:.2}\t{}\t{}",
            session.grid().voxel_mm,
            -preview.cavity.signed_volume() / 1000.0,
            at(0.0),
            at(0.01),
            at(0.5),
            preview.cavity.triangle_count(),
            applied.is_ok(),
        );
    }
}
