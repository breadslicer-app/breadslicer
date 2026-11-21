//! The mesh a support set is sliced as: every root, shaft, joint, brace and
//! tip as its own closed piece, and the rafts under them, as one world-space
//! triangle soup (nine floats a triangle).

use std::collections::{HashMap, HashSet};

use super::smooth_shafts::{ChainPoint, ShaftStyle, append_smooth_chain};
use crate::geom::buffer_geometry::BufferGeometry;
use crate::geom::math_utils::js_max;
use crate::geom::primitive_geometry::{cylinder_geometry, sphere_geometry};
use crate::geom::{Matrix4, Quaternion, Vector2};
use crate::supports::constants::JOINT_DIAMETER_OFFSET_MM;
use crate::supports::curves::bezier_utils::get_bezier_point_at_t;
use crate::supports::rafts::crenelated::geometry::compute_raft_footprint::{
    inflate_model_plate_clearance, raft_band_top_mm, raft_solid_height_mm, raft_wall_base_height_mm,
};
use crate::supports::rafts::crenelated::geometry::generate_unioned_line_raft_mesh::{
    filter_line_raft_edges, generate_unioned_line_raft_mesh,
};
use crate::supports::rafts::crenelated::geometry::line_raft_graph::{
    BuildLineRaftEdgePairsOptions, build_line_raft_edge_pairs,
};
use crate::supports::rafts::crenelated::geometry::model_plate_footprint::{
    PlateFootprintSource, collect_model_plate_footprint,
};
use crate::supports::rafts::crenelated::geometry::polygon_set_2d::PolygonError;
use crate::supports::rafts::crenelated::geometry::polygon_set_2d::PolygonWithHoles;
use crate::supports::rafts::crenelated::geometry::raft_meshes::{
    build_raft_footprint_meshes, is_untrimmed_footprint,
};
use crate::supports::rafts::crenelated::geometry::robust_wall::{
    robust_chamfered_base, robust_wall_from_polygons,
};
use crate::supports::rafts::crenelated::raft_types::{
    RaftBottomMode, RaftSettings, SupportBaseCircle,
};
use crate::supports::support_primitives::contact_cone::contact_cone_utils::get_final_socket_position;
use crate::supports::support_primitives::contact_cone::types::ContactCone;
use crate::supports::support_primitives::contact_disk::contact_disk_utils::{
    calculate_disk_thickness, get_disk_center, get_disk_rotation,
};
use crate::supports::support_primitives::knot::segment_endpoints::{
    EndpointHosts, ShaftEntity, resolve_segment_endpoints, root_top,
};
use crate::supports::types::{
    BezierCurve, ContactDisk, Joint, Knot, Roots, Segment, SegmentShape, SupportCollections, Vec3,
};

/// How finely the supports are drawn: sides around a shaft, a curve, a root,
/// a tip and a joint, and the straight pieces a curve is drawn as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupportSliceTessellation {
    pub shaft_radial_segments: u32,
    pub bezier_radial_segments: u32,
    pub bezier_steps: u32,
    pub root_radial_segments: u32,
    pub contact_cone_radial_segments: u32,
    pub joint_radial_segments: u32,
}

/// The detail every support set is drawn and sliced at, whatever its size.
///
/// The inherited budget dropped to 8, 6 and finally 3 sides per shaft as a set
/// grew (#503): fine for a viewer, but this mesh is the one that is printed, so
/// a large model got shafts with up to 59 % less section than its preset asks
/// for. Every set now keeps the detail a small one always had.
pub const SUPPORT_SLICE_TESSELLATION: SupportSliceTessellation = SupportSliceTessellation {
    shaft_radial_segments: 12,
    bezier_radial_segments: 10,
    bezier_steps: 12,
    root_radial_segments: 14,
    contact_cone_radial_segments: 12,
    joint_radial_segments: 12,
};

/// The tessellation a set is sliced at: [`SUPPORT_SLICE_TESSELLATION`] for
/// every set, so a shaft's section never depends on how many supports share
/// its set.
pub fn resolve_support_slice_tessellation(_state: &SupportCollections) -> SupportSliceTessellation {
    SUPPORT_SLICE_TESSELLATION
}

/// A triangle soup (nine floats per triangle) and where each piece of it
/// ends. A piece is one closed primitive (a frustum, a sphere, a smooth tube,
/// a raft part): pieces may overlap in space but never share a vertex, which
/// is what keeps a mesh welded piece by piece edge-manifold.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TriangleSink {
    pub floats: Vec<f32>,
    /// `floats.len()` at the end of each piece, ascending.
    pub parts: Vec<usize>,
    /// A raft whose clearance around its models could
    /// not be cut was drawn without it, under the models, instead of none.
    pub raft_without_clearance: bool,
}

impl TriangleSink {
    /// Closes the current piece (no-op when it is empty).
    pub fn end_part(&mut self) {
        if self.parts.last().copied().unwrap_or(0) < self.floats.len() {
            self.parts.push(self.floats.len());
        }
    }
}

/// The triangles of `geometry` as one piece: its indexed triples, or its
/// corners three by three.
pub fn append_geometry_triangles(sink: &mut TriangleSink, geometry: &BufferGeometry) {
    let Some(position) = geometry.position.as_deref() else {
        return;
    };
    match &geometry.index {
        Some(index) => {
            for &v in &index[..index.len() / 3 * 3] {
                let v = v as usize * 3;
                sink.floats.extend_from_slice(&position[v..v + 3]);
            }
        }
        None => sink
            .floats
            .extend_from_slice(&position[..position.len() / 9 * 9]),
    }
    sink.end_part();
}

/// A capped frustum from `start` (radius `r0`) to `end` (radius `r1`), each
/// radius at least 0.001 mm, `sides` around; nothing when the ends meet.
fn append_frustum(sink: &mut TriangleSink, start: Vec3, end: Vec3, r0: f64, r1: f64, sides: u32) {
    let axis = end.sub(start);
    let length = axis.length();
    if !(length.is_finite() && length > 1e-6) {
        return;
    }
    // The builder stands it on its Y axis, its first radius at the top.
    let mut tube = cylinder_geometry(
        js_max(0.001, r1),
        js_max(0.001, r0),
        length,
        sides as f64,
        1.0,
        false,
    );
    let turn = Quaternion::set_from_unit_vectors(Vec3::new(0.0, 1.0, 0.0), axis.normalize());
    let middle = start.add(end).multiply_scalar(0.5);
    tube.apply_matrix4(&Matrix4::compose(middle, turn, Vec3::new(1.0, 1.0, 1.0)));
    append_geometry_triangles(sink, &tube);
}

/// A ball of `diameter` (at least 0.002 mm) at `centre`, `sides` around and
/// three quarters as many rows from pole to pole (at least 3).
fn append_ball(sink: &mut TriangleSink, centre: Vec3, diameter: f64, sides: u32) {
    let rows = js_max(3.0, (sides as f64 * 0.75).floor());
    let mut ball = sphere_geometry(js_max(0.001, diameter * 0.5), sides as f64, rows);
    ball.apply_matrix4(&Matrix4::make_translation(centre.x, centre.y, centre.z));
    append_geometry_triangles(sink, &ball);
}

