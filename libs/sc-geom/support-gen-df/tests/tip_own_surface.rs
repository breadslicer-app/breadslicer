//! Tips that stay where they belong (SoulCrafted, #500).
//!
//! A candidate on a surface that faces up, under one that hangs 18 mm above
//! it: the case of a mini's curled fingers over its body. DragonFruit's
//! search takes the hanging surface; with the tip kept on its own surface,
//! the search finds the candidate's.
//!
//! A tip cone that goes through another part of the model on its way to its
//! contact (a claw under a finger, a jaw in front of a tooth), with its axis
//! or with its side, or starts inside the model, crosses it; one in the open
//! does not, nor one that leans on a flat surface.

use std::cell::RefCell;
use std::sync::Arc;

use support_gen_df::supports::auto_support::auto_place::helpers::{
    PlanMesh, contact_cone_crosses_model, resolve_surface_normal,
};
use support_gen_df::supports::placement_logic::pathfinding::sdf_cache::SdfCache;
use support_gen_df::supports::support_primitives::contact_cone::types::{
    ContactCone, DEFAULT_TIP_PROFILE,
};
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

/// What a plan sees of `triangles`.
struct Model {
    bvh: Arc<MeshBvh>,
    normals: Vec<[f32; 3]>,
    sdf: RefCell<SdfCache>,
}

impl Model {
    fn of(triangles: Vec<[f32; 3]>) -> Self {
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
        Model { bvh, normals, sdf }
    }

    fn mesh(&self) -> PlanMesh<'_> {
        PlanMesh {
            bvh: &self.bvh,
            matrix: Matrix4::IDENTITY,
            side: Side::Front,
            normals: Some(&self.normals),
            sdf: &self.sdf,
            blocked: None,
        }
    }
}

#[test]
fn a_tip_cone_through_another_part_of_the_model_crosses_it() {
    // A tip under a slab whose underside faces down at z 10, its cone
    // reaching down from the contact.
    let cone = ContactCone {
        pos: Vec3::new(5.0, 5.0, 10.0),
        normal: Vec3::new(0.0, 0.0, -1.0),
        profile: DEFAULT_TIP_PROFILE,
        ..Default::default()
    };
    let slab = closed_box([0.0, 0.0, 10.0], [10.0, 10.0, 12.0]);
    let open = Model::of(slab.clone());
    assert!(
        !contact_cone_crosses_model(&cone, &open.mesh()),
        "in the open"
    );

    // A claw 0.5 mm thick, 1 mm under the contact.
    let mut claw = slab.clone();
    claw.extend(closed_box([4.0, 4.0, 8.5], [6.0, 6.0, 9.0]));
    let clawed = Model::of(claw);
    assert!(
        contact_cone_crosses_model(&cone, &clawed.mesh()),
        "through the claw"
    );

    // Its socket inside a body under the slab: the cone starts in the model.
    let mut body = slab.clone();
    body.extend(closed_box([2.0, 2.0, 4.0], [8.0, 8.0, 8.0]));
    let buried = Model::of(body);
    assert!(
        contact_cone_crosses_model(&cone, &buried.mesh()),
        "from inside"
    );

    // A bump of the surface around the contact, 0.3 mm deep, is its own.
    let mut bump = slab;
    bump.extend(closed_box([4.5, 4.5, 9.7], [5.5, 5.5, 10.0]));
    let bumpy = Model::of(bump);
    assert!(!contact_cone_crosses_model(&cone, &bumpy.mesh()), "a bump");

    // The end of a claw beside the cone's axis, 1.6 mm under the contact,
    // 0.09 mm into the cone's side: the axis passes it by.
    let mut beside = closed_box([0.0, 0.0, 10.0], [10.0, 10.0, 12.0]);
    beside.extend(closed_box([5.35, 4.8, 8.0], [5.9, 5.2, 8.4]));
    let grazed = Model::of(beside);
    assert!(
        contact_cone_crosses_model(&cone, &grazed.mesh()),
        "its side into a claw"
    );
    // The same claw clear of the cone.
    let mut clear = closed_box([0.0, 0.0, 10.0], [10.0, 10.0, 12.0]);
    clear.extend(closed_box([5.7, 4.8, 8.0], [6.2, 5.2, 8.4]));
    let passed = Model::of(clear);
    assert!(
        !contact_cone_crosses_model(&cone, &passed.mesh()),
        "beside it"
    );

    // A cone that leans 50 degrees from the normal of a flat surface meets
    // it with its low side just before its contact, and is no crossing.
    let lean = 50f64.to_radians();
    let leaning = ContactCone {
        normal: Vec3::new(lean.sin(), 0.0, -lean.cos()),
        surface_normal: Some(Vec3::new(0.0, 0.0, -1.0)),
        ..cone.clone()
    };
    assert!(
        !contact_cone_crosses_model(&leaning, &open.mesh()),
        "leaning"
    );
}

#[test]
fn a_candidate_on_a_surface_that_faces_up_keeps_to_it() {
    // A body whose top faces up at z 2, and a slab hanging over it whose
    // underside faces down at z 20.
    let mut triangles = closed_box([0.0, 0.0, 0.0], [10.0, 10.0, 2.0]);
    triangles.extend(closed_box([0.0, 0.0, 20.0], [10.0, 10.0, 22.0]));
    let model = Model::of(triangles);
    let mesh = model.mesh();
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
