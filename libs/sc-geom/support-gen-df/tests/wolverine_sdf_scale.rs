//! Scale check on a real model: build the three-mesh-bvh port and query the
//! SDF over Wolverine (1.9M triangles). Ignored by default, and skipped when
//! the private posed STL (`.local/df-oracle/corpus-wolverine/`) is absent.
//!
//!     cargo test -p support-gen-df --release --test wolverine_sdf_scale -- --ignored --nocapture

use std::sync::Arc;
use std::time::Instant;

use support_gen_df::supports::placement_logic::pathfinding::sdf_cache::SdfCache;
use support_gen_df::three::Matrix4;
use support_gen_df::three_mesh_bvh::MeshBvh;

fn read_binary_stl(path: &std::path::Path) -> Option<Vec<[f32; 3]>> {
    let data = std::fs::read(path).ok()?;
    let n = u32::from_le_bytes(data[80..84].try_into().ok()?) as usize;
    let mut positions = Vec::with_capacity(n * 3);
    for t in 0..n {
        let base = 84 + 50 * t + 12;
        for v in 0..3 {
            let at = base + v * 12;
            let f = |o: usize| f32::from_le_bytes(data[at + o..at + o + 4].try_into().unwrap());
            positions.push([f(0), f(4), f(8)]);
        }
    }
    Some(positions)
}

#[test]
#[ignore = "needs the private Wolverine STL; run with --ignored --release"]
fn wolverine_bvh_and_sdf_at_scale() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../.local/df-oracle/corpus-wolverine/wolverine-center-z10.stl");
    let Some(positions) = read_binary_stl(&path) else {
        eprintln!("skipped: {} not found", path.display());
        return;
    };
    let triangles = positions.len() / 3;

    let started = Instant::now();
    let bvh = Arc::new(MeshBvh::new(positions, None));
    let build = started.elapsed();

    let sdf = SdfCache::new(bvh, Matrix4::IDENTITY, Some(0.5));
    let started = Instant::now();
    let mut blocked = 0;
    let mut segments = 0;
    // A column every 2 mm across the model's footprint, down to the plate.
    for ix in -14..=14 {
        for iy in -12..=12 {
            let (x, y) = (f64::from(ix) * 2.0, f64::from(iy) * 2.0);
            segments += 1;
            if sdf.segment_blocked_xyz(x, y, 40.0, x, y, 0.0, 0.6) {
                blocked += 1;
            }
        }
    }
    let march = started.elapsed();
    let stats = sdf.stats();
    eprintln!(
        "wolverine: {triangles} triangles, BVH build {build:?}; {segments} vertical columns in {march:?} \
         ({blocked} blocked), {} cell reads, {} BVH queries, {} cells",
        stats.cell_reads,
        stats.bvh_queries,
        sdf.size()
    );
    assert!(blocked > 0 && blocked < segments);
}