/// A segment's shaft of `diameter`: one frustum, or its curve as
/// `bezier_steps` straight pieces.
fn append_segment_primitive(
    sink: &mut TriangleSink,
    start: Vec3,
    end: Vec3,
    diameter: f64,
    bezier: Option<&BezierCurve>,
    tessellation: &SupportSliceTessellation,
) {
    let r = js_max(0.001, diameter * 0.5);
    let Some(curve) = bezier else {
        append_frustum(
            sink,
            start,
            end,
            r,
            r,
            tessellation.shaft_radial_segments.max(3),
        );
        return;
    };
    let steps = tessellation.bezier_steps.max(2);
    let sides = tessellation.bezier_radial_segments.max(3);
    let (c1, c2) = (curve.control_point1, curve.control_point2);
    let at = |i: u32| get_bezier_point_at_t(start, c1, c2, end, i as f64 / steps as f64);
    for i in 0..steps {
        let from = if i == 0 { start } else { at(i) };
        append_frustum(sink, from, at(i + 1), r, r, sides);
    }
}

/// How much of its host's radius a brace end takes (#238): inside the
/// host's 12-sided section (its flats are 0.966 of the radius) with room to
/// spare, so no edge of the brace's end pokes out of the shaft it hangs on.
pub const BRACE_END_FIT: f64 = 0.92;

/// A brace from `start` to `end` (two knots on the axes of their hosts),
/// `radius` along its length and narrowing over its last stretch at each end
/// to `end_radii`. The inherited brace kept one radius, the mean of both
/// hosts': on the thinner host its end stuck out of the shaft, a sliver that
/// started in mid-air below it (#238).
fn append_brace_primitive(
    sink: &mut TriangleSink,
    (start, end): (Vec3, Vec3),
    radius: f64,
    end_radii: (f64, f64),
    bezier: Option<&BezierCurve>,
    tessellation: &SupportSliceTessellation,
) {
    let length = end.sub(start).length();
    if !length.is_finite() || length <= 1e-6 {
        return;
    }
    // Each end narrows over two of its host's radii, at most 45 % of the
    // brace: the narrow end is inside the host, the full width outside it.
    let ramp = |r: f64| (2.0 * r / length).min(0.45);
    let (ta, tb) = (
        ramp(end_radii.0 / BRACE_END_FIT),
        ramp(end_radii.1 / BRACE_END_FIT),
    );
    let radius_at = |t: f64| {
        if t < ta {
            end_radii.0 + (radius - end_radii.0) * t / ta
        } else if t > 1.0 - tb {
            end_radii.1 + (radius - end_radii.1) * (1.0 - t) / tb
        } else {
            radius
        }
    };
    type Path = Box<dyn Fn(f64) -> Vec3>;
    let (point_at, steps, sides): (Path, Vec<f64>, u32) = match bezier {
        Some(curve) => {
            let (p1, p2) = (curve.control_point1, curve.control_point2);
            let n = tessellation.bezier_steps.max(2);
            let mut ts: Vec<f64> = (0..=n).map(|i| i as f64 / n as f64).collect();
            ts.extend([ta, 1.0 - tb]);
            ts.sort_by(f64::total_cmp);
            ts.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
            (
                Box::new(move |t| get_bezier_point_at_t(start, p1, p2, end, t)),
                ts,
                tessellation.bezier_radial_segments.max(3),
            )
        }
        None => {
            let mut ts = vec![0.0, ta, 1.0 - tb, 1.0];
            ts.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
            (
                Box::new(move |t| start.add(end.sub(start).multiply_scalar(t))),
                ts,
                tessellation.shaft_radial_segments.max(3),
            )
        }
    };
    for w in steps.windows(2) {
        let (r0, r1) = (radius_at(w[0]), radius_at(w[1]));
        append_frustum(sink, point_at(w[0]), point_at(w[1]), r0, r1, sides);
    }
}

fn segment_bezier(segment: &Segment) -> Option<&BezierCurve> {
    match &segment.shape {
        SegmentShape::Bezier(curve) => Some(curve),
        SegmentShape::Straight { .. } => None,
    }
}

/// The way out of the model at a contact, one millimetre long. A normal that
/// comes from a smoothed surface is shorter than that, and a tip sunk along it
/// would stop short of its penetration.
fn sink_direction(cone: &ContactCone) -> Vec3 {
    let n = cone.surface_normal.unwrap_or(cone.normal);
    let length = n.length();
    if length > 1e-9 {
        n.multiply_scalar(1.0 / length)
    } else {
        cone.normal
    }
}

/// A tip drawn plain: from `penetration_mm` inside the model along the
/// surface normal, at its contact radius, out to its socket, at its body
/// radius.
pub fn append_contact_cone_primitive(
    sink: &mut TriangleSink,
    cone: &ContactCone,
    sides: u32,
    penetration_mm: f64,
    tip_scale: f64,
) {
    let inward = cone.surface_normal.unwrap_or(cone.normal);
    let tip = cone.pos.sub(inward.multiply_scalar(penetration_mm));
    let r_tip = js_max(0.05, cone.profile.contact_diameter_mm * 0.5 * tip_scale);
    let r_socket = js_max(0.05, cone.profile.body_diameter_mm * 0.5);
    append_frustum(
        sink,
        tip,
        get_final_socket_position(cone),
        r_tip,
        r_socket,
        sides.max(4),
    );
}

/// How far a zone root's neck reaches above its join cone, mm: past where
/// the shaft's tube is at its full width (#360).
const ROOT_NECK_ABOVE_JOIN_MM: f64 = 1.0;

/// On a raft a root's foot is a small cone standing on it, this much wider
/// than a line of a line raft (#518; on a solid raft too since
/// #435): the raft carries the supports to the plate, and a root's full disk
/// only adds resin, room and suction on it.
const LINE_FOOT_SPREAD: f64 = 1.15;

/// How wide and how tall a root's foot on a raft is: its radius where it
/// stands on the raft and the height of its 45° cone up to the shaft's
/// radius.
fn line_foot(raft: &RaftSettings, shaft_radius: f64) -> (f64, f64) {
    let radius = js_max(
        shaft_radius,
        js_max(0.0, raft.line_width_mm) * 0.5 * LINE_FOOT_SPREAD,
    );
    (radius, js_max(0.05, radius - shaft_radius))
}

