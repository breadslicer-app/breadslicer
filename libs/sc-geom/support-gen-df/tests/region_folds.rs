//! A region's own faces, and the folds of a region (SoulCrafted, #500).
//!
//! The overhang scan names a region's triangles in the order of the soup it
//! reads; a ray's hit names its face in the order the tree's build left the
//! triangles in. A sampler given the one for the other reads tops and walls
//! of the model as the region's underside and misses the underside: the
//! pipeline renumbers the scan's regions by the tree's faces.
//!
//! A region is every down-facing face of a connected part of the model, so it
//! folds over itself in plan wherever one part hangs over another: the
//! underside of a raised hand over that of its arm. Each fold takes its
//! candidates.

use std::cell::RefCell;
use std::sync::Arc;

use support_gen_df::native::overhang::{OverhangScan, overhang_and_stability_from_soup};
use support_gen_df::pipeline::regions_on_tree_faces;
use support_gen_df::supports::auto_support::grid_placement::{SurfaceSampler, face_normal_at};
use support_gen_df::supports::auto_support::types::AutoSupportMesh;
use support_gen_df::supports::auto_support::zone_candidates::{
    ZoneInput, generate_zone_candidates,
};
use support_gen_df::supports::auto_support::zone_rules::{OverhangRule, SupportSize, ZoneRules};
use support_gen_df::supports::model_mesh::ModelMesh;
use support_gen_df::supports::placement_logic::pathfinding::sdf_cache::SdfCache;
use support_gen_df::three::Matrix4;
use support_gen_df::three_mesh_bvh::{BvhMesh, MeshBvh, Side};
use support_gen_df::volume_analysis::islands::types::DetectedIsland;
use support_gen_df::volume_analysis::islands::use_islands::overhang_region_to_island;

/// The 12 triangles of a closed box, each face looking out, its underside
/// first.
fn closed_box(lo: [f32; 3], hi: [f32; 3]) -> Vec<[f32; 3]> {
    let [x0, y0, z0] = lo;
    let [x1, y1, z1] = hi;
    let c = [
        [x0, y0, z0],
        [x1, y0, z0],
        [x1, y1, z0],
        [x0, y1, z0],
        [x0, y0, z1],
        [x1, y0, z1],
        [x1, y1, z1],
        [x0, y1, z1],
    ];
    let faces = [
        [0, 2, 1],
        [0, 3, 2],
        [4, 5, 6],
        [4, 6, 7],
        [0, 1, 5],
        [0, 5, 4],
        [1, 2, 6],
        [1, 6, 5],
        [2, 3, 7],
        [2, 7, 6],
        [3, 0, 4],
        [3, 4, 7],
    ];
    faces.iter().flat_map(|f| f.map(|i| c[i])).collect()
}

/// The walls of a C that opens towards +x, run along y: a foot from x 0 to
/// 10 with its underside at z 5, a back, and an arm from x 2 to 10 with its
/// underside at z 13. One strip of quads, so the two undersides are faces of
/// the same connected part.
fn c_shape() -> Vec<[f32; 3]> {
    let outline: [[f32; 2]; 8] = [
        [0.0, 5.0],
        [10.0, 5.0],
        [10.0, 7.0],
        [2.0, 7.0],
        [2.0, 13.0],
        [10.0, 13.0],
        [10.0, 15.0],
        [0.0, 15.0],
    ];
    let (y0, y1) = (0.0f32, 6.0f32);
    let mut out = Vec::new();
    for i in 0..outline.len() {
        let (a, b) = (outline[i], outline[(i + 1) % outline.len()]);
        // The outline runs anticlockwise seen from -y: each wall looks out.
        let quad = [
            [a[0], y0, a[1]],
            [b[0], y0, b[1]],
            [b[0], y1, b[1]],
            [a[0], y1, a[1]],
        ];
        out.extend([quad[0], quad[2], quad[1], quad[0], quad[3], quad[2]]);
    }
    out
}

fn soup_of(triangles: &[[f32; 3]]) -> Vec<f32> {
    triangles.iter().flatten().copied().collect()
}

/// The overhang regions of `triangles`, as the islands the plan reads: their
/// triangles named as the scan does with `bvh` left out, by the faces of
/// `bvh` with it.
fn regions(triangles: &[[f32; 3]], bvh: Option<&MeshBvh>) -> Vec<DetectedIsland> {
    let (regions, stability) =
        overhang_and_stability_from_soup(&soup_of(triangles), 45.0, 0.25, false);
    let scan = OverhangScan {
        regions,
        stability,
        steep_regions: Vec::new(),
    };
    let scan = match bvh {
        Some(bvh) => regions_on_tree_faces(scan, bvh),
        None => scan,
    };
    scan.regions
        .iter()
        .enumerate()
        .map(|(i, region)| overhang_region_to_island(region, i))
        .collect()
}

