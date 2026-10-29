//! Moving a model's supports with it (#119): every point of the supports
//! that stand on one model moves by the same amount, and directions
//! (normals, tangents, cone axes) stay as they are. DragonFruit keeps each
//! model's supports to themselves (a fan, a merge or a brace only joins
//! supports of the same model), so they move apart from the others whole.

use std::collections::HashSet;

use support_gen_df::supports::support_primitives::contact_cone::types::ContactCone;
use support_gen_df::supports::types::{
    BezierCurve, ContactDisk, Segment, SupportCollections, SupportEntity, Vec3,
};

fn shift(p: &mut Vec3, d: Vec3) {
    *p = p.add(d);
}

/// The curve's control points; its tangents are directions.
fn shift_curve(curve: &mut BezierCurve, d: Vec3) {
    shift(&mut curve.control_point1, d);
    shift(&mut curve.control_point2, d);
}

fn shift_segments(segments: &mut [Segment], d: Vec3) {
    for segment in segments {
        for joint in [&mut segment.top_joint, &mut segment.bottom_joint]
            .into_iter()
            .flatten()
        {
            shift(&mut joint.pos, d);
        }
        if let Some(curve) = segment.bezier_mut() {
            shift_curve(curve, d);
        }
    }
}

fn shift_cone(cone: &mut ContactCone, d: Vec3) {
    shift(&mut cone.pos, d);
}

fn shift_disk(disk: &mut ContactDisk, d: Vec3) {
    shift(&mut disk.pos, d);
}

/// Every entity of the collections, by its base record. Knots have none:
/// they belong to the shaft they sit on.
pub(crate) fn bases(c: &SupportCollections) -> impl Iterator<Item = &SupportEntity> {
    c.roots
        .values()
        .map(|e| &e.base)
        .chain(c.trunks.values().map(|e| &e.base))
        .chain(c.branches.values().map(|e| &e.base))
        .chain(c.leaves.values().map(|e| &e.base))
        .chain(c.twigs.values().map(|e| &e.base))
        .chain(c.sticks.values().map(|e| &e.base))
        .chain(c.braces.values().map(|e| &e.base))
        .chain(c.stumps.values().map(|e| &e.base))
        .chain(c.kickstands.values().map(|e| &e.base))
}

/// The ids a knot's `parent_shaft_id` may name on the entities `keep`
/// picks: an entity's own and its segments'.
fn shaft_ids(c: &SupportCollections, keep: impl Fn(&SupportEntity) -> bool) -> HashSet<String> {
    let mut ids: HashSet<String> = bases(c)
        .filter(|base| keep(base))
        .map(|base| base.id.clone())
        .collect();
    let segments = c
        .trunks
        .values()
        .filter(|e| keep(&e.base))
        .flat_map(|e| &e.segments)
        .chain(
            c.branches
                .values()
                .filter(|e| keep(&e.base))
                .flat_map(|e| &e.segments),
        )
        .chain(
            c.twigs
                .values()
                .filter(|e| keep(&e.base))
                .flat_map(|e| &e.segments),
        )
        .chain(
            c.sticks
                .values()
                .filter(|e| keep(&e.base))
                .flat_map(|e| &e.segments),
        )
        .chain(
            c.stumps
                .values()
                .filter(|e| keep(&e.base))
                .flat_map(|e| &e.segments),
        )
        .chain(
            c.kickstands
                .values()
                .filter(|e| keep(&e.base))
                .flat_map(|e| &e.segments),
        );
    ids.extend(segments.map(|s| s.id.clone()));
    ids
}

/// The knots the entities `keep` picks hang from or brace to.
fn knots_named(c: &SupportCollections, keep: impl Fn(&SupportEntity) -> bool) -> HashSet<String> {
    let mut ids = HashSet::new();
    ids.extend(
        c.branches
            .values()
            .filter(|e| keep(&e.base))
            .map(|e| e.parent_knot_id.clone()),
    );
    ids.extend(
        c.leaves
            .values()
            .filter(|e| keep(&e.base))
            .map(|e| e.parent_knot_id.clone()),
    );
    for brace in c.braces.values().filter(|e| keep(&e.base)) {
        ids.insert(brace.start_knot_id.clone());
        ids.insert(brace.end_knot_id.clone());
    }
    ids.extend(
        c.kickstands
            .values()
            .filter(|e| keep(&e.base))
            .map(|e| e.host_knot_id.clone()),
    );
    ids
}

/// Moves every support of the model `model_id` by `d`, knots included: a
/// knot goes with the shaft it sits on or, when no support holds that shaft,
/// with the supports that name it.
pub(crate) fn shift_model_supports(c: &mut SupportCollections, model_id: &str, d: Vec3) {
    let on = |base: &SupportEntity| base.model_id == model_id;
    let own_shafts = shaft_ids(c, on);
    let all_shafts = shaft_ids(c, |_| true);
    let own_knots = knots_named(c, on);
    for (id, knot) in &mut c.knots {
        let host = knot.parent_shaft_id.as_str();
        if own_shafts.contains(host) || (!all_shafts.contains(host) && own_knots.contains(id)) {
            shift(&mut knot.pos, d);
        }
    }
    for root in c.roots.values_mut().filter(|e| on(&e.base)) {
        shift(&mut root.transform.pos, d);
    }
    for trunk in c.trunks.values_mut().filter(|e| on(&e.base)) {
        shift_segments(&mut trunk.segments, d);
        if let Some(cone) = &mut trunk.contact_cone {
            shift_cone(cone, d);
        }
    }
    for branch in c.branches.values_mut().filter(|e| on(&e.base)) {
        shift_segments(&mut branch.segments, d);
        if let Some(cone) = &mut branch.contact_cone {
            shift_cone(cone, d);
        }
    }
    for leaf in c.leaves.values_mut().filter(|e| on(&e.base)) {
        shift_cone(&mut leaf.contact_cone, d);
    }
    for twig in c.twigs.values_mut().filter(|e| on(&e.base)) {
        shift_segments(&mut twig.segments, d);
        shift_disk(&mut twig.contact_disk_a, d);
        shift_disk(&mut twig.contact_disk_b, d);
    }
    for stick in c.sticks.values_mut().filter(|e| on(&e.base)) {
        shift_segments(&mut stick.segments, d);
        shift_cone(&mut stick.contact_cone_a, d);
        shift_cone(&mut stick.contact_cone_b, d);
    }
    for brace in c.braces.values_mut().filter(|e| on(&e.base)) {
        if let Some(curve) = &mut brace.curve {
            shift_curve(&mut curve.curve, d);
        }
    }
    for stump in c.stumps.values_mut().filter(|e| on(&e.base)) {
        shift(&mut stump.root_pos, d);
        shift(&mut stump.joint.pos, d);
        shift_segments(&mut stump.segments, d);
        shift_cone(&mut stump.contact_cone, d);
    }
    for kickstand in c.kickstands.values_mut().filter(|e| on(&e.base)) {
        shift_segments(&mut kickstand.segments, d);
    }
}