/// The radius each root's shaft leaves it at, by root id: its trunk's or
/// kickstand's first segment.
pub fn root_shaft_radii(state: &SupportCollections) -> HashMap<&str, f64> {
    let first = |segments: &[Segment]| {
        let d = segments.first()?.diameter;
        (d.is_finite() && d > 0.0).then(|| js_max(0.05, d * 0.5))
    };
    let trunks = state
        .trunks
        .values()
        .map(|t| (t.root_id.as_str(), first(&t.segments)));
    let kickstands = state
        .kickstands
        .values()
        .map(|k| (k.root_id.as_str(), first(&k.segments)));
    trunks
        .chain(kickstands)
        .filter_map(|(id, r)| Some((id, r?)))
        .collect()
}

/// The circle a root takes on `raft`, by which the raft's outline is drawn
/// around the roots: its foot as it is drawn, the small one standing on the
/// raft (#435), so a raft reaches past the roots as little as it can.
pub fn raft_circle_of(
    raft: &RaftSettings,
    radii: &HashMap<&str, f64>,
    root: &Roots,
) -> SupportBaseCircle {
    let r = if raft.bottom_mode == RaftBottomMode::Off {
        root.diameter * 0.5
    } else {
        line_foot(raft, shaft_radius_of(radii, root)).0
    };
    SupportBaseCircle {
        x: root.transform.pos.x,
        y: root.transform.pos.y,
        r,
    }
}

/// A root's shaft radius, or the share of its disk a shaft takes when none
/// leaves it.
fn shaft_radius_of(radii: &HashMap<&str, f64>, root: &Roots) -> f64 {
    radii
        .get(root.base.id.as_str())
        .copied()
        .unwrap_or_else(|| js_max(0.05, js_max(0.05, root.diameter * 0.5) * 0.45))
}

/// How contact tips are drawn (#360). The default draws a plain cone from
/// the run's penetration inside the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TipDrawing {
    /// The cone is exactly its contact diameter where it crosses the model
    /// surface, narrowing along the same slope inside it. Without, the cone
    /// starts at the contact diameter at the bottom of its penetration, and
    /// crosses the surface wider than its number.
    pub exact_contact: bool,
    /// Each tip that holds the model sinks by its own profile's penetration
    /// instead of the run's; a tip resting on the model below keeps at least
    /// the run's.
    pub per_support_penetration: bool,
    /// Every tip ends in a ball of its contact radius, part of the same piece,
    /// whose centre is the contact point on the face (#445). It sinks by that
    /// radius ([`tip_sink_mm`]), whatever its profile says, and being round it
    /// meets the face at any angle.
    pub round_tip: bool,
}

/// How far a round tip goes into the model: its radius, as its centre stands on
/// the surface. The one definition of how deep a tip goes (#445).
pub fn tip_sink_mm(contact_radius_mm: f64) -> f64 {
    contact_radius_mm
}

/// A contact cone's ends as drawn: its socket, where its tip ends inside
/// the model, their radii, and the top of its break-point neck (a cylinder
/// of the tip radius from the surface up to there) when it has one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContactEnds {
    pub socket: Vec3,
    pub tip: Vec3,
    pub r_socket: f64,
    pub r_tip: f64,
    pub neck: Option<Vec3>,
    /// The centre of a round end, of radius `r_tip`: the cone starts here, and
    /// `tip` is where the ball ends inside the model.
    pub ball: Option<Vec3>,
}

/// Where a contact cone ends as the slicer draws it, under the run's
/// penetration, tip scale and [`TipDrawing`].
pub fn contact_ends(
    cone: &ContactCone,
    penetration_mm: f64,
    tip_scale: f64,
    drawing: TipDrawing,
) -> ContactEnds {
    let socket = get_final_socket_position(cone);
    let n = sink_direction(cone);
    // A tip whose socket stands above it rests on the model below (a
    // stick's base tip): it keeps the run's sink, or its apex would cure on
    // nothing. A tip holding the model takes its own (its normal leads to a
    // socket below it).
    let r_contact = js_max(0.05, cone.profile.contact_diameter_mm * 0.5 * tip_scale);
    let penetration = if drawing.round_tip {
        tip_sink_mm(r_contact)
    } else if drawing.per_support_penetration && cone.normal.z <= 0.0 {
        js_max(0.0, cone.profile.penetration_mm)
    } else if drawing.per_support_penetration {
        js_max(penetration_mm, cone.profile.penetration_mm)
    } else {
        penetration_mm
    };
    let tip = cone.pos.sub(n.multiply_scalar(penetration));
    let r_socket = js_max(0.05, cone.profile.body_diameter_mm * 0.5);
    let mut ends = ContactEnds {
        socket,
        tip,
        r_socket,
        r_tip: r_contact,
        neck: None,
        ball: drawing.round_tip.then_some(cone.pos),
    };
    let length = cone.pos.distance_to(socket);
    if !(length > 1e-9) {
        return ends;
    }
    let neck = cone
        .profile
        .break_point_mm
        .filter(|b| *b > 0.0)
        .map(|b| b.min(length * 0.8));
    if let Some(neck) = neck {
        let axis = socket.sub(cone.pos).multiply_scalar(1.0 / length);
        ends.neck = Some(cone.pos.add(axis.multiply_scalar(neck)));
    } else if drawing.exact_contact && !drawing.round_tip {
        let slope = (r_socket - r_contact) / length;
        ends.r_tip = js_max(0.005, r_contact - slope * penetration);
    }
    ends
}

/// [`append_contact_cone_primitive`] drawn as `drawing` says.
pub fn append_contact_cone_with(
    sink: &mut TriangleSink,
    cone: &ContactCone,
    radial_segments: u32,
    penetration_mm: f64,
    tip_scale: f64,
    drawing: TipDrawing,
) {
    if drawing == TipDrawing::default() {
        append_contact_cone_primitive(sink, cone, radial_segments, penetration_mm, tip_scale);
        return;
    }
    let ends = contact_ends(cone, penetration_mm, tip_scale, drawing);
    let segments = radial_segments.max(4);
    let from = ends.ball.unwrap_or(ends.tip);
    match ends.neck {
        Some(neck) => {
            append_frustum(sink, from, neck, ends.r_tip, ends.r_tip, segments);
            append_frustum(sink, neck, ends.socket, ends.r_tip, ends.r_socket, segments);
        }
        None => append_frustum(sink, from, ends.socket, ends.r_tip, ends.r_socket, segments),
    }
    if let Some(ball) = ends.ball {
        append_ball(sink, ball, ends.r_tip * 2.0, segments);
    }
}

/// [`append_contact_disk_primitive`] drawn as `drawing` says: a round end is a
/// ball on the contact point, and the disk goes no further in.
pub fn append_contact_disk_with(
    sink: &mut TriangleSink,
    disk: &ContactDisk,
    radial_segments: u32,
    penetration_mm: f64,
    tip_scale: f64,
    drawing: TipDrawing,
) {
    if !drawing.round_tip {
        append_contact_disk_primitive(sink, disk, radial_segments, penetration_mm, tip_scale);
        return;
    }
    append_contact_disk_primitive(sink, disk, radial_segments, 0.0, tip_scale);
    let (_, radius) = contact_disk_size(disk, tip_scale);
    append_ball(sink, disk.pos, radius * 2.0, radial_segments.max(4));
}

