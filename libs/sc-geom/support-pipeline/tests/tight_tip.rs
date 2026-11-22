//! The tight pass of the trunk router (SoulCrafted, #466).
//!
//! A contact under a slab, with a shelf of the model 1 mm under it whose edge
//! passes 0.2 mm beside the contact: every tip within 30 degrees of the face runs
//! into the shelf, so the normal pass refuses the contact. The tip ends in a
//! ball and holds at any angle to the face, so the tight pass leans it further
//! out, past the shelf's edge, with every collision check the normal pass has.

use std::sync::Arc;

use support_placement::placement_logic::pathfinding::sdf_cache::SdfCache;
use support_placement::placement_logic::standard_placement::TrunkPlacementInput;
use support_placement::placement_logic_v3::smart_placement_v3::{
    RouterStats, SmartPlacementV3Context, SmartPlacementV3Input, calculate_smart_placement_v3,
};
use support_model::settings::types::SupportSettings;
use support_model::tips::DEFAULT_TIP_PROFILE;
use support_model::types::LimitationCode;
use support_geom::geom::{Matrix4, Vec3};
use support_geom::bvh::MeshBvh;

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

const CONTACT: Vec3 = Vec3::new(5.0, 5.0, 10.0);
const DOWN: Vec3 = Vec3::new(0.0, 0.0, -1.0);

fn place(triangles: Vec<[f32; 3]>) -> (Option<LimitationCode>, Vec3) {
    let mut settings = SupportSettings::default();
    settings.grid.enabled = false;
    let mut profile = DEFAULT_TIP_PROFILE;
    profile.length_mm = 2.3;
    let bvh = Arc::new(MeshBvh::new(triangles, None));
    let mut sdf = SdfCache::new(bvh, Matrix4::IDENTITY, Some(0.25));
    let result = calculate_smart_placement_v3(
        &SmartPlacementV3Input {
            placement: TrunkPlacementInput {
                tip_pos: CONTACT,
                tip_normal: DOWN,
                tip_profile: profile,
                roots_top_z: settings.roots.disk_height_mm + settings.roots.cone_height_mm,
                is_small_island: None,
            },
            model_id: "m",
            is_preview: Some(false),
        },
        &mut sdf,
        Matrix4::IDENTITY,
        &mut RouterStats::default(),
        &settings,
        SmartPlacementV3Context::default(),
    );
    (result.error, result.socket_pos)
}

/// How far the tip leans off the face's normal, in degrees.
fn lean_deg(socket: Vec3) -> f64 {
    let axis = socket.sub(CONTACT).normalize();
    axis.angle_to(DOWN).to_degrees()
}

#[test]
fn a_contact_beside_a_shelf_takes_a_leaning_tip_instead_of_a_refusal() {
    let mut model = closed_box([0.0, 0.0, 10.0], [10.0, 10.0, 12.0]);
    // The shelf: 1 mm under the slab, its edge 0.2 mm past the contact.
    model.extend(closed_box([0.0, 0.0, 6.0], [5.2, 10.0, 9.0]));

    let (error, socket) = place(model);
    assert_eq!(error, None, "placed");
    // The normal pass never leans a tip more than 30 degrees off the face: a
    // tip that does came from the tight pass.
    let lean = lean_deg(socket);
    assert!(
        lean > 30.0 + 1e-6,
        "leans past the normal pass's cap: {lean}"
    );
    assert!(
        lean <= 45.0 + 1e-6,
        "and no further than the tight pass's: {lean}"
    );
    assert!(
        socket.x > 5.2,
        "the socket clears the shelf's edge: {socket:?}"
    );
    assert!(socket.z < CONTACT.z, "below the contact: {socket:?}");
}

#[test]
fn a_contact_with_room_keeps_its_straight_tip() {
    // The slab alone: the straight tip clears, its full length.
    let (error, socket) = place(closed_box([0.0, 0.0, 10.0], [10.0, 10.0, 12.0]));
    assert_eq!(error, None);
    assert!(lean_deg(socket) < 1e-6);
    assert!(CONTACT.distance_to(socket) > 2.3 - 1e-6);
}
