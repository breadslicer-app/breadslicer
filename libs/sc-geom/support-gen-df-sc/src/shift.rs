//! Moving a model's supports with it in the plate plane: in X/Y (#119), and
//! turned about the vertical axis (#153). Every point of the supports that
//! stand on one model goes where the motion takes it; directions (normals,
//! tangents, cone axes) only turn with it, and Z stays. DragonFruit keeps
//! each model's supports to themselves (a fan, a merge or a brace only joins
//! supports of the same model), so they move apart from the others whole.

use std::collections::HashSet;

use support_gen_df::supports::support_primitives::contact_cone::types::ContactCone;
use support_gen_df::supports::types::{
    BezierCurve, ContactDisk, Quaternion, Segment, SupportCollections, SupportEntity, Vec3,
};

/// A rigid motion of the plate plane: a turn about the Z axis through the
/// origin, then a move in X/Y.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PlaneMotion {
    /// The turn's angle and its cosine and sine; `None` for a plain move,
    /// which leaves directions exactly as they were.
    turn: Option<(f64, f64, f64)>,
    d: Vec3,
}

impl PlaneMotion {
    /// A move by `d` in X/Y.
    pub(crate) fn shift(d: [f64; 2]) -> Self {
        PlaneMotion {
            turn: None,
            d: Vec3::new(d[0], d[1], 0.0),
        }
    }

    /// A turn by `angle` radians about the Z axis, then a move by `d`.
    pub(crate) fn turn(angle: f64, d: [f64; 2]) -> Self {
        let (s, c) = angle.sin_cos();
        PlaneMotion {
            turn: Some((angle, c, s)),
            d: Vec3::new(d[0], d[1], 0.0),
        }
    }

    fn direction(&self, v: Vec3) -> Vec3 {
        match self.turn {
            None => v,
            Some((_, c, s)) => Vec3::new(c * v.x - s * v.y, s * v.x + c * v.y, v.z),
        }
    }

    pub(crate) fn point(&self, p: Vec3) -> Vec3 {
        self.direction(p).add(self.d)
    }

    fn rotation(&self, q: Quaternion) -> Quaternion {
        match self.turn {
            None => q,
            Some((angle, ..)) => {
                Quaternion::set_from_axis_angle(Vec3::new(0.0, 0.0, 1.0), angle).multiply(q)
            }
        }
    }
}

fn go(p: &mut Vec3, m: &PlaneMotion) {
    *p = m.point(*p);
}

fn turn(v: &mut Vec3, m: &PlaneMotion) {
    *v = m.direction(*v);
}

fn move_curve(curve: &mut BezierCurve, m: &PlaneMotion) {
    go(&mut curve.control_point1, m);
    go(&mut curve.control_point2, m);
    turn(&mut curve.start_tangent, m);
    turn(&mut curve.end_tangent, m);
}

fn move_segments(segments: &mut [Segment], m: &PlaneMotion) {
    for segment in segments {
        for joint in [&mut segment.top_joint, &mut segment.bottom_joint]
            .into_iter()
            .flatten()
        {
            go(&mut joint.pos, m);
        }
        if let Some(curve) = segment.bezier_mut() {
            move_curve(curve, m);
        }
    }
}

fn move_cone(cone: &mut ContactCone, m: &PlaneMotion) {
    go(&mut cone.pos, m);
    turn(&mut cone.normal, m);
    if let Some(normal) = &mut cone.surface_normal {
        turn(normal, m);
    }
}

fn move_disk(disk: &mut ContactDisk, m: &PlaneMotion) {
    go(&mut disk.pos, m);
    turn(&mut disk.surface_normal, m);
    turn(&mut disk.cone_axis, m);
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

/// The knots that belong to the supports of the model `model_id`: a knot
/// goes with the shaft it sits on or, when no support holds that shaft, with
/// the supports that name it.
fn model_knots(c: &SupportCollections, model_id: &str) -> HashSet<String> {
    let on = |base: &SupportEntity| base.model_id == model_id;
    let own_shafts = shaft_ids(c, on);
    let all_shafts = shaft_ids(c, |_| true);
    let own_knots = knots_named(c, on);
    c.knots
        .iter()
        .filter(|(id, knot)| {
            let host = knot.parent_shaft_id.as_str();
            own_shafts.contains(host) || (!all_shafts.contains(host) && own_knots.contains(*id))
        })
        .map(|(id, _)| id.clone())
        .collect()
}

/// The supports of the model `model_id` alone, knots included (#197).
pub(crate) fn model_collections(c: &SupportCollections, model_id: &str) -> SupportCollections {
    let knots = model_knots(c, model_id);
    let mut out = c.clone();
    let on = |base: &SupportEntity| base.model_id == model_id;
    out.roots.retain(|_, e| on(&e.base));
    out.trunks.retain(|_, e| on(&e.base));
    out.branches.retain(|_, e| on(&e.base));
    out.leaves.retain(|_, e| on(&e.base));
    out.twigs.retain(|_, e| on(&e.base));
    out.sticks.retain(|_, e| on(&e.base));
    out.braces.retain(|_, e| on(&e.base));
    out.stumps.retain(|_, e| on(&e.base));
    out.kickstands.retain(|_, e| on(&e.base));
    out.knots.retain(|id, _| knots.contains(id));
    out
}

/// Takes every support of the model `model_id` along by `m`, knots included
/// ([`model_knots`]).
pub(crate) fn move_model_supports(c: &mut SupportCollections, model_id: &str, m: &PlaneMotion) {
    let on = |base: &SupportEntity| base.model_id == model_id;
    let own_knots = model_knots(c, model_id);
    for (id, knot) in &mut c.knots {
        if own_knots.contains(id) {
            go(&mut knot.pos, m);
        }
    }
    for root in c.roots.values_mut().filter(|e| on(&e.base)) {
        go(&mut root.transform.pos, m);
        root.transform.rot = m.rotation(root.transform.rot);
    }
    for trunk in c.trunks.values_mut().filter(|e| on(&e.base)) {
        move_segments(&mut trunk.segments, m);
        if let Some(cone) = &mut trunk.contact_cone {
            move_cone(cone, m);
        }
    }
    for branch in c.branches.values_mut().filter(|e| on(&e.base)) {
        move_segments(&mut branch.segments, m);
        if let Some(cone) = &mut branch.contact_cone {
            move_cone(cone, m);
        }
    }
    for leaf in c.leaves.values_mut().filter(|e| on(&e.base)) {
        move_cone(&mut leaf.contact_cone, m);
    }
    for twig in c.twigs.values_mut().filter(|e| on(&e.base)) {
        move_segments(&mut twig.segments, m);
        move_disk(&mut twig.contact_disk_a, m);
        move_disk(&mut twig.contact_disk_b, m);
    }
    for stick in c.sticks.values_mut().filter(|e| on(&e.base)) {
        move_segments(&mut stick.segments, m);
        move_cone(&mut stick.contact_cone_a, m);
        move_cone(&mut stick.contact_cone_b, m);
    }
    for brace in c.braces.values_mut().filter(|e| on(&e.base)) {
        if let Some(curve) = &mut brace.curve {
            move_curve(&mut curve.curve, m);
        }
    }
    for stump in c.stumps.values_mut().filter(|e| on(&e.base)) {
        go(&mut stump.root_pos, m);
        go(&mut stump.joint.pos, m);
        move_segments(&mut stump.segments, m);
        move_cone(&mut stump.contact_cone, m);
    }
    for kickstand in c.kickstands.values_mut().filter(|e| on(&e.base)) {
        move_segments(&mut kickstand.segments, m);
    }
}