/// The thickness and the radius of a contact disk as
/// [`append_contact_disk_primitive`] draws it.
pub fn contact_disk_size(disk: &ContactDisk, tip_scale: f64) -> (f64, f64) {
    let thickness = disk.disk_length_override.unwrap_or_else(|| {
        calculate_disk_thickness(disk.surface_normal, disk.cone_axis, Some(&disk.profile))
    });
    (
        thickness,
        js_max(0.01, disk.contact_diameter_mm * 0.5 * tip_scale),
    )
}

/// A contact disk: its plate, reaching `penetration_mm` further into the
/// model, and a ball on its outer face.
pub fn append_contact_disk_primitive(
    sink: &mut TriangleSink,
    disk: &ContactDisk,
    sides: u32,
    penetration_mm: f64,
    tip_scale: f64,
) {
    let (thickness, radius) = contact_disk_size(disk, tip_scale);
    let centre = get_disk_center(disk.pos, disk.surface_normal, thickness);
    let frame = Matrix4::compose(
        centre,
        get_disk_rotation(disk.surface_normal),
        Vec3::new(1.0, 1.0, 1.0),
    );
    let sides = sides.max(4) as f64;
    let mut plate = cylinder_geometry(
        radius,
        radius,
        thickness + penetration_mm,
        sides,
        1.0,
        false,
    );
    plate.apply_matrix4(&frame.multiply(&Matrix4::make_translation(
        0.0,
        -penetration_mm / 2.0,
        0.0,
    )));
    append_geometry_triangles(sink, &plate);
    let mut ball = sphere_geometry(radius, sides, js_max(3.0, (sides * 0.75).floor()));
    ball.apply_matrix4(&frame.multiply(&Matrix4::make_translation(0.0, thickness / 2.0, 0.0)));
    append_geometry_triangles(sink, &ball);
}

/// What the support mesh is drawn from.
#[derive(Debug, Clone)]
pub struct SupportSliceInput<'a> {
    pub state: &'a SupportCollections,
    pub visible_model_ids: &'a HashSet<String>,
    /// The raft every set stands on.
    pub raft: &'a RaftSettings,
    /// The models standing on the plate, which the rafts keep clear of.
    pub plate_clearance_models: &'a [PlateFootprintSource<'a>],
    /// How much thinner than their profiles the tips are drawn, %.
    pub support_tip_shrink_percent: f64,
    /// How far a plain tip goes into the model, mm.
    pub tip_penetration_mm: f64,
    /// How contact tips are drawn.
    pub tip_drawing: TipDrawing,
    /// How shafts and joints are drawn.
    pub shaft_style: ShaftStyle,
    /// Whether the rafts are drawn; without, the roots keep the feet they
    /// stand on the raft with (#556: one raft for many sets,
    /// drawn apart by [`plate_raft_parts`]).
    pub draw_rafts: bool,
}

/// What every support of a set is drawn with.
struct Draw<'a> {
    input: &'a SupportSliceInput<'a>,
    tip_scale: f64,
    tessellation: &'a SupportSliceTessellation,
    joint_blend: f64,
}

impl<'a> Draw<'a> {
    /// A joint's ball, once per joint.
    fn joint<'s>(
        &self,
        sink: &mut TriangleSink,
        seen: &mut HashSet<&'s str>,
        joint: Option<&'s Joint>,
    ) {
        let Some(joint) = joint else { return };
        if seen.insert(joint.id.as_str()) {
            append_ball(
                sink,
                joint.pos,
                js_max(0.001, joint.diameter - self.joint_blend),
                self.tessellation.joint_radial_segments,
            );
        }
    }

    /// A shaft's segments, standing on `hosts`: one smooth tube that takes
    /// in its end cones `ends` (lower, upper), or a frustum per segment with
    /// its joints. Returns whether the end cones were drawn with the tube.
    fn shaft<'s>(
        &self,
        sink: &mut TriangleSink,
        seen: &mut HashSet<&'s str>,
        shaft: ShaftEntity<'s>,
        segments: &'s [Segment],
        hosts: EndpointHosts<'s>,
        ends: (Option<&'s ContactCone>, Option<&'s ContactCone>),
    ) -> bool {
        let placed = segments
            .iter()
            .enumerate()
            .filter_map(|(i, s)| resolve_segment_endpoints(shaft, s, i, hosts).map(|ep| (s, ep)));
        if let ShaftStyle::Smooth(params) = self.input.shaft_style {
            let placed: Vec<_> = placed.collect();
            let tips = (
                self.input.tip_penetration_mm,
                self.tip_scale,
                self.input.tip_drawing,
            );
            return append_smooth_shaft(sink, &placed, ends, tips, self.tessellation, &params);
        }
        for (segment, ep) in placed {
            append_segment_primitive(
                sink,
                ep.start,
                ep.end,
                js_max(0.05, segment.diameter),
                segment_bezier(segment),
                self.tessellation,
            );
            self.joint(sink, seen, segment.bottom_joint.as_ref());
            self.joint(sink, seen, segment.top_joint.as_ref());
        }
        false
    }

    /// The contact cones of a support, unless its tube took them in.
    fn cones<'c>(
        &self,
        sink: &mut TriangleSink,
        cones: impl Iterator<Item = &'c ContactCone>,
        joined: bool,
    ) {
        if joined {
            return;
        }
        for cone in cones {
            append_contact_cone_with(
                sink,
                cone,
                self.tessellation.contact_cone_radial_segments,
                self.input.tip_penetration_mm,
                self.tip_scale,
                self.input.tip_drawing,
            );
        }
    }
}

/// A root's foot, up to where its shaft (of `shaft_radius`) leaves it. On a
/// raft: a pad as wide as the foot up the raft's body, a 45° cone to the
/// shaft, and a neck of the shaft's width past where the shaft starts (#518,
/// #435). On the plate: its disk, then its cone to the shaft, or a 45° join
/// cone from the shaft out with a neck through it (#360).
fn append_root(
    sink: &mut TriangleSink,
    root: &Roots,
    raft: &RaftSettings,
    shaft_radius: f64,
    tessellation: &SupportSliceTessellation,
) {
    let base = root.transform.pos;
    let up = |z: f64| base.add(Vec3::new(0.0, 0.0, z));
    let sides = tessellation.root_radial_segments;
    if raft.bottom_mode != RaftBottomMode::Off {
        let (foot, rise) = line_foot(raft, shaft_radius);
        let body = js_max(0.01, raft_solid_height_mm(raft));
        let shaft_from = root_top(root).z - base.z;
        append_frustum(sink, base, up(body), foot, foot, sides);
        append_frustum(sink, up(body), up(body + rise), foot, shaft_radius, sides);
        let neck_top = js_max(body + rise, shaft_from) + ROOT_NECK_ABOVE_JOIN_MM;
        append_frustum(
            sink,
            up(body),
            up(neck_top),
            shaft_radius,
            shaft_radius,
            sides,
        );
        return;
    }
    let disk = js_max(0.01, root.disk_height);
    let disk_radius = js_max(0.05, root.diameter * 0.5);
    append_frustum(sink, base, up(disk), disk_radius, disk_radius, sides);
    match root.join_cone_mm.filter(|j| *j > 0.0) {
        Some(join) => {
            let neck_top = disk + join + ROOT_NECK_ABOVE_JOIN_MM;
            append_frustum(
                sink,
                up(disk),
                up(neck_top),
                shaft_radius,
                shaft_radius,
                sides,
            );
            append_frustum(
                sink,
                up(disk),
                up(disk + join),
                shaft_radius + join,
                shaft_radius,
                sides,
            );
        }
        None => {
            let cone = js_max(0.01, root.cone_height);
            append_frustum(
                sink,
                up(disk),
                up(disk + cone),
                disk_radius,
                shaft_radius,
                sides,
            );
        }
    }
}

