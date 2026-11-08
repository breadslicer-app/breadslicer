//! A cavity bridge under a contact whose first landing is a slanted wall
//! (SoulCrafted, #445): the case of a part hanging in a hollow model, where
//! the cone at the foot of a stick follows the wall and leans the shaft past
//! what a stick stands, while flatter ground lies a step away.

use std::sync::Arc;

use support_gen_df::supports::model_mesh::ModelMesh;
use support_gen_df::supports::placement_logic::pathfinding::sdf_cache::SdfCache;
use support_gen_df::supports::settings::types::SupportSettings;
use support_gen_df::supports::support_types::contact_bridge::BridgeEntity;
use support_gen_df::supports::support_types::trunk::cavity_bridge::{
    BridgeAttempt, BridgeRefusal, attempt_cavity_bridge,
};
use support_gen_df::supports::types::Vec3;
use support_gen_df::three::Matrix4;
use support_gen_df::three_mesh_bvh::{BvhMesh, MeshBvh, Side};

/// The two triangles of a quad, counter-clockwise from where it looks.
fn quad(corners: [[f32; 3]; 4]) -> Vec<[f32; 3]> {
    let [a, b, c, d] = corners;
    vec![a, b, c, a, c, d]
}

/// A steep wall right under the origin's column, and flat ground beside it.
fn ground() -> Arc<MeshBvh> {
    // z = -2.14 x: its normal looks up at 65 degrees from vertical.
    let mut triangles = quad([
        [0.5, -4.0, -1.07],
        [0.5, 4.0, -1.07],
        [-0.5, 4.0, 1.07],
        [-0.5, -4.0, 1.07],
    ]);
    triangles.extend(quad([
        [1.0, -4.0, 0.0],
        [6.0, -4.0, 0.0],
        [6.0, 4.0, 0.0],
        [1.0, 4.0, 0.0],
    ]));
    Arc::new(MeshBvh::new(triangles, None))
}

fn attempt_with(every_landing: bool) -> BridgeAttempt {
    attempt_at(Vec3::new(0.0, 0.0, 6.2), every_landing)
}

fn attempt_at(tip: Vec3, every_landing: bool) -> BridgeAttempt {
    let bvh = ground();
    let matrix = Matrix4::IDENTITY;
    let sdf = SdfCache::new(bvh.clone(), matrix, Some(0.5));
    let mut settings = SupportSettings::default();
    settings.mesh_to_mesh.every_landing = every_landing;
    let mut next = 0;
    let mut new_id = || {
        next += 1;
        format!("id-{next}")
    };
    attempt_cavity_bridge(
        tip,
        Vec3::new(0.0, 0.0, -1.0),
        "m",
        ModelMesh {
            field: &sdf,
            rays: BvhMesh {
                bvh: &bvh,
                matrix_world: &matrix,
            },
        },
        Side::Front,
        None,
        &settings,
        &mut new_id,
    )
}

/// A contact read from the top of the ground, its normal turned over to look
/// down into it: nothing hangs there.
#[test]
fn a_contact_on_a_top_surface_takes_no_bridge() {
    let attempt = attempt_at(Vec3::new(3.0, 0.0, 0.0), true);
    assert!(attempt.bridge.is_none());
    assert_eq!(attempt.refusals, [BridgeRefusal::TopSurface]);
}

#[test]
fn the_first_landing_alone_is_turned_down_for_its_cant() {
    let attempt = attempt_with(false);
    assert!(attempt.bridge.is_none());
    assert_eq!(attempt.refusals, [BridgeRefusal::Canted]);
}

#[test]
fn a_bridge_the_first_landing_turns_down_lands_on_the_ground_beside_it() {
    let attempt = attempt_with(true);
    assert_eq!(attempt.refusals.first(), Some(&BridgeRefusal::Canted));
    let bridge = attempt.bridge.expect("a landing on the flat ground");
    let BridgeEntity::Stick(stick) = bridge.entity else {
        panic!("a span this long is a stick");
    };
    let foot = stick.contact_cone_a.pos.z.min(stick.contact_cone_b.pos.z);
    assert!(foot.abs() < 1e-6, "it stands on the ground, at {foot}");
}

/// Over a gap narrower than where the search starts, that start is under the
/// ground: the ground right under the tip is the landing.
#[test]
fn a_bridge_over_a_narrow_gap_lands_right_under_the_tip() {
    let tip = Vec3::new(3.0, 0.0, 0.45);
    assert!(attempt_at(tip, false).bridge.is_none());
    let bridge = attempt_at(tip, true)
        .bridge
        .expect("a landing under the tip");
    let BridgeEntity::Twig(twig) = bridge.entity else {
        panic!("a span this short is a twig");
    };
    let (a, b) = (twig.contact_disk_a.pos, twig.contact_disk_b.pos);
    let foot = if a.z < b.z { a } else { b };
    assert!(foot.z.abs() < 1e-6, "it stands on the ground, at {foot:?}");
    assert!((foot.x - tip.x).abs() < 1e-6 && (foot.y - tip.y).abs() < 1e-6);
}

/// In less air than the two tips of a bridge sink, a bridge would fill the gap.
#[test]
fn a_slot_of_the_model_takes_no_bridge() {
    assert!(attempt_at(Vec3::new(3.0, 0.0, 0.3), true).bridge.is_none());
}
