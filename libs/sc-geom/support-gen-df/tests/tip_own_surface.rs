//! A candidate on a surface that faces up, under one that hangs 18 mm above
//! it (SoulCrafted, #500): the case of a mini's curled fingers over its body.
//! DragonFruit's search takes the hanging surface; with the tip kept on its
//! own surface, the search finds the candidate's.

use std::cell::RefCell;
use std::sync::Arc;

use support_gen_df::supports::auto_support::auto_place::helpers::{
    PlanMesh, resolve_surface_normal,
};
use support_gen_df::supports::placement_logic::pathfinding::sdf_cache::SdfCache;
use support_gen_df::three::buffer_geometry::BufferGeometry;
use support_gen_df::three::{Matrix4, Vec3};
use support_gen_df::three_mesh_bvh::{MeshBvh, Side};

/// The 12 triangles of a closed box, each face looking out.
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

#[test]
fn a_candidate_on_a_surface_that_faces_up_keeps_to_it() {
    // A body whose top faces up at z 2, and a slab hanging over it whose
    // underside faces down at z 20.
    let mut triangles = closed_box([0.0, 0.0, 0.0], [10.0, 10.0, 2.0]);
    triangles.extend(closed_box([0.0, 0.0, 20.0], [10.0, 10.0, 22.0]));
    let soup: Vec<f32> = triangles.iter().flatten().copied().collect();
    let mut geometry = BufferGeometry {
        position: Some(soup),
        ..Default::default()
    };
    geometry.compute_vertex_normals();
    let normals: Vec<[f32; 3]> = geometry
        .normal
        .as_ref()
        .expect("normals")
        .as_chunks::<3>()
        .0
        .iter()
        .map(|c| [c[0], c[1], c[2]])
        .collect();
    let bvh = Arc::new(MeshBvh::new(triangles, None));
    let sdf = RefCell::new(SdfCache::new(bvh.clone(), Matrix4::IDENTITY, Some(0.5)));
    let mesh = PlanMesh {
        bvh: &bvh,
        matrix: Matrix4::IDENTITY,
        side: Side::Front,
        normals: Some(&normals),
        sdf: &sdf,
        blocked: None,
    };
    let on_top = Vec3::new(5.0, 5.0, 2.0);

    // DragonFruit's search: the slab 18 mm up.
    let (point, normal) = resolve_surface_normal(on_top, Some(&mesh), false);
    assert!((point.z - 20.0).abs() < 1e-6, "{point:?}");
    assert!(normal.z < -0.9, "the slab's underside: {normal:?}");

    // Kept on its own surface: the top it is on, its normal turned down, so
    // a support would come up through the body and be refused.
    let (point, normal) = resolve_surface_normal(on_top, Some(&mesh), true);
    assert!((point.z - 2.0).abs() < 1e-6, "{point:?}");
    assert!(normal.z < -0.9, "{normal:?}");

    // A candidate on the slab's underside finds it either way.
    let under = Vec3::new(5.0, 5.0, 20.0);
    for own in [false, true] {
        let (point, normal) = resolve_surface_normal(under, Some(&mesh), own);
        assert!((point.z - 20.0).abs() < 1e-6, "{point:?}");
        assert!(normal.z < -0.9, "{normal:?}");
    }
}
