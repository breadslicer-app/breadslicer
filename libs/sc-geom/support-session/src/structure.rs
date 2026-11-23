//! The supports as a beam frame (#243): every shaft, brace and tip a run of
//! beams, for [`bread_structure::check_layer`] to load while a layer peels.

use std::collections::HashMap;

use bread_structure::{NECK_KT, Point, SupportGraph};
use bread_support_geom::geom::Vec3;
use bread_support_model::bezier::get_bezier_point_at_t;
use bread_support_model::shafts::{EndpointHosts, ShaftEntity, resolve_segment_endpoints};
use bread_support_model::types::{Segment, SegmentShape, SupportCollections};

use super::SupportSession;
use super::pieces::{SupportContact, SupportPieceKind};

/// How many straight spans a curved segment or brace is read as.
const CURVE_SPANS: u32 = 6;

/// A root this close over the set's lowest stands on the plate or the raft
/// (mm); higher, it stands on the model.
const ON_PLATE_MM: f64 = 0.3;

/// A root never stands on the plate higher than this (mm).
const PLATE_ROOT_MAX_MM: f64 = 3.0;

fn at(p: Vec3) -> Point {
    [p.x, p.y, p.z]
}

/// The points of `segment` from `start` to `end`: its ends, and on a curve
/// the points between.
fn segment_points(segment: &Segment, start: Vec3, end: Vec3) -> Vec<Point> {
    match &segment.shape {
        SegmentShape::Straight => vec![at(start), at(end)],
        SegmentShape::Bezier(c) => (0..=CURVE_SPANS)
            .map(|i| {
                let t = f64::from(i) / f64::from(CURVE_SPANS);
                at(get_bezier_point_at_t(
                    start,
                    c.control_point1,
                    c.control_point2,
                    end,
                    t,
                ))
            })
            .collect(),
    }
}

/// A shaft's line from its foot up: each point with the diameter of the span
/// that ends there.
type Line = Vec<(Point, f64)>;

fn push(line: &mut Line, p: Point, d: f64) {
    if line.last().is_none_or(|(last, _)| *last != p) {
        line.push((p, d));
    }
}