#[test]
fn a_face_of_the_tree_knows_its_triangle_of_the_soup() {
    // The far box first: the tree's build sorts it after the near one.
    let mut triangles = closed_box([100.0, 0.0, 5.0], [110.0, 10.0, 7.0]);
    triangles.extend(closed_box([0.0, 0.0, 5.0], [10.0, 10.0, 7.0]));
    let bvh = MeshBvh::new(triangles.clone(), None);
    let faces = bvh.faces_of_soup().expect("built from a soup");
    assert_eq!(faces.len(), 24);
    let mut moved = 0;
    for (triangle, &face) in faces.iter().enumerate() {
        moved += usize::from(face as usize != triangle);
        let corners = bvh.face(face as usize).map(|i| bvh.positions()[i as usize]);
        assert_eq!(corners, [0, 1, 2].map(|k| triangles[triangle * 3 + k]));
    }
    assert!(moved > 0, "the build left every triangle where it was");

    // A geometry with its own index does not say.
    let index = (0..triangles.len() as u32).collect();
    assert_eq!(MeshBvh::new(triangles, Some(index)).faces_of_soup(), None);
}

#[test]
fn a_region_is_sampled_on_its_own_faces() {
    let mut triangles = closed_box([100.0, 0.0, 5.0], [110.0, 10.0, 7.0]);
    triangles.extend(closed_box([0.0, 0.0, 5.0], [10.0, 10.0, 7.0]));
    let bvh = MeshBvh::new(triangles.clone(), None);
    let matrix = Matrix4::IDENTITY;
    let rays = BvhMesh {
        bvh: &bvh,
        matrix_world: &matrix,
    };
    // As the scan numbers them, the far box's underside is two faces of the
    // near box to the tree: the sampler finds nothing under the far box.
    let unsorted = regions(&triangles, None);
    let far = unsorted
        .iter()
        .find(|island| island.contact.x > 50.0)
        .expect("the far box's underside");
    let mut sampler = SurfaceSampler::triangle(far, Some(rays)).expect("its triangles");
    assert_eq!(sampler.at(far.contact.x, 5.0), None);

    let islands = regions(&triangles, Some(&bvh));
    assert_eq!(islands.len(), 2, "one underside each");
    for island in &islands {
        let x = island.contact.x;
        let mut sampler = SurfaceSampler::triangle(island, Some(rays)).expect("its triangles");
        let hit = sampler
            .at(x, 5.0)
            .expect("its underside is over its contact");
        assert!((hit.z - 5.0).abs() < 1e-6, "{hit:?}");
        let normal = face_normal_at(rays, hit.face_index.expect("a face")).expect("a normal");
        assert!(normal.z < -0.99, "the underside, not {normal:?}");
        // And nothing of the other box's.
        let other = if x > 50.0 { 5.0 } else { 105.0 };
        assert_eq!(sampler.at(other, 5.0), None);
    }
}

#[test]
fn a_region_folded_over_itself_takes_candidates_on_every_fold() {
    let triangles = c_shape();
    let bvh = Arc::new(MeshBvh::new(triangles.clone(), None));
    let matrix = Matrix4::IDENTITY;
    let rays = BvhMesh {
        bvh: &bvh,
        matrix_world: &matrix,
    };
    let islands = regions(&triangles, Some(&bvh));
    assert_eq!(islands.len(), 1, "both undersides are of one part");

    // Over x 6 the region has its foot's underside and its arm's.
    let mut sampler = SurfaceSampler::triangle(&islands[0], Some(rays)).expect("its triangles");
    let folds: Vec<f64> = sampler.all_at(6.0, 3.0).iter().map(|hit| hit.z).collect();
    assert_eq!(folds.len(), 2, "{folds:?}");
    assert!(
        (folds[0] - 5.0).abs() < 1e-6 && (folds[1] - 13.0).abs() < 1e-6,
        "{folds:?}"
    );
    assert!((sampler.at(6.0, 3.0).expect("the lowest").z - 5.0).abs() < 1e-6);

    let sdf = RefCell::new(SdfCache::new(bvh.clone(), matrix, Some(0.5)));
    let field = sdf.borrow();
    let rules = ZoneRules {
        sizes: vec![SupportSize {
            id: "small".into(),
            ..Default::default()
        }],
        overhang: OverhangRule {
            size: "small".into(),
            spacing_mm: 2.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let candidates = generate_zone_candidates(&ZoneInput {
        islands: &islands,
        steep: &[],
        rules: &rules,
        layer_height_mm: 0.05,
        mesh: Some(AutoSupportMesh {
            mesh: ModelMesh {
                field: &*field,
                rays,
            },
            side: Side::Front,
            blocked: None,
        }),
    });
    let on = |z: f64| {
        candidates
            .iter()
            .filter(|c| (c.tip_pos.z - z).abs() < 1e-6)
            .count()
    };
    // The foot is 10 by 6 mm, the arm 8 by 6: a tip every 2 mm on each.
    assert!(on(5.0) >= 12, "the foot: {}", on(5.0));
    assert!(on(13.0) >= 9, "the arm: {}", on(13.0));
    assert_eq!(on(5.0) + on(13.0), candidates.len(), "all on an underside");
    for c in &candidates {
        assert_eq!(c.grid_point, Some(true), "{}", c.id);
        assert!(c.tip_normal.z < -0.99, "{}: {:?}", c.id, c.tip_normal);
    }
    let mut ids: Vec<&str> = candidates.iter().map(|c| c.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), candidates.len(), "each its own id");
}