/// The support mesh of the visible models: every support and its raft, as
/// one triangle soup.
pub fn build_support_and_raft_world_triangles(
    input: &SupportSliceInput<'_>,
) -> Result<Vec<f32>, PolygonError> {
    build_support_and_raft_world_parts(input).map(|sink| sink.floats)
}

/// How high the raft and the feet the roots of `state` stand on reach over the
/// plate, in millimetres, as [`build_support_and_raft_world_parts`] draws
/// them: the raft's plate or lines and its wall, each root's disk, and its
/// cone or its join cone; the neck above, as wide as the shaft, does not count
/// (#481: what the view from under the plate hides). A root on
/// a raft counts its foot's cone instead (#518, #435).
pub fn raft_and_feet_top_mm(state: &SupportCollections, raft: &RaftSettings) -> f64 {
    let raft_on = raft.bottom_mode != RaftBottomMode::Off;
    let raft_top = if raft_on { raft_band_top_mm(raft) } else { 0.0 };
    let radii = root_shaft_radii(state);
    state
        .roots
        .values()
        .map(|root| {
            if raft_on {
                let (_, rise) = line_foot(raft, shaft_radius_of(&radii, root));
                return root.transform.pos.z + js_max(0.01, raft_solid_height_mm(raft)) + rise;
            }
            let disk_height = js_max(0.01, root.disk_height);
            let foot = match root.join_cone_mm.filter(|j| *j > 0.0) {
                Some(join) => disk_height + join,
                None => disk_height + js_max(0.01, root.cone_height),
            };
            root.transform.pos.z + foot
        })
        .fold(raft_top, js_max)
}

/// [`build_support_and_raft_world_triangles`] with the soup split into its
/// closed pieces (the slicer welds each piece apart).
pub fn build_support_and_raft_world_parts(
    input: &SupportSliceInput<'_>,
) -> Result<TriangleSink, PolygonError> {
    let visible = input.visible_model_ids;
    let mut sink = TriangleSink::default();
    if visible.is_empty() {
        return Ok(sink);
    }
    let state = input.state;
    let tip_scale = 1.0 - input.support_tip_shrink_percent / 100.0;
    let tessellation = resolve_support_slice_tessellation(state);
    let joint_blend = JOINT_DIAMETER_OFFSET_MM * 0.75;
    // A root is shown when its model is, or when a shown model's trunk or
    // kickstand stands on it; its raft is that model's.
    let mut visible_root_ids: HashSet<&str> = HashSet::new();
    let mut root_model_key: HashMap<&str, &str> = HashMap::new();
    let standing = (state
        .trunks
        .values()
        .map(|t| (&t.base.model_id, &t.root_id)))
    .chain(
        state
            .kickstands
            .values()
            .map(|k| (&k.base.model_id, &k.root_id)),
    );
    for (model, root) in standing.filter(|(model, _)| visible.contains(*model)) {
        visible_root_ids.insert(root);
        root_model_key.entry(root).or_insert(model);
    }

    let shaft_radii = root_shaft_radii(state);
    for root in state.roots.values() {
        if visible.contains(&root.base.model_id) || visible_root_ids.contains(root.base.id.as_str())
        {
            append_root(
                &mut sink,
                root,
                input.raft,
                shaft_radius_of(&shaft_radii, root),
                &tessellation,
            );
        }
    }

    let draw = Draw {
        input,
        tip_scale,
        tessellation: &tessellation,
        joint_blend,
    };
    let mut seen_joints: HashSet<&str> = HashSet::new();
    let shown = |model_id: &str| visible.contains(model_id);
    let knot = |id: &str| state.knots.get(id);

    for t in state.trunks.values().filter(|t| shown(&t.base.model_id)) {
        let Some(root) = state.roots.get(&t.root_id) else {
            continue;
        };
        let cone = t.contact_cone.as_ref();
        let hosts = EndpointHosts {
            root: Some(root),
            host_knot: None,
        };
        let joined = draw.shaft(
            &mut sink,
            &mut seen_joints,
            ShaftEntity::Trunk(t),
            &t.segments,
            hosts,
            (None, cone),
        );
        draw.cones(&mut sink, cone.into_iter(), joined);
    }
    for b in state.branches.values().filter(|b| shown(&b.base.model_id)) {
        let Some(parent) = knot(&b.parent_knot_id) else {
            continue;
        };
        let cone = b.contact_cone.as_ref();
        let hosts = EndpointHosts {
            root: None,
            host_knot: Some(parent),
        };
        let joined = draw.shaft(
            &mut sink,
            &mut seen_joints,
            ShaftEntity::Branch(b),
            &b.segments,
            hosts,
            (None, cone),
        );
        draw.cones(&mut sink, cone.into_iter(), joined);
    }
    for l in state.leaves.values().filter(|l| shown(&l.base.model_id)) {
        if knot(&l.parent_knot_id).is_some() {
            draw.cones(&mut sink, std::iter::once(&l.contact_cone), false);
        }
    }
    for w in state.twigs.values().filter(|w| shown(&w.base.model_id)) {
        let hosts = EndpointHosts {
            root: None,
            host_knot: None,
        };
        draw.shaft(
            &mut sink,
            &mut seen_joints,
            ShaftEntity::Twig(w),
            &w.segments,
            hosts,
            (None, None),
        );
        for disk in [&w.contact_disk_a, &w.contact_disk_b] {
            append_contact_disk_with(
                &mut sink,
                disk,
                tessellation.contact_cone_radial_segments,
                input.tip_penetration_mm,
                tip_scale,
                input.tip_drawing,
            );
        }
    }
    for s in state.sticks.values().filter(|s| shown(&s.base.model_id)) {
        let ends = (Some(&s.contact_cone_a), Some(&s.contact_cone_b));
        let hosts = EndpointHosts {
            root: None,
            host_knot: None,
        };
        let joined = draw.shaft(
            &mut sink,
            &mut seen_joints,
            ShaftEntity::Stick(s),
            &s.segments,
            hosts,
            ends,
        );
        draw.cones(
            &mut sink,
            [&s.contact_cone_a, &s.contact_cone_b].into_iter(),
            joined,
        );
    }
    for b in state.braces.values().filter(|b| shown(&b.base.model_id)) {
        let (Some(start), Some(end)) = (knot(&b.start_knot_id), knot(&b.end_knot_id)) else {
            continue;
        };
        let profile = js_max(0.001, b.profile.diameter);
        // A knot is as wide as its host plus 0.1 mm.
        let host_width = |k: &Knot| js_max(0.05, k.diameter.unwrap_or(profile + 0.1) - 0.1);
        let (start_d, end_d) = (host_width(start), host_width(end));
        let radius = js_max(0.001, (start_d + end_d) * 0.25);
        let fit = |d: f64| js_max(0.001, (d * 0.5 * BRACE_END_FIT).min(radius));
        append_brace_primitive(
            &mut sink,
            (start.pos, end.pos),
            radius,
            (fit(start_d), fit(end_d)),
            b.curve.as_ref().map(|c| &c.curve),
            &tessellation,
        );
    }
    for s in state.stumps.values().filter(|s| shown(&s.base.model_id)) {
        let top = s
            .root_pos
            .add(Vec3::new(0.0, 0.0, js_max(0.01, s.root_height)));
        append_frustum(
            &mut sink,
            s.root_pos,
            top,
            js_max(0.05, s.root_base_diameter * 0.5),
            js_max(0.05, s.root_top_diameter * 0.5),
            tessellation.root_radial_segments,
        );
        if input.shaft_style == ShaftStyle::DragonFruit {
            draw.joint(&mut sink, &mut seen_joints, Some(&s.joint));
        }
        let cone = Some(&s.contact_cone);
        let hosts = EndpointHosts {
            root: None,
            host_knot: None,
        };
        let joined = draw.shaft(
            &mut sink,
            &mut seen_joints,
            ShaftEntity::Stump(s),
            &s.segments,
            hosts,
            (None, cone),
        );
        draw.cones(&mut sink, cone.into_iter(), joined);
    }
    for k in state
        .kickstands
        .values()
        .filter(|k| shown(&k.base.model_id))
    {
        let Some(root) = state.roots.get(&k.root_id) else {
            continue;
        };
        let hosts = EndpointHosts {
            root: Some(root),
            host_knot: knot(&k.host_knot_id),
        };
        draw.shaft(
            &mut sink,
            &mut seen_joints,
            ShaftEntity::Kickstand(k),
            &k.segments,
            hosts,
            (None, None),
        );
    }

    if input.draw_rafts {
        append_slice_rafts(&mut sink, input, &visible_root_ids, &root_model_key)?;
    }
    on_the_plate(&mut sink);
    Ok(sink)
}

