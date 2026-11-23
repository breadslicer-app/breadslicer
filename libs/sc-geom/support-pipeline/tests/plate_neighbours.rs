//! A plate of models as one (#725): supports keep clear of the models
//! standing near their own.
//!
//! A slab 2 mm thick stands 10 mm up, and a block (another model, on the
//! plate) stands under the middle of it: supports made for the slab alone
//! come straight down through the block. Planned with the block beside it,
//! or settled once made, they go round it.

use std::collections::HashSet;
use std::sync::Arc;

use bread_support_geom::bvh::MeshBvh;
use bread_support_geom::geom::buffer_geometry::BufferGeometry;
use bread_support_geom::geom::{Matrix4, Vec3};
use bread_support_mesh::rafts::crenelated::geometry::model_plate_footprint::ModelTransform;
use bread_support_model::collision_avoidance::CollisionField;
use bread_support_model::types::{SupportCollections, SupportState};
use bread_support_pipeline::plate::{
    NEIGHBOUR_CLEARANCE_MM, PlateModel, SettleInput, as_hosts, crowding, settle_crowded,
};
use bread_support_pipeline::{
    CancelToken, ModelInput, PipelineOutput, PipelineSettings, plan_supports_with, world_soup,
};
use bread_support_placement::placement_logic::pathfinding::sdf_cache::SdfCache;

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

/// A box model standing from `lo` to `hi` in the world.
fn box_model(id: &str, lo: [f32; 3], hi: [f32; 3]) -> ModelInput {
    let soup: Vec<f32> = closed_box(lo, hi).into_iter().flatten().collect();
    let centre = Vec3::new(
        f64::from(lo[0] + hi[0]) / 2.0,
        f64::from(lo[1] + hi[1]) / 2.0,
        f64::from(lo[2] + hi[2]) / 2.0,
    );
    ModelInput {
        id: id.to_owned(),
        geometry: Arc::new(BufferGeometry {
            position: Some(soup),
            ..Default::default()
        }),
        transform: ModelTransform {
            position: centre,
            rotation: Vec3::ZERO,
            scale: Vec3::new(1.0, 1.0, 1.0),
        },
        visible: true,
    }
}

/// A model of the plate, from its world soup.
fn plate_model(model: &ModelInput) -> PlateModel {
    let soup = world_soup(model);
    let triples: Vec<[f32; 3]> = soup.as_chunks::<3>().0.to_vec();
    PlateModel::new(
        model.id.clone(),
        Arc::new(MeshBvh::new(triples, None)),
        Matrix4::IDENTITY,
    )
}

fn slab() -> ModelInput {
    box_model("slab", [0.0, 0.0, 10.0], [30.0, 10.0, 12.0])
}

fn block() -> ModelInput {
    box_model("block", [10.0, 2.0, 0.0], [20.0, 8.0, 6.0])
}

fn plan(models: &[ModelInput]) -> PipelineOutput {
    let mut next = 0u64;
    plan_supports_with(
        models,
        &SupportState::default(),
        &PipelineSettings::defaults(0.05),
        None,
        &mut || {
            next += 1;
            format!("p-{next}")
        },
        &mut |_| {},
        &CancelToken::new(),
    )
    .expect("the plan")
}

/// The supports of `model_id` in `c` that come too near `others`.
fn crowded(c: &SupportCollections, model_id: &str, others: &[&PlateModel]) -> Vec<String> {
    crowding(c, &|_, m| m == model_id, others, NEIGHBOUR_CLEARANCE_MM)
        .into_iter()
        .map(|c| c.id)
        .collect()
}

/// Every contact of the supports of `c`, rounded to a micron.
fn contacts(c: &SupportCollections) -> HashSet<[i64; 3]> {
    let key = |p: Vec3| [p.x, p.y, p.z].map(|v| (v * 1000.0).round() as i64);
    c.trunks
        .values()
        .filter_map(|t| t.contact_cone.as_ref())
        .chain(c.branches.values().filter_map(|b| b.contact_cone.as_ref()))
        .chain(c.leaves.values().map(|l| &l.contact_cone))
        .map(|cone| key(cone.pos))
        .collect()
}

#[test]
fn a_field_reads_the_models_near_its_own() {
    let own = Arc::new(MeshBvh::new(closed_box([0.0; 3], [10.0; 3]), None));
    let near = Arc::new(MeshBvh::new(
        closed_box([12.0, 0.0, 0.0], [22.0, 10.0, 10.0]),
        None,
    ));
    let alone = SdfCache::new(own.clone(), Matrix4::IDENTITY, Some(0.25));
    let field = SdfCache::new(own, Matrix4::IDENTITY, Some(0.25))
        .with_obstacles([(near, Matrix4::IDENTITY)], 0.2);
    assert_eq!(field.obstacle_count(), 1);
    // 1.5 mm from its own model, 0.5 from the neighbour grown by 0.2.
    assert!((alone.exact_signed_distance_at(11.5, 5.0, 5.0) - 1.5).abs() < 1e-9);
    assert!((field.exact_signed_distance_at(11.5, 5.0, 5.0) - 0.3).abs() < 1e-9);
    // Inside the neighbour, inside its own: below zero either way.
    assert!(field.exact_signed_distance_at(15.0, 5.0, 5.0) < 0.0);
    assert!(field.exact_signed_distance_at(5.0, 5.0, 5.0) < 0.0);
    // The neighbour alone, from inside its own model.
    assert!((field.obstacle_distance(Vec3::new(5.0, 5.0, 5.0), f64::INFINITY) - 6.8).abs() < 1e-9);
    assert_eq!(
        field.obstacle_distance(Vec3::new(5.0, 5.0, 5.0), 3.0),
        f64::INFINITY
    );
    // A shaft 0.3 mm off the neighbour: clear of its own model, not of the
    // plate.
    let (a, b) = (Vec3::new(11.7, 5.0, 0.0), Vec3::new(11.7, 5.0, 9.0));
    assert!(!CollisionField::segment_blocked(&alone, a, b, 0.4));
    assert!(CollisionField::segment_blocked(&field, a, b, 0.4));
    assert!(field.obstacles_block(a, b, 0.4));
    // Above its own model, far from the neighbour: clear.
    let (c, d) = (Vec3::new(5.0, 5.0, 11.0), Vec3::new(5.0, 5.0, 20.0));
    assert!(!field.obstacles_block(c, d, 0.4));
    assert!(!CollisionField::segment_blocked(&field, c, d, 0.4));
}

