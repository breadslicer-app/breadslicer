//! What bracing sees of a shaft: its segments as resolved line pieces, its
//! highest and lowest points, and where on it a brace end lands at a given
//! height.

use std::cmp::Ordering;

use super::two_axis_detection::normalize_axis_angle_rad;
use crate::geom::math_utils::{clamp, js_atan2, js_max, js_min};
use crate::supports::auto_support::types::js_compare;
use crate::supports::shafts::{
    EndpointHosts, ShaftEntity, calculate_knot_position_on_segment_from_t,
    resolve_segment_endpoints,
};
use crate::supports::support_kinds::SupportTypeId;
use crate::supports::types::{Segment, SupportCollections, Vec3};
use crate::utils::collation::js_locale_compare;

pub(super) const EPS: f64 = 0.000001;

/// One segment of a shaft, with its ends where they stand in the scene.
#[derive(Debug, Clone)]
pub(super) struct Piece {
    pub segment: Segment,
    pub start: Vec3,
    pub end: Vec3,
}

/// A shaft a brace may join.
#[derive(Debug, Clone)]
pub(super) struct Shaft {
    pub id: String,
    pub kind: SupportTypeId,
    /// The model it holds; for models braced together, the group's key.
    pub model_id: String,
    pub pieces: Vec<Piece>,
    /// Its highest segment end (the first, on a tie).
    pub top: Vec3,
    /// Its lowest segment end (the first, on a tie).
    pub foot: Vec3,
    /// For a kickstand, the segment it leans on.
    pub host_segment_id: Option<String>,
}

impl Shaft {
    pub(super) fn top_z(&self) -> f64 {
        self.top.z
    }

    pub(super) fn bottom_z(&self) -> f64 {
        self.foot.z
    }

    /// Its widest segment's diameter.
    pub(super) fn width(&self) -> f64 {
        self.pieces
            .iter()
            .map(|p| p.segment.diameter)
            .fold(0.0, js_max)
    }
}

/// Where a brace end lands on a shaft.
#[derive(Debug, Clone)]
pub(super) struct Anchor {
    pub model_id: String,
    pub segment_id: String,
    pub t: f64,
    pub pos: Vec3,
    /// The diameter of the segment it lands on.
    pub host_diameter_mm: f64,
}

/// The kind of the store's collection that holds `id`, for an entity whose
/// kind is not stamped on it; collections are tried in kind order.
pub fn store_type_of(store: &SupportCollections, id: &str) -> Option<SupportTypeId> {
    [
        (SupportTypeId::Trunk, store.trunks.contains_key(id)),
        (SupportTypeId::Branch, store.branches.contains_key(id)),
        (SupportTypeId::Leaf, store.leaves.contains_key(id)),
        (SupportTypeId::Twig, store.twigs.contains_key(id)),
        (SupportTypeId::Stick, store.sticks.contains_key(id)),
        (SupportTypeId::Brace, store.braces.contains_key(id)),
        (SupportTypeId::Stump, store.stumps.contains_key(id)),
        (SupportTypeId::Kickstand, store.kickstands.contains_key(id)),
    ]
    .into_iter()
    .find_map(|(kind, held)| held.then_some(kind))
}

/// One entity of `collect_shafts`, before its segments are resolved.
struct Candidate<'a> {
    entity: ShaftEntity<'a>,
    stamped: Option<SupportTypeId>,
    kind: SupportTypeId,
    model_id: &'a str,
    segments: &'a [Segment],
    hosts: EndpointHosts<'a>,
    host_segment_id: Option<&'a str>,
}

/// The shaft `candidate` makes, or none when it has no kind (an unstamped
/// entity `store` does not hold, such as a kickstand built this run) or no
/// segment resolves.
fn resolve_shaft(candidate: Candidate<'_>, store: &SupportCollections) -> Option<Shaft> {
    let entity = candidate.entity;
    candidate
        .stamped
        .or_else(|| store_type_of(store, entity.id()))?;
    let pieces: Vec<Piece> = candidate
        .segments
        .iter()
        .enumerate()
        .filter_map(|(index, segment)| {
            let ends = resolve_segment_endpoints(entity, segment, index, candidate.hosts)?;
            Some(Piece {
                segment: segment.clone(),
                start: ends.start,
                end: ends.end,
            })
        })
        .collect();
    let mut ends = pieces.iter().flat_map(|p| [p.start, p.end]);
    let first = ends.next()?;
    let (mut top, mut foot) = (first, first);
    for p in ends {
        if p.z > top.z {
            top = p;
        }
        if p.z < foot.z {
            foot = p;
        }
    }
    Some(Shaft {
        id: entity.id().to_owned(),
        kind: candidate.kind,
        model_id: candidate.model_id.to_owned(),
        pieces,
        top,
        foot,
        host_segment_id: candidate
            .host_segment_id
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
    })
}

/// By model, then by the plan position of the top, then by id.
fn shaft_order(a: &Shaft, b: &Shaft) -> Ordering {
    if a.model_id != b.model_id {
        return js_locale_compare(&a.model_id, &b.model_id);
    }
    if a.top.x != b.top.x {
        return js_compare(a.top.x - b.top.x);
    }
    if a.top.y != b.top.y {
        return js_compare(a.top.y - b.top.y);
    }
    js_locale_compare(&a.id, &b.id)
}