/// Nothing of the supports below the plate: the ring at the
/// low end of a slanted tube that starts on the plate (a brace, a
/// kickstand) dips under it by a fraction of its radius, which nothing can
/// print and which the view flags as out of the volume. Those corners stand
/// on the plate instead.
fn on_the_plate(sink: &mut TriangleSink) {
    for z in sink.floats.iter_mut().skip(2).step_by(3) {
        if *z < 0.0 {
            *z = 0.0;
        }
    }
}

/// One raft under every root of `circles` (#556): the raft of
/// many sets standing together, drawn as [`append_slice_rafts`] draws one
/// model's, with no model to clear.
pub fn plate_raft_parts(
    circles: &[SupportBaseCircle],
    raft: &RaftSettings,
) -> Result<TriangleSink, PolygonError> {
    let mut sink = TriangleSink::default();
    if raft.bottom_mode == RaftBottomMode::Off || circles.is_empty() {
        return Ok(sink);
    }
    append_raft_of(&mut sink, circles, raft, &[], &[])?;
    Ok(sink)
}

/// The rafts under the shown roots: one per model their supports hold, cut
/// clear of the models standing on the plate. A raft whose clearance cannot
/// be cut is drawn without it, and the sink says so (#467).
fn append_slice_rafts(
    sink: &mut TriangleSink,
    input: &SupportSliceInput<'_>,
    visible_root_ids: &HashSet<&str>,
    root_model_key: &HashMap<&str, &str>,
) -> Result<(), PolygonError> {
    let raft = input.raft;
    if raft.bottom_mode == RaftBottomMode::Off {
        return Ok(());
    }
    let radii = root_shaft_radii(input.state);
    let mut groups: Vec<(&str, Vec<SupportBaseCircle>)> = Vec::new();
    for root in input.state.roots.values() {
        let id = root.base.id.as_str();
        if !input.visible_model_ids.contains(&root.base.model_id) && !visible_root_ids.contains(id)
        {
            continue;
        }
        let model = root_model_key
            .get(id)
            .copied()
            .unwrap_or(&root.base.model_id);
        let circle = raft_circle_of(raft, &radii, root);
        match groups.iter_mut().find(|(m, _)| *m == model) {
            Some((_, circles)) => circles.push(circle),
            None => groups.push((model, vec![circle])),
        }
    }
    let cleared =
        collect_model_plate_footprint(input.plate_clearance_models, raft_band_top_mm(raft))
            .and_then(|plan| Ok((inflate_model_plate_clearance(&plan)?, plan)));
    let (cut, plan) = cleared.unwrap_or_else(|_| {
        sink.raft_without_clearance = true;
        (Vec::new(), Vec::new())
    });
    for (_, circles) in &groups {
        let mark = (sink.floats.len(), sink.parts.len());
        match append_raft_of(sink, circles, raft, &plan, &cut) {
            Err(_) if !plan.is_empty() => {
                sink.floats.truncate(mark.0);
                sink.parts.truncate(mark.1);
                sink.raft_without_clearance = true;
                append_raft_of(sink, circles, raft, &[], &[])?;
            }
            drawn => drawn?,
        }
    }
    Ok(())
}

/// The raft under the roots `circles`, clear of `clearance` (its beams clear
/// of `clearance_cut`): a solid raft's plate, closed ring by ring when its
/// outline was cut, or a line raft's beams; then its wall.
fn append_raft_of(
    sink: &mut TriangleSink,
    circles: &[SupportBaseCircle],
    raft: &RaftSettings,
    clearance: &[PolygonWithHoles],
    clearance_cut: &[PolygonWithHoles],
) -> Result<(), PolygonError> {
    let parts = build_raft_footprint_meshes(circles, raft, Some(clearance))?;
    if parts.base_mesh.is_none() && parts.footprint.is_empty() {
        return Ok(());
    }
    let cut_plate = (parts.base_mesh.is_some() && !is_untrimmed_footprint(&parts.footprint))
        .then(|| robust_chamfered_base(&parts.footprint, raft));
    match cut_plate.as_ref().or(parts.base_mesh.as_ref()) {
        Some(plate) => append_geometry_triangles(sink, plate),
        None if raft.bottom_mode == RaftBottomMode::Line => append_line_raft(
            sink,
            circles,
            raft,
            !parts.footprint.is_empty(),
            clearance_cut,
        )?,
        None => {}
    }
    let wall = if raft.wall_enabled {
        robust_wall_from_polygons(&parts.footprint, raft, raft_wall_base_height_mm(raft))?
    } else {
        parts.wall_mesh
    };
    if let Some(wall) = &wall {
        append_geometry_triangles(sink, wall);
    }
    Ok(())
}