/// Where `p` lies on `line`: the span nearest it and how far along it.
fn place_on(line: &Line, p: Point) -> Option<(usize, f64)> {
    let mut best: Option<(f64, usize, f64)> = None;
    for k in 1..line.len() {
        let (a, b) = (line[k - 1].0, line[k].0);
        let ab: Point = std::array::from_fn(|i| b[i] - a[i]);
        let ap: Point = std::array::from_fn(|i| p[i] - a[i]);
        let len2: f64 = ab.iter().map(|v| v * v).sum();
        let t = if len2 > 0.0 {
            (ab.iter().zip(&ap).map(|(x, y)| x * y).sum::<f64>() / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let q: Point = std::array::from_fn(|i| a[i] + ab[i] * t);
        let d: f64 = q.iter().zip(&p).map(|(x, y)| (x - y).powi(2)).sum();
        if best.is_none_or(|(b, _, _)| d < b) {
            best = Some((d, k, t));
        }
    }
    best.map(|(_, k, t)| (k, t))
}

/// Whether a root at a height stands on the plate or the raft: as low as
/// the set's lowest root, give or take [`ON_PLATE_MM`].
fn plate_rule(c: &SupportCollections) -> impl Fn(f64) -> bool + Copy {
    let lowest = c
        .roots
        .values()
        .map(|r| r.transform.pos.z)
        .fold(f64::INFINITY, f64::min);
    move |z: f64| z <= lowest + ON_PLATE_MM && z <= PLATE_ROOT_MAX_MM
}

impl SupportSession {
    /// Where supports stand on the model instead of the plate (#244): the
    /// roots of trunks, stumps and kickstands that are not on it, world mm.
    pub fn feet_on_model(&self) -> Vec<Point> {
        let c = &self.state.collections;
        let on_plate = plate_rule(c);
        let roots = c
            .trunks
            .values()
            .map(|t| &t.root_id)
            .chain(c.kickstands.values().map(|k| &k.root_id))
            .filter_map(|id| c.roots.get(id))
            .map(|r| at(r.transform.pos));
        roots
            .chain(c.stumps.values().map(|s| at(s.root_pos)))
            .filter(|p| !on_plate(p[2]))
            .collect()
    }

    /// The supports as a beam frame (#243), in world mm: every trunk,
    /// branch, stick, twig, stump and kickstand a run of beams along its
    /// segments (curves in straight spans), every tip a cone and a neck
    /// ending on the model's surface, braces between the knots they join,
    /// knots on their host shafts. Roots on the plate are fixed; a root on
    /// the model is a foot, tied to the body it stands on when checked.
    pub fn support_graph(&self) -> SupportGraph {
        let c = &self.state.collections;
        let mut graph = SupportGraph::new();

        // Every shaft's line, its foot first.
        // Kickstands are no piece: `None`.
        let mut lines: HashMap<String, (Option<SupportPieceKind>, Line)> = HashMap::new();
        // Knots name the segment they are on: its shaft.
        let mut shaft_of: HashMap<String, String> = HashMap::new();
        let mut add_shaft = |id: &str,
                             kind: Option<SupportPieceKind>,
                             shaft: ShaftEntity<'_>,
                             hosts: EndpointHosts<'_>,
                             min_d: f64| {
            let mut line = Line::new();
            for (i, segment) in shaft.segments().iter().enumerate() {
                shaft_of.insert(segment.id.clone(), id.to_owned());
                let Some(ends) = resolve_segment_endpoints(shaft, segment, i, hosts) else {
                    continue;
                };
                for p in segment_points(segment, ends.start, ends.end) {
                    push(&mut line, p, segment.diameter.max(min_d));
                }
            }
            if line.len() >= 2 {
                lines.insert(id.to_owned(), (kind, line));
            }
        };
        for (id, t) in &c.trunks {
            let hosts = EndpointHosts {
                root: c.roots.get(&t.root_id),
                host_knot: None,
            };
            add_shaft(
                id,
                Some(SupportPieceKind::Trunk),
                ShaftEntity::Trunk(t),
                hosts,
                0.0,
            );
        }
        for (id, b) in &c.branches {
            let hosts = EndpointHosts {
                root: None,
                host_knot: c.knots.get(&b.parent_knot_id),
            };
            add_shaft(
                id,
                Some(SupportPieceKind::Branch),
                ShaftEntity::Branch(b),
                hosts,
                0.0,
            );
        }
        for (id, t) in &c.twigs {
            add_shaft(
                id,
                Some(SupportPieceKind::Twig),
                ShaftEntity::Twig(t),
                EndpointHosts::default(),
                0.0,
            );
        }
        for (id, s) in &c.sticks {
            add_shaft(
                id,
                Some(SupportPieceKind::Stick),
                ShaftEntity::Stick(s),
                EndpointHosts::default(),
                0.0,
            );
        }
        for (id, s) in &c.stumps {
            add_shaft(
                id,
                Some(SupportPieceKind::Stump),
                ShaftEntity::Stump(s),
                EndpointHosts::default(),
                0.0,
            );
        }
        for (id, k) in &c.kickstands {
            let hosts = EndpointHosts {
                root: c.roots.get(&k.root_id),
                host_knot: c.knots.get(&k.host_knot_id),
            };
            add_shaft(
                id,
                None,
                ShaftEntity::Kickstand(k),
                hosts,
                k.profile.body_diameter_mm,
            );
        }

        // Knots on their hosts' lines, nearest span first.
        let mut on_line: HashMap<String, Vec<(usize, f64, Point)>> = HashMap::new();
        for knot in c.knots.values() {
            let host = shaft_of
                .get(&knot.parent_shaft_id)
                .unwrap_or(&knot.parent_shaft_id);
            let Some((_, line)) = lines.get(host) else {
                continue;
            };
            let p = at(knot.pos);
            if let Some((k, t)) = place_on(line, p) {
                on_line.entry(host.clone()).or_default().push((k, t, p));
            }
        }
        for (id, knots) in &mut on_line {
            knots.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
            let Some((_, line)) = lines.get_mut(id) else {
                continue;
            };
            let mut out = Line::with_capacity(line.len() + knots.len());
            let mut next = knots.iter().peekable();
            for (k, &(p, d)) in line.iter().enumerate() {
                while let Some(&&(span, _, q)) = next.peek() {
                    if span != k {
                        break;
                    }
                    push(&mut out, q, d);
                    next.next();
                }
                push(&mut out, p, d);
            }
            *line = out;
        }

        let on_plate = plate_rule(c);

        // The shafts, in order of id so that the graph is the same each time.
        let mut ids: Vec<&String> = lines.keys().collect();
        ids.sort();
        let mut tops: HashMap<String, (usize, usize)> = HashMap::new();
        for id in ids {
            let (kind, line) = &lines[id];
            let owner = graph.owner(id);
            let nodes = graph.line(line, owner);
            let (first, last) = (nodes[0], nodes[nodes.len() - 1]);
            tops.insert(id.clone(), (first, last));
            match kind {
                None => {
                    let root = c.kickstands.get(id).and_then(|k| c.roots.get(&k.root_id));
                    if let Some(root) = root {
                        stand(&mut graph, first, at(root.transform.pos), on_plate, owner);
                    }
                }
                Some(SupportPieceKind::Trunk) => {
                    let root = c.trunks.get(id).and_then(|t| c.roots.get(&t.root_id));
                    if let Some(root) = root {
                        stand(&mut graph, first, at(root.transform.pos), on_plate, owner);
                    }
                }
                Some(SupportPieceKind::Stump) => {
                    if let Some(s) = c.stumps.get(id) {
                        let foot = graph.node(at(s.root_pos));
                        graph.member(
                            foot,
                            first,
                            s.root_base_diameter.max(s.root_top_diameter),
                            s.root_top_diameter.max(line[0].1),
                            1.0,
                            owner,
                        );
                        stand(&mut graph, foot, at(s.root_pos), on_plate, owner);
                    }
                }
                _ => {}
            }
        }
        // Braces, between their knots, as wide as the shafts they join.
        let mut brace_ids: Vec<&String> = c.braces.keys().collect();
        brace_ids.sort();
        for id in brace_ids {
            let brace = &c.braces[id];
            let (Some(s), Some(e)) = (
                c.knots.get(&brace.start_knot_id),
                c.knots.get(&brace.end_knot_id),
            ) else {
                continue;
            };
            let profile = brace.profile.diameter.max(0.001);
            let host = |d: Option<f64>| (d.unwrap_or(profile + 0.1) - 0.1).max(0.05);
            let d = 0.5 * (host(s.diameter) + host(e.diameter));
            let points: Vec<Point> = match &brace.curve {
                Some(curve) => (0..=CURVE_SPANS)
                    .map(|i| {
                        at(get_bezier_point_at_t(
                            s.pos,
                            curve.curve.control_point1,
                            curve.curve.control_point2,
                            e.pos,
                            f64::from(i) / f64::from(CURVE_SPANS),
                        ))
                    })
                    .collect(),
                None => vec![at(s.pos), at(e.pos)],
            };
            let owner = graph.owner(id);
            let line: Line = points.into_iter().map(|p| (p, d)).collect();
            graph.line(&line, owner);
        }

        // The tips: each from its socket to the model's surface.
        for piece in self.pieces() {
            let owner = graph.owner(&piece.id);
            let ends = tops.get(&piece.id).copied();
            for (k, contact) in piece.contacts.iter().enumerate() {
                let socket = tip_chain(&mut graph, contact, owner);
                // What the tip's socket joins: the shaft's top, the foot of a
                // bar for its first tip, the knot of a leaf.
                let joins = match piece.kind {
                    SupportPieceKind::Leaf => piece.line.first().map(|j| graph.node(j.at)),
                    SupportPieceKind::Twig | SupportPieceKind::Stick => {
                        ends.map(|(first, last)| if k == 0 { first } else { last })
                    }
                    _ => ends.map(|(_, last)| last),
                };
                if let Some(node) = joins {
                    let d = 2.0 * contact.socket_radius_mm;
                    graph.member(socket, node, d, d, 1.0, owner);
                }
            }
        }
        graph
    }
}

/// Stands node `node` of support `owner`, whose root is at `root`: fixed on
/// the plate, a foot on the model.
fn stand(
    graph: &mut SupportGraph,
    node: usize,
    root: Point,
    on_plate: impl Fn(f64) -> bool,
    owner: usize,
) {
    if on_plate(root[2]) {
        graph.fix(node);
    } else {
        graph.foot(node, root, owner);
    }
}

/// How many beams a tip's cone is read as, shorter towards its thin end: its
/// taper concentrates the bending there, which one beam of even section
/// misses (measured on a real set: within 4 % of 8 even steps).
const CONE_SPANS: u32 = 4;

/// The beams of a tip, from its socket to where it enters the model: a neck
/// of the tip's width when it has one, then a cone in [`CONE_SPANS`] steps
/// up to the socket. Returns the socket's node.
fn tip_chain(graph: &mut SupportGraph, contact: &SupportContact, owner: usize) -> usize {
    let surface = graph.node(contact.surface);
    let (d_tip, d_socket) = (2.0 * contact.tip_radius_mm, 2.0 * contact.socket_radius_mm);
    // Where the cone starts: the top of the neck, or the surface.
    let (mut from, cone_from) = match contact.neck {
        Some(neck) => {
            let node = graph.node(neck);
            if node == surface {
                (surface, contact.surface)
            } else {
                graph.member(surface, node, d_tip, d_tip, NECK_KT, owner);
                (node, neck)
            }
        }
        None => (surface, contact.surface),
    };
    let mut kt = if from == surface { NECK_KT } else { 1.0 };
    let mut d_from = d_tip;
    for k in 1..=CONE_SPANS {
        // Shorter steps near the thin end, where it bends.
        let t = (f64::from(k) / f64::from(CONE_SPANS)).powi(2);
        let p: Point =
            std::array::from_fn(|i| cone_from[i] + (contact.socket[i] - cone_from[i]) * t);
        let d = d_tip + (d_socket - d_tip) * t;
        let node = graph.node(p);
        if node != from {
            graph.member(from, node, d_from, d, kt, owner);
            kt = 1.0;
            d_from = d;
            from = node;
        }
    }
    graph.contact(surface, contact.tip, owner);
    from
}
