//! A contact inside the closed cavity of a hollow model (SoulCrafted, #632):
//! what tells it from the outside and from a part sunk in the material.

use std::sync::Arc;

use support_engine::supports::auto_support::types::AutoSupportMesh;
use support_engine::supports::auto_support::zone_candidates::is_enclosed;
use support_engine::supports::model_mesh::ModelMesh;
use support_engine::supports::placement_logic::pathfinding::sdf_cache::SdfCache;
use support_engine::supports::types::Vec3;
use support_engine::three::Matrix4;
use support_engine::three_mesh_bvh::{BvhMesh, MeshBvh, Side};

/// The 12 triangles of the box from `lo` to `hi`, looking out, or in when
/// `inward` (a cavity's wall).
fn cube(lo: [f32; 3], hi: [f32; 3], inward: bool) -> Vec<[f32; 3]> {
    let c = |i: usize| {
        [
            if i & 1 == 0 { lo[0] } else { hi[0] },
            if i & 2 == 0 { lo[1] } else { hi[1] },
            if i & 4 == 0 { lo[2] } else { hi[2] },
        ]
    };
    let faces = [
        [0, 2, 1],
        [1, 2, 3],
        [4, 5, 6],
        [5, 7, 6],
        [0, 1, 4],
        [1, 5, 4],
        [2, 6, 3],
        [3, 6, 7],
        [0, 4, 2],
        [2, 4, 6],
        [1, 3, 5],
        [3, 7, 5],
    ];
    faces
        .iter()
        .flat_map(|f| {
            let t = [c(f[0]), c(f[1]), c(f[2])];
            if inward { [t[0], t[2], t[1]] } else { t }
        })
        .collect()
}

#[test]
fn a_contact_in_a_closed_cavity_is_enclosed_and_none_outside_is() {
    // A 30 mm box hollowed to a 20 mm cavity, and a small block sunk in its
    // wall, overlapping the material.
    let mut triangles = cube([0.0; 3], [30.0; 3], false);
    triangles.extend(cube([5.0; 3], [25.0; 3], true));
    triangles.extend(cube([1.0, 1.0, 1.0], [3.0, 3.0, 3.0], false));
    let bvh = Arc::new(MeshBvh::new(triangles, None));
    let matrix = Matrix4::IDENTITY;
    let sdf = SdfCache::new(bvh.clone(), matrix, Some(0.5));
    let mesh = AutoSupportMesh {
        mesh: ModelMesh {
            field: &sdf,
            rays: BvhMesh {
                bvh: &bvh,
                matrix_world: &matrix,
            },
        },
        side: Side::Double,
        blocked: None,
    };
    let down = Vec3::new(0.0, 0.0, -1.0);
    // The cavity's ceiling.
    assert!(is_enclosed(mesh, Vec3::new(15.0, 15.0, 25.0), down));
    assert!(is_enclosed(mesh, Vec3::new(6.0, 23.0, 25.0), down));
    // The box's underside, over the plate.
    assert!(!is_enclosed(mesh, Vec3::new(15.0, 15.0, 0.0), down));
    // The sunk block's top, inside the material of the wall.
    assert!(!is_enclosed(
        mesh,
        Vec3::new(2.0, 2.0, 3.0),
        Vec3::new(0.0, 0.0, 1.0)
    ));
}

#[test]
fn a_cavity_open_to_the_side_is_not_enclosed() {
    // The same box, its cavity open through the +x wall: a pocket.
    let triangles = cube([0.0; 3], [30.0; 3], false);
    let walls: Vec<[f32; 3]> = cube([5.0; 3], [25.0; 3], true);
    // Drop the cavity's +x wall and the outer +x wall: the pocket opens.
    let keep = |t: &[[f32; 3]]| !t.iter().all(|v| v[0] == 25.0) && !t.iter().all(|v| v[0] == 30.0);
    let mut soup: Vec<[f32; 3]> = Vec::new();
    for tri in triangles.chunks(3).chain(walls.chunks(3)) {
        if keep(tri) {
            soup.extend_from_slice(tri);
        }
    }
    let bvh = Arc::new(MeshBvh::new(soup, None));
    let matrix = Matrix4::IDENTITY;
    let sdf = SdfCache::new(bvh.clone(), matrix, Some(0.5));
    let mesh = AutoSupportMesh {
        mesh: ModelMesh {
            field: &sdf,
            rays: BvhMesh {
                bvh: &bvh,
                matrix_world: &matrix,
            },
        },
        side: Side::Double,
        blocked: None,
    };
    assert!(!is_enclosed(
        mesh,
        Vec3::new(15.0, 15.0, 25.0),
        Vec3::new(0.0, 0.0, -1.0)
    ));
}