/// A line raft's beams between the roots, less those that would run into a
/// model, joined into one mesh (or each its own when they will not join).
fn append_line_raft(
    sink: &mut TriangleSink,
    circles: &[SupportBaseCircle],
    raft: &RaftSettings,
    has_border_ring: bool,
    clearance_cut: &[PolygonWithHoles],
) -> Result<(), PolygonError> {
    let nodes: Vec<Vector2> = circles.iter().map(|c| Vector2::new(c.x, c.y)).collect();
    let options = BuildLineRaftEdgePairsOptions {
        has_border_ring,
        keep_factor: Some(8.0),
        abs_max_len: Some(220.0),
        enforce_connected: Some(true),
        include_spanning_tree_backbone: None,
    };
    let beams: Vec<(Vector2, Vector2)> = build_line_raft_edge_pairs(&nodes, options)
        .into_iter()
        .map(|(a, b)| (nodes[a], nodes[b]))
        .collect();
    let beams = filter_line_raft_edges(&beams, Some(clearance_cut), raft.line_width_mm);
    let height = js_max(0.01, raft.line_height_mm);
    let joined = generate_unioned_line_raft_mesh(&beams, raft.line_width_mm, height, None, None)?;
    if joined.vertex_count() > 0 {
        append_geometry_triangles(sink, &joined);
        return Ok(());
    }
    for (a, b) in &beams {
        append_beam_box(sink, *a, *b, js_max(0.01, raft.line_width_mm), height);
    }
    Ok(())
}

/// A beam from `a` to `b` on the plate, `width` wide and `height` tall, as a
/// closed box (nothing when its ends meet).
fn append_beam_box(sink: &mut TriangleSink, a: Vector2, b: Vector2, width: f64, height: f64) {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let length = (dx * dx + dy * dy).sqrt();
    if !(length.is_finite() && length >= 0.001) {
        return;
    }
    let (sx, sy) = (-dy / length * width / 2.0, dx / length * width / 2.0);
    // Bottom then top, each: a right, a left, b left, b right.
    let corner = |i: usize| {
        let (p, side, z) = (
            [a, a, b, b][i % 4],
            [-1.0, 1.0, 1.0, -1.0][i % 4],
            if i < 4 { 0.0 } else { height },
        );
        [(p.x + sx * side) as f32, (p.y + sy * side) as f32, z as f32]
    };
    const FACES: [usize; 36] = [
        0, 2, 1, 0, 3, 2, 4, 5, 6, 4, 6, 7, 0, 4, 7, 0, 7, 3, 1, 6, 2, 1, 5, 6, 0, 1, 5, 0, 5, 4,
        3, 6, 7, 3, 2, 6,
    ];
    for i in FACES {
        sink.floats.extend(corner(i));
    }
    sink.end_part();
}

/// One tube's centre line and its edges' (start, end) radii.
type Chain = (Vec<ChainPoint>, Vec<(f64, f64)>);