/// Every trunk, branch, stick and kickstand of `c` with a segment, as
/// shafts in [`shaft_order`]. `store` gives the kind of an unstamped entity.
pub(super) fn collect_shafts(c: &SupportCollections, store: &SupportCollections) -> Vec<Shaft> {
    let root_of = |id: &str| (!id.is_empty()).then(|| c.roots.get(id)).flatten();
    let trunks = c.trunks.values().map(|t| Candidate {
        entity: ShaftEntity::Trunk(t),
        stamped: t.base.type_id,
        kind: SupportTypeId::Trunk,
        model_id: &t.base.model_id,
        segments: &t.segments,
        hosts: EndpointHosts {
            root: root_of(&t.root_id),
            host_knot: None,
        },
        host_segment_id: None,
    });
    let branches = c.branches.values().map(|b| Candidate {
        entity: ShaftEntity::Branch(b),
        stamped: b.base.type_id,
        kind: SupportTypeId::Branch,
        model_id: &b.base.model_id,
        segments: &b.segments,
        hosts: EndpointHosts {
            root: None,
            host_knot: c.knots.get(&b.parent_knot_id),
        },
        host_segment_id: None,
    });
    // Sticks run model to model; inside a cavity they can stand tens of
    // millimetres tall (#632).
    let sticks = c.sticks.values().map(|s| Candidate {
        entity: ShaftEntity::Stick(s),
        stamped: s.base.type_id,
        kind: SupportTypeId::Stick,
        model_id: &s.base.model_id,
        segments: &s.segments,
        hosts: EndpointHosts {
            root: None,
            host_knot: None,
        },
        host_segment_id: None,
    });
    let kickstands = c.kickstands.values().map(|k| Candidate {
        entity: ShaftEntity::Kickstand(k),
        stamped: k.base.type_id,
        kind: SupportTypeId::Kickstand,
        model_id: &k.base.model_id,
        segments: &k.segments,
        hosts: EndpointHosts {
            root: root_of(&k.root_id),
            host_knot: c.knots.get(&k.host_knot_id),
        },
        host_segment_id: Some(&k.host_segment_id),
    });
    let mut shafts: Vec<Shaft> = trunks
        .chain(branches)
        .chain(sticks)
        .chain(kickstands)
        .filter_map(|candidate| resolve_shaft(candidate, store))
        .collect();
    shafts.sort_by(shaft_order);
    shafts
}

/// Where on `shaft` a brace end at height `z` lands: on the segment whose
/// point at that height comes nearest it (the first, within [`EPS`]).
pub(super) fn anchor_at(shaft: &Shaft, z: f64) -> Option<Anchor> {
    let mut best: Option<(&Piece, f64, Vec3, f64)> = None;
    for piece in &shaft.pieces {
        let (low, high) = (
            js_min(piece.start.z, piece.end.z),
            js_max(piece.start.z, piece.end.z),
        );
        if z < low - EPS || z > high + EPS {
            continue;
        }
        let rise = piece.end.z - piece.start.z;
        let t = if rise.abs() < EPS {
            0.0
        } else {
            (z - piece.start.z) / rise
        };
        let t = clamp(t, 0.0, 1.0);
        let pos =
            calculate_knot_position_on_segment_from_t(piece.start, piece.end, &piece.segment, t);
        let miss = (pos.z - z).abs();
        if best.is_none_or(|(_, _, _, best_miss)| miss < best_miss - EPS) {
            best = Some((piece, t, pos, miss));
        }
    }
    let (piece, t, pos, _) = best?;
    Some(Anchor {
        model_id: shaft.model_id.clone(),
        segment_id: piece.segment.id.clone(),
        t,
        pos,
        host_diameter_mm: piece.segment.diameter,
    })
}

/// The horizontal distance from `a` to `b`.
pub(super) fn plan_distance(a: Vec3, b: Vec3) -> f64 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    (dx * dx + dy * dy).sqrt()
}

/// The height two shafts are compared at: the middle of the heights both
/// span, or the lower top when they share none.
pub(super) fn shared_mid_z(a: &Shaft, b: &Shaft) -> f64 {
    let low = js_max(a.bottom_z(), b.bottom_z());
    let high = js_min(a.top_z(), b.top_z());
    if high > low + 0.001 {
        (low + high) / 2.0
    } else {
        js_min(a.top_z(), b.top_z())
    }
}

/// The horizontal distance and axis from `a` to `b` at height `z` (a shaft
/// that does not reach it counts at its top), or none when they coincide.
pub(super) fn span_at(a: &Shaft, b: &Shaft, z: f64) -> Option<(f64, f64)> {
    let pa = anchor_at(a, z).map_or(a.top, |p| p.pos);
    let pb = anchor_at(b, z).map_or(b.top, |p| p.pos);
    let (dx, dy) = (pb.x - pa.x, pb.y - pa.y);
    let h_dist = (dx * dx + dy * dy).sqrt();
    if h_dist < EPS {
        return None;
    }
    Some((h_dist, normalize_axis_angle_rad(js_atan2(dy, dx))))
}