#[test]
fn supports_made_alone_go_through_the_block() {
    let out = plan(&[slab()]);
    let block = plate_model(&block());
    let through = crowded(&out.support.collections, "slab", &[&block]);
    assert!(!through.is_empty(), "the scene must crowd the block");
}

#[test]
fn models_planned_together_keep_clear_of_each_other() {
    let (slab, block) = (slab(), block());
    let out = plan(&[slab.clone(), block.clone()]);
    let (s, b) = (plate_model(&slab), plate_model(&block));
    let c = &out.support.collections;
    assert!(
        crowded(c, "slab", &[&b]).is_empty(),
        "{:?}",
        crowded(c, "slab", &[&b])
    );
    assert!(crowded(c, "block", &[&s]).is_empty());
    assert!(!contacts(c).is_empty());
}

/// Two slabs planned in one run each keep their own supports: the second
/// model's final check reads the first as a neighbour, not as a fault of
/// the first's supports.
#[test]
fn models_planned_together_each_keep_their_supports() {
    let a = box_model("a", [0.0, 0.0, 10.0], [30.0, 10.0, 12.0]);
    let b = box_model("b", [0.0, 11.0, 10.0], [30.0, 21.0, 12.0]);
    let alone_a = contacts(&plan(&[a.clone()]).support.collections).len();
    let alone_b = contacts(&plan(&[b.clone()]).support.collections).len();
    let together = plan(&[a, b]);
    let c = &together.support.collections;
    let of = |model: &str| {
        let mut one = c.clone();
        one.trunks.retain(|_, t| t.base.model_id == model);
        one.branches.retain(|_, t| t.base.model_id == model);
        one.leaves.retain(|_, t| t.base.model_id == model);
        contacts(&one).len()
    };
    // A tip on one slab holds nothing of the other: the second keeps the
    // contacts it has alone.
    assert_eq!(of("a"), alone_a);
    assert_eq!(of("b"), alone_b);
}

#[test]
fn crowded_supports_are_placed_again_round_the_neighbour() {
    let alone = plan(&[slab()]);
    let (slab, block) = (plate_model(&slab()), plate_model(&block()));
    let state = &alone.support;
    let through: HashSet<String> = crowded(&state.collections, "slab", &[&block])
        .into_iter()
        .collect();
    assert!(!through.is_empty());
    let mut next = 0u64;
    let settled = settle_crowded(
        &SettleInput {
            model: &slab,
            others: &[&block],
            state,
            hosts: None,
            crowded: &through,
            settings: &PipelineSettings::defaults(0.05),
        },
        &mut || {
            next += 1;
            format!("s-{next}")
        },
    );
    let c = &settled.state.collections;
    assert!(
        crowded(c, "slab", &[&block]).is_empty(),
        "{:?}",
        crowded(c, "slab", &[&block])
    );
    assert!(settled.taken >= through.len());
    assert!(settled.placed > 0, "{settled:?}");
    // What did not crowd the block stays as it was.
    for (id, trunk) in &state.collections.trunks {
        if !through.contains(id) && settled.state.collections.trunks.contains_key(id) {
            assert_eq!(&settled.state.collections.trunks[id], trunk);
        }
    }
    // Every contact is held again or counted lost.
    let before = contacts(&state.collections);
    let after = contacts(c);
    let missing = before.difference(&after).count();
    assert_eq!(missing, settled.lost.len(), "{settled:?}");
}

#[test]
fn a_neighbour_offers_its_trunks_under_ids_of_its_own() {
    let out = plan(&[slab()]);
    let c = &out.support.collections;
    let hosts = as_hosts(c, "@7");
    assert_eq!(hosts.trunks.len(), c.trunks.len());
    assert!(hosts.trunks.keys().all(|id| id.ends_with("@7")));
    assert!(hosts.trunks.values().all(|t| t.base.model_id == "slab@7"
        && t.root_id.ends_with("@7")
        && hosts.roots.contains_key(&t.root_id)
        && t.segments.iter().all(|s| s.id.ends_with("@7"))));
    assert!(
        hosts
            .knots
            .values()
            .all(|k| k.parent_shaft_id.ends_with("@7"))
    );
    assert!(hosts.braces.is_empty());
}