/// A shaft's resolved segments as smooth tubes: each run of connected
/// segments is one chain, curves sampled as the plain style draws them, a
/// bicone wherever the plain style draws a joint ball between segments.
/// The shaft's lower and upper contact cones (`cones`) taper from its first
/// and last chains to their tips, so the bend into a contact is part of the
/// tube. Returns whether a tube was drawn (and so the cones with it).
fn append_smooth_shaft(
    sink: &mut TriangleSink,
    resolved: &[(
        &Segment,
        crate::supports::support_primitives::knot::segment_endpoints::SegmentEndpoints,
    )],
    cones: (Option<&ContactCone>, Option<&ContactCone>),
    (penetration_mm, tip_scale, drawing): (f64, f64, TipDrawing),
    tessellation: &SupportSliceTessellation,
    params: &super::smooth_shafts::SmoothShaftParams,
) -> bool {
    if resolved.is_empty() {
        return false;
    }
    // A contact's ends as drawn.
    let cone_ends = |cone: &ContactCone| contact_ends(cone, penetration_mm, tip_scale, drawing);
    let mut chains: Vec<Chain> = Vec::new();
    let mut previous: Option<&Segment> = None;
    for &(seg, ep) in resolved {
        let radius = js_max(0.001, js_max(0.05, seg.diameter) * 0.5);
        let joined = chains
            .last()
            .and_then(|(points, _)| points.last())
            .is_some_and(|p| p.pos.distance_to(ep.start) <= 1e-6);
        if !joined {
            chains.push((
                vec![ChainPoint {
                    pos: ep.start,
                    bulge: false,
                }],
                Vec::new(),
            ));
        } else if let Some(last) = chains.last_mut().and_then(|(points, _)| points.last_mut()) {
            last.bulge =
                previous.is_some_and(|p| p.top_joint.is_some()) || seg.bottom_joint.is_some();
        }
        let (points, radii) = chains.last_mut().unwrap();
        match segment_bezier(seg) {
            Some(curve) => {
                let steps = tessellation.bezier_steps.max(2);
                for i in 1..=steps {
                    let t = i as f64 / steps as f64;
                    let p = get_bezier_point_at_t(
                        ep.start,
                        curve.control_point1,
                        curve.control_point2,
                        ep.end,
                        t,
                    );
                    points.push(ChainPoint {
                        pos: p,
                        bulge: false,
                    });
                    radii.push((radius, radius));
                }
            }
            None => {
                points.push(ChainPoint {
                    pos: ep.end,
                    bulge: false,
                });
                radii.push((radius, radius));
            }
        }
        if let Some(last) = points.last_mut() {
            last.bulge = seg.top_joint.is_some();
        }
        previous = Some(seg);
    }

    // A shaft whose segment runs from its upper contact to its lower one (a
    // stick's, whose ends are ordered by height after its segment is made):
    // the tube runs the other way, so each contact joins the end of the tube
    // at its own socket instead of the tube doubling back on itself (#564: a
    // thin neck between two wide cones, and a ball where the lower tip met
    // the shaft).
    let mut ends = (cones.0.map(cone_ends), cones.1.map(cone_ends));
    if let [(points, radii)] = chains.as_mut_slice()
        && let (Some(first), Some(last)) = (points.first(), points.last())
    {
        let (lower, upper) = (ends.0.map(|e| e.socket), ends.1.map(|e| e.socket));
        let gap = |end: Vec3, at: Option<Vec3>| at.map_or(0.0, |s| end.distance_to(s));
        let kept = gap(first.pos, lower) + gap(last.pos, upper);
        let turned = gap(last.pos, lower) + gap(first.pos, upper);
        if turned + 1e-9 < kept {
            points.reverse();
            radii.reverse();
            for r in radii.iter_mut() {
                *r = (r.1, r.0);
            }
        }
    }
    // Two tips longer than the span between them: their sockets run past
    // each other, and the tube between would double back. They meet where
    // their lengths share the span, with no trunk between.
    if let ([(points, radii)], (Some(low), Some(high)), (Some(cl), Some(ch))) =
        (chains.as_mut_slice(), (&mut ends.0, &mut ends.1), cones)
    {
        let along = ch.pos.sub(cl.pos);
        if high.socket.sub(low.socket).dot(along) < 0.0 {
            let (la, lb) = (
                cl.pos.distance_to(low.socket),
                ch.pos.distance_to(high.socket),
            );
            let meet = cl
                .pos
                .add(along.multiply_scalar(la / js_max(1e-9, la + lb)));
            for (end, contact) in [(&mut *low, cl.pos), (&mut *high, ch.pos)] {
                end.socket = meet;
                if end
                    .neck
                    .is_some_and(|n| contact.distance_to(n) >= contact.distance_to(meet))
                {
                    end.neck = None;
                }
            }
            *points = vec![ChainPoint {
                pos: meet,
                bulge: false,
            }];
            radii.clear();
        }
    }

    // The upper contact: on from the last chain through its socket to the tip.
    if let (Some(upper), Some((points, radii))) = (ends.1, chains.last_mut()) {
        let ContactEnds {
            socket,
            tip,
            r_socket,
            r_tip,
            neck,
            ball,
        } = upper;
        let tip = ball.unwrap_or(tip);
        let shaft_r = radii.last().map_or(r_socket, |r| r.1);
        if points
            .last()
            .is_some_and(|p| p.pos.distance_to(socket) > 1e-6)
        {
            points.push(ChainPoint {
                pos: socket,
                bulge: false,
            });
            radii.push((shaft_r, shaft_r));
        }
        if let Some(neck) = neck {
            points.push(ChainPoint {
                pos: neck,
                bulge: false,
            });
            radii.push((r_socket, r_tip));
            points.push(ChainPoint {
                pos: tip,
                bulge: false,
            });
            radii.push((r_tip, r_tip));
        } else {
            points.push(ChainPoint {
                pos: tip,
                bulge: false,
            });
            radii.push((r_socket, r_tip));
        }
    }
    // The lower contact: from its tip through its socket into the first chain.
    if let (Some(lower), Some((points, radii))) = (ends.0, chains.first_mut()) {
        let ContactEnds {
            socket,
            tip,
            r_socket,
            r_tip,
            neck,
            ball,
        } = lower;
        let tip = ball.unwrap_or(tip);
        let shaft_r = radii.first().map_or(r_socket, |r| r.0);
        if points
            .first()
            .is_some_and(|p| p.pos.distance_to(socket) > 1e-6)
        {
            points.insert(
                0,
                ChainPoint {
                    pos: socket,
                    bulge: false,
                },
            );
            radii.insert(0, (shaft_r, shaft_r));
        }
        if let Some(neck) = neck {
            points.insert(
                0,
                ChainPoint {
                    pos: neck,
                    bulge: false,
                },
            );
            radii.insert(0, (r_tip, r_socket));
            points.insert(
                0,
                ChainPoint {
                    pos: tip,
                    bulge: false,
                },
            );
            radii.insert(0, (r_tip, r_tip));
        } else {
            points.insert(
                0,
                ChainPoint {
                    pos: tip,
                    bulge: false,
                },
            );
            radii.insert(0, (r_tip, r_socket));
        }
    }

    for (points, radii) in &chains {
        if points.len() >= 2 {
            append_smooth_chain(
                sink,
                points,
                radii,
                tessellation.shaft_radial_segments,
                params,
            );
        }
    }
    for cone in [cones.0, cones.1].into_iter().flatten() {
        let ends = cone_ends(cone);
        if let Some(ball) = ends.ball {
            let segments = tessellation.contact_cone_radial_segments.max(4);
            append_ball(sink, ball, ends.r_tip * 2.0, segments);
        }
    }
    true
}

#[cfg(test)]
mod brace_end_tests {
    use super::*;

    /// The corners of `sink` at the brace end `at`, as far from `at` as the
    /// end's radius: the cap that meets the host.
    fn cap_corners(sink: &TriangleSink, at: Vec3, radius: f64) -> Vec<Vec3> {
        sink.floats
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| Vec3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2])))
            .filter(|p| p.sub(at).length() <= radius + 1e-4)
            .collect()
    }

    /// Off the vertical axis through `axis`, in the plane.
    fn off_axis(p: Vec3, axis: Vec3) -> f64 {
        (p.x - axis.x).hypot(p.y - axis.y)
    }

    /// #238: a brace between a thin and a thick trunk ends inside each one,
    /// within the flats of its 12-sided section, not wider than the thin
    /// one as the mean of the two was.
    #[test]
    fn a_brace_ends_inside_each_shaft_it_joins() {
        let tess = SUPPORT_SLICE_TESSELLATION;
        let (thin, thick) = (0.5, 0.8);
        let (start, end) = (Vec3::new(0.0, 0.0, 5.0), Vec3::new(4.0, 0.0, 7.0));
        let radius = (thin + thick) / 2.0;
        let flats = (std::f64::consts::PI / f64::from(tess.shaft_radial_segments)).cos();
        for bezier in [
            None,
            Some(BezierCurve {
                control_point1: Vec3::new(1.0, 0.5, 5.5),
                control_point2: Vec3::new(3.0, -0.5, 6.5),
                ..BezierCurve::default()
            }),
        ] {
            let ends = (
                (thin * BRACE_END_FIT).min(radius),
                (thick * BRACE_END_FIT).min(radius),
            );
            let mut sink = TriangleSink::default();
            append_brace_primitive(
                &mut sink,
                (start, end),
                radius,
                ends,
                bezier.as_ref(),
                &tess,
            );
            for (at, host, r) in [(start, thin, ends.0), (end, thick, ends.1)] {
                let corners = cap_corners(&sink, at, r);
                assert!(corners.len() >= 3, "the end at {at:?} has a cap");
                let widest = corners.iter().map(|&p| off_axis(p, at)).fold(0.0, f64::max);
                assert!(
                    widest < host * flats,
                    "the end sticks {widest:.3} mm off the axis of a {host} mm shaft"
                );
            }
            // Away from the ends it keeps its width.
            if bezier.is_none() {
                let axis = end.sub(start).normalize();
                let widest = sink
                    .floats
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .map(|p| {
                        let w =
                            Vec3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2])).sub(start);
                        w.sub(axis.multiply_scalar(w.dot(axis))).length()
                    })
                    .fold(0.0, f64::max);
                assert!((widest - radius).abs() < 1e-3, "{widest} against {radius}");
            }
        }
    }
}
