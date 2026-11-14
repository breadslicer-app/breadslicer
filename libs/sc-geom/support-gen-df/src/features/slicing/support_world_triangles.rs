//! DragonFruit `src/features/slicing/rasterLayerZipExport.ts`, the support
//! tessellation the slicer rasterises: `buildSupportAndRaftWorldTriangles`
//! and the primitives it is built from (`createFrustumGeometryBetween`,
//! `appendJointSphere`, `resolveSupportSliceTessellation`,
//! `appendSegmentPrimitive`, `appendContactConePrimitive`,
//! `appendContactDiskPrimitive`, `appendGeometryTriangles`).
//!
//! The output is a world-space triangle soup, nine floats per triangle. Every
//! value is read back from a `Float32Array` attribute, so the soup holds them
//! as `f32` without loss.
//!
//! DragonFruit reads the tip penetration from the active material and printer
//! profiles (`calculateTipOffset`); here it is the caller's `tip_penetration_mm`.

use std::collections::{HashMap, HashSet};

use super::smooth_shafts::{ChainPoint, ShaftStyle, append_smooth_chain};
use crate::clipper::ClipperError;
use crate::supports::constants::JOINT_DIAMETER_OFFSET_MM;
use crate::supports::curves::bezier_utils::get_bezier_point_at_t;
use crate::supports::rafts::crenelated::geometry::build_line_raft_edge_pairs::{
    BuildLineRaftEdgePairsOptions, build_line_raft_edge_pairs,
};
use crate::supports::rafts::crenelated::geometry::compute_raft_footprint::{
    inflate_model_plate_clearance, raft_band_top_mm, raft_wall_base_height_mm,
};
use crate::supports::rafts::crenelated::geometry::generate_chamfered_beam::generate_chamfered_beam;
use crate::supports::rafts::crenelated::geometry::generate_raft_from_footprint::{
    build_raft_footprint_meshes, is_untrimmed_footprint,
};
use crate::supports::rafts::crenelated::geometry::generate_unioned_line_raft_mesh::{
    filter_line_raft_edges, generate_unioned_line_raft_mesh,
};
use crate::supports::rafts::crenelated::geometry::model_plate_footprint::{
    PlateFootprintSource, collect_model_plate_footprint,
};
use crate::supports::rafts::crenelated::geometry::polygon_set_2d::PolygonWithHoles;
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
use crate::supports::support_type_registry::{
    SUPPORT_TYPES, SupportCollectionKey, SupportEdgeTarget, SupportEndpointKind,
};
use crate::supports::types::{
    BezierCurve, ContactDisk, Joint, Knot, Roots, Segment, SegmentShape, SupportCollections, Vec3,
};
use crate::three::buffer_geometry::BufferGeometry;
use crate::three::math_utils::js_max;
use crate::three::primitive_geometry::{cylinder_geometry, sphere_geometry};
use crate::three::{Matrix4, Quaternion, Vector2};

/// The detail level the scene's size picks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupportSliceTessellation {
    pub shaft_radial_segments: u32,
    pub bezier_radial_segments: u32,
    pub bezier_steps: u32,
    pub root_radial_segments: u32,
    pub contact_cone_radial_segments: u32,
    pub joint_radial_segments: u32,
}

/// `resolveSupportSliceTessellation`: fewer segments as the scene grows.
pub fn resolve_support_slice_tessellation(state: &SupportCollections) -> SupportSliceTessellation {
    let mut segment_count = 0usize;
    let mut shaftless_count = 0usize;
    for descriptor in &SUPPORT_TYPES {
        if !descriptor.has_segments {
            shaftless_count += match descriptor.location.key() {
                SupportCollectionKey::Leaves => state.leaves.len(),
                SupportCollectionKey::Braces => state.braces.len(),
                _ => 0,
            };
            continue;
        }
        segment_count += match descriptor.location.key() {
            SupportCollectionKey::Trunks => state.trunks.values().map(|e| e.segments.len()).sum(),
            SupportCollectionKey::Branches => {
                state.branches.values().map(|e| e.segments.len()).sum()
            }
            SupportCollectionKey::Twigs => state.twigs.values().map(|e| e.segments.len()).sum(),
            SupportCollectionKey::Sticks => state.sticks.values().map(|e| e.segments.len()).sum(),
            SupportCollectionKey::Stumps => state.stumps.values().map(|e| e.segments.len()).sum(),
            SupportCollectionKey::Kickstands => {
                state.kickstands.values().map(|e| e.segments.len()).sum()
            }
            _ => 0,
        };
    }
    let primitive_count = segment_count + state.roots.len() + shaftless_count;
    let t = |s, b, st, r, c, j| SupportSliceTessellation {
        shaft_radial_segments: s,
        bezier_radial_segments: b,
        bezier_steps: st,
        root_radial_segments: r,
        contact_cone_radial_segments: c,
        joint_radial_segments: j,
    };
    if segment_count >= 20_000 || primitive_count >= 24_000 {
        t(3, 3, 4, 4, 4, 4)
    } else if segment_count >= 8_000 || primitive_count >= 10_000 {
        t(6, 6, 6, 8, 6, 6)
    } else if segment_count >= 3_000 || primitive_count >= 4_000 {
        t(8, 7, 8, 10, 8, 8)
    } else {
        t(12, 10, 12, 14, 12, 12)
    }
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
}

impl TriangleSink {
    /// Closes the current piece (no-op when it is empty).
    pub fn end_part(&mut self) {
        if self.parts.last().copied().unwrap_or(0) < self.floats.len() {
            self.parts.push(self.floats.len());
        }
    }

    /// The pieces, as float ranges.
    pub fn part_ranges(&self) -> impl Iterator<Item = std::ops::Range<usize>> + '_ {
        let mut start = 0;
        self.parts.iter().map(move |&end| {
            let range = start..end;
            start = end;
            range
        })
    }
}

/// `appendGeometryTriangles(sink, geometry)` without a matrix: each indexed
/// (or consecutive) triple of stored positions.
pub fn append_geometry_triangles(sink: &mut TriangleSink, geometry: &BufferGeometry) {
    let Some(position) = geometry.position.as_deref() else {
        return;
    };
    let mut tri = |a: usize, b: usize, c: usize| {
        for v in [a, b, c] {
            sink.floats.extend_from_slice(&position[v * 3..v * 3 + 3]);
        }
    };
    if let Some(index) = &geometry.index {
        for t in index.as_chunks::<3>().0 {
            tri(t[0] as usize, t[1] as usize, t[2] as usize);
        }
        sink.end_part();
        return;
    }
    let count = position.len() / 3;
    let mut i = 0;
    while i + 2 < count {
        tri(i, i + 1, i + 2);
        i += 3;
    }
    sink.end_part();
}

/// `createFrustumGeometryBetween`: a capped cylinder from `start` to `end`.
fn create_frustum_geometry_between(
    start: Vec3,
    end: Vec3,
    radius_start: f64,
    radius_end: f64,
    radial_segments: u32,
) -> Option<BufferGeometry> {
    let dir = end.sub(start);
    let length = dir.length();
    if !length.is_finite() || length <= 1e-6 {
        return None;
    }
    let r0 = js_max(0.001, radius_start);
    let r1 = js_max(0.001, radius_end);
    let mut geom = cylinder_geometry(r1, r0, length, radial_segments as f64, 1.0, false);
    let q = Quaternion::set_from_unit_vectors(Vec3::new(0.0, 1.0, 0.0), dir.normalize());
    let m = Matrix4::compose(
        start.add(end).multiply_scalar(0.5),
        q,
        Vec3::new(1.0, 1.0, 1.0),
    );
    geom.apply_matrix4(&m);
    Some(geom)
}

fn append_frustum(
    sink: &mut TriangleSink,
    start: Vec3,
    end: Vec3,
    r0: f64,
    r1: f64,
    segments: u32,
) {
    if let Some(g) = create_frustum_geometry_between(start, end, r0, r1, segments) {
        append_geometry_triangles(sink, &g);
    }
}

/// `appendJointSphere`.
fn append_joint_sphere(sink: &mut TriangleSink, pos: Vec3, diameter: f64, radial_segments: u32) {
    let radius = js_max(0.001, diameter * 0.5);
    let height_segments = js_max(3.0, (radial_segments as f64 * 0.75).floor());
    let mut geom = sphere_geometry(radius, radial_segments as f64, height_segments);
    geom.apply_matrix4(&Matrix4::make_translation(pos.x, pos.y, pos.z));
    append_geometry_triangles(sink, &geom);
}

/// `appendSegmentPrimitive`: a straight frustum, or a Bezier as `bezierSteps`
/// short ones.
fn append_segment_primitive(
    sink: &mut TriangleSink,
    start: Vec3,
    end: Vec3,
    diameter: f64,
    bezier: Option<&BezierCurve>,
    tessellation: &SupportSliceTessellation,
) {
    let radius = js_max(0.001, diameter * 0.5);
    let shaft_radial = tessellation.shaft_radial_segments.max(3);
    let bezier_radial = tessellation.bezier_radial_segments.max(3);
    let bezier_steps = tessellation.bezier_steps.max(2);
    if let Some(curve) = bezier {
        let (p0, p1, p2, p3) = (start, curve.control_point1, curve.control_point2, end);
        let mut prev = start;
        for i in 1..=bezier_steps {
            let t = i as f64 / bezier_steps as f64;
            let cur = get_bezier_point_at_t(p0, p1, p2, p3, t);
            append_frustum(sink, prev, cur, radius, radius, bezier_radial);
            prev = cur;
        }
        return;
    }
    append_frustum(sink, start, end, radius, radius, shaft_radial);
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

/// `appendContactConePrimitive`: penetration point to socket.
pub fn append_contact_cone_primitive(
    sink: &mut TriangleSink,
    cone: &ContactCone,
    radial_segments: u32,
    penetration_mm: f64,
    tip_scale: f64,
) {
    let socket = get_final_socket_position(cone);
    let n = cone.surface_normal.unwrap_or(cone.normal);
    let start = Vec3::new(
        cone.pos.x - n.x * penetration_mm,
        cone.pos.y - n.y * penetration_mm,
        cone.pos.z - n.z * penetration_mm,
    );
    append_frustum(
        sink,
        start,
        socket,
        js_max(0.05, cone.profile.contact_diameter_mm * 0.5 * tip_scale),
        js_max(0.05, cone.profile.body_diameter_mm * 0.5),
        radial_segments.max(4),
    );
}

/// How far a zone root's neck reaches above its join cone, mm: past where
/// the shaft's tube is at its full width (SoulCrafted, #360).
const ROOT_NECK_ABOVE_JOIN_MM: f64 = 1.0;

/// On a line raft a root's foot is a small cone standing on the lines, this
/// much wider than a line where the lines meet (SoulCrafted, #518): the lines
/// carry the supports to the plate, and a root's full disk only adds resin
/// and suction on it.
const LINE_FOOT_SPREAD: f64 = 1.15;

/// How wide and how tall a root's foot on a line raft is: its radius at the
/// lines and the height of its 45° cone up to the shaft's radius.
fn line_foot(raft: &RaftSettings, shaft_radius: f64) -> (f64, f64) {
    let radius = js_max(
        shaft_radius,
        js_max(0.0, raft.line_width_mm) * 0.5 * LINE_FOOT_SPREAD,
    );
    (radius, js_max(0.05, radius - shaft_radius))
}

/// The radius each root's shaft leaves it at, by root id.
fn root_shaft_radii(state: &SupportCollections) -> HashMap<&str, f64> {
    let first_radius = |segments: &[Segment]| {
        segments
            .first()
            .map(|s| s.diameter)
            .filter(|d| d.is_finite() && *d > 0.0)
            .map(|d| js_max(0.05, d * 0.5))
    };
    let mut radii = HashMap::new();
    for t in state.trunks.values() {
        if let Some(r) = first_radius(&t.segments) {
            radii.insert(t.root_id.as_str(), r);
        }
    }
    for k in state.kickstands.values() {
        if let Some(r) = first_radius(&k.segments) {
            radii.insert(k.root_id.as_str(), r);
        }
    }
    radii
}

/// A root's shaft radius, or the share of its disk a shaft takes when none
/// leaves it.
fn shaft_radius_of(radii: &HashMap<&str, f64>, root: &Roots) -> f64 {
    radii
        .get(root.base.id.as_str())
        .copied()
        .unwrap_or_else(|| js_max(0.05, js_max(0.05, root.diameter * 0.5) * 0.45))
}

/// How contact tips are drawn (SoulCrafted, #360). The default is
/// DragonFruit's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TipDrawing {
    /// The cone is exactly its contact diameter where it crosses the model
    /// surface, narrowing along the same slope inside it. DragonFruit starts
    /// the cone at the contact diameter at the bottom of its penetration, so
    /// it crosses the surface wider than its number.
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
        append_joint_sphere(sink, ball, ends.r_tip * 2.0, segments);
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
    append_joint_sphere(sink, disk.pos, radius * 2.0, radial_segments.max(4));
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

/// `appendContactDiskPrimitive`: a cylinder through the disk plus a sphere.
pub fn append_contact_disk_primitive(
    sink: &mut TriangleSink,
    disk: &ContactDisk,
    radial_segments: u32,
    penetration_mm: f64,
    tip_scale: f64,
) {
    let (thickness, radius) = contact_disk_size(disk, tip_scale);
    let center = get_disk_center(disk.pos, disk.surface_normal, thickness);
    let rotation = get_disk_rotation(disk.surface_normal);
    let group = Matrix4::compose(center, rotation, Vec3::new(1.0, 1.0, 1.0));

    let mut cylinder = cylinder_geometry(
        radius,
        radius,
        thickness + penetration_mm,
        radial_segments.max(4) as f64,
        1.0,
        false,
    );
    cylinder.apply_matrix4(&group.multiply(&Matrix4::make_translation(
        0.0,
        -penetration_mm / 2.0,
        0.0,
    )));
    append_geometry_triangles(sink, &cylinder);

    let sphere_segments = radial_segments.max(4) as f64;
    let height_segments = js_max(3.0, (sphere_segments * 0.75).floor());
    let mut sphere = sphere_geometry(radius, sphere_segments, height_segments);
    sphere.apply_matrix4(&group.multiply(&Matrix4::make_translation(0.0, thickness / 2.0, 0.0)));
    append_geometry_triangles(sink, &sphere);
}

/// The inputs `buildSupportAndRaftWorldTriangles` reads from DragonFruit's
/// stores.
#[derive(Debug, Clone)]
pub struct SupportSliceInput<'a> {
    pub state: &'a SupportCollections,
    pub visible_model_ids: &'a HashSet<String>,
    /// `getRaftSettings()`: the global raft settings (the slicer does not read
    /// per-model overrides).
    pub raft: &'a RaftSettings,
    pub plate_clearance_models: &'a [PlateFootprintSource<'a>],
    pub support_tip_shrink_percent: f64,
    /// `calculateTipOffset(...)` for the active material and printer, or 0.
    pub tip_penetration_mm: f64,
    /// How contact tips are drawn (SoulCrafted).
    pub tip_drawing: TipDrawing,
    /// How shafts and joints are drawn (SoulCrafted's option; the default is
    /// DragonFruit's).
    pub shaft_style: ShaftStyle,
    /// SoulCrafted's option: raft walls built with Clipper and trimmed bases
    /// built on rings without collinear points (`robust_wall`), so the raft
    /// is closed and manifold. `false` is DragonFruit's output.
    pub robust_raft_geometry: bool,
    /// Whether the rafts are drawn; without, the roots keep the feet they
    /// stand on the raft with (SoulCrafted, #556: one raft for many sets,
    /// drawn apart by [`plate_raft_parts`]).
    pub draw_rafts: bool,
}

/// The entity fields the generic walk needs, read per type.
struct EntityView<'a> {
    model_id: &'a str,
    root_id: Option<&'a str>,
    knot_ids: Vec<&'a str>,
    shaft: Option<ShaftEntity<'a>>,
    segments: &'a [Segment],
    inline_root: Option<(Vec3, f64, f64, f64, &'a Joint)>,
    brace: Option<(&'a crate::supports::types::Brace,)>,
    cones: Vec<(&'static str, &'a ContactCone)>,
    disks: Vec<(&'static str, &'a ContactDisk)>,
}

fn entities_of(state: &SupportCollections, key: SupportCollectionKey) -> Vec<EntityView<'_>> {
    let empty = |model_id| EntityView {
        model_id,
        root_id: None,
        knot_ids: Vec::new(),
        shaft: None,
        segments: &[],
        inline_root: None,
        brace: None,
        cones: Vec::new(),
        disks: Vec::new(),
    };
    match key {
        SupportCollectionKey::Trunks => state
            .trunks
            .values()
            .map(|e| EntityView {
                root_id: Some(&e.root_id),
                shaft: Some(ShaftEntity::Trunk(e)),
                segments: &e.segments,
                cones: e.contact_cone.iter().map(|c| ("contactCone", c)).collect(),
                ..empty(&e.base.model_id)
            })
            .collect(),
        SupportCollectionKey::Branches => state
            .branches
            .values()
            .map(|e| EntityView {
                knot_ids: vec![&e.parent_knot_id],
                shaft: Some(ShaftEntity::Branch(e)),
                segments: &e.segments,
                cones: e.contact_cone.iter().map(|c| ("contactCone", c)).collect(),
                ..empty(&e.base.model_id)
            })
            .collect(),
        SupportCollectionKey::Leaves => state
            .leaves
            .values()
            .map(|e| EntityView {
                knot_ids: vec![&e.parent_knot_id],
                cones: vec![("contactCone", &e.contact_cone)],
                ..empty(&e.base.model_id)
            })
            .collect(),
        SupportCollectionKey::Twigs => state
            .twigs
            .values()
            .map(|e| EntityView {
                shaft: Some(ShaftEntity::Twig(e)),
                segments: &e.segments,
                disks: vec![
                    ("contactDiskA", &e.contact_disk_a),
                    ("contactDiskB", &e.contact_disk_b),
                ],
                ..empty(&e.base.model_id)
            })
            .collect(),
        SupportCollectionKey::Sticks => state
            .sticks
            .values()
            .map(|e| EntityView {
                shaft: Some(ShaftEntity::Stick(e)),
                segments: &e.segments,
                cones: vec![
                    ("contactConeA", &e.contact_cone_a),
                    ("contactConeB", &e.contact_cone_b),
                ],
                ..empty(&e.base.model_id)
            })
            .collect(),
        SupportCollectionKey::Braces => state
            .braces
            .values()
            .map(|e| EntityView {
                knot_ids: vec![&e.start_knot_id, &e.end_knot_id],
                brace: Some((e,)),
                ..empty(&e.base.model_id)
            })
            .collect(),
        SupportCollectionKey::Stumps => state
            .stumps
            .values()
            .map(|e| EntityView {
                shaft: Some(ShaftEntity::Stump(e)),
                segments: &e.segments,
                inline_root: Some((
                    e.root_pos,
                    e.root_height,
                    e.root_base_diameter,
                    e.root_top_diameter,
                    &e.joint,
                )),
                cones: vec![("contactCone", &e.contact_cone)],
                ..empty(&e.base.model_id)
            })
            .collect(),
        SupportCollectionKey::Kickstands => state
            .kickstands
            .values()
            .map(|e| EntityView {
                root_id: Some(&e.root_id),
                knot_ids: vec![&e.host_knot_id],
                shaft: Some(ShaftEntity::Kickstand(e)),
                segments: &e.segments,
                ..empty(&e.base.model_id)
            })
            .collect(),
        SupportCollectionKey::Roots | SupportCollectionKey::Knots => Vec::new(),
    }
}

/// `buildSupportAndRaftWorldTriangles(visibleModelIds, undefined,
/// supportTipShrinkPercent, plateClearanceModels)`.
pub fn build_support_and_raft_world_triangles(
    input: &SupportSliceInput<'_>,
) -> Result<Vec<f32>, ClipperError> {
    build_support_and_raft_world_parts(input).map(|sink| sink.floats)
}

/// How high the raft and the feet the roots of `state` stand on reach over the
/// plate, in millimetres, as [`build_support_and_raft_world_parts`] draws
/// them: the raft's plate or lines and its wall, each root's disk, and its
/// cone or its join cone; the neck above, as wide as the shaft, does not count
/// (SoulCrafted, #481: what the view from under the plate hides). A root on
/// a line raft counts its foot's cone instead (#518).
pub fn raft_and_feet_top_mm(state: &SupportCollections, raft: &RaftSettings) -> f64 {
    let raft_on = raft.bottom_mode != RaftBottomMode::Off;
    let raft_top = if raft_on { raft_band_top_mm(raft) } else { 0.0 };
    let plate_top = if raft_on {
        js_max(0.0, raft.thickness)
    } else {
        0.0
    };
    let radii = root_shaft_radii(state);
    state
        .roots
        .values()
        .map(|root| {
            if raft.bottom_mode == RaftBottomMode::Line {
                let (_, rise) = line_foot(raft, shaft_radius_of(&radii, root));
                return root.transform.pos.z + js_max(0.01, raft.line_height_mm) + rise;
            }
            let disk_height = js_max(0.01, root.disk_height);
            let foot = match root.join_cone_mm.filter(|j| *j > 0.0) {
                Some(join) => js_max(disk_height, plate_top) + join,
                None => disk_height + js_max(0.01, root.cone_height),
            };
            root.transform.pos.z + foot
        })
        .fold(raft_top, js_max)
}

/// [`build_support_and_raft_world_triangles`] with the soup split into its
/// closed pieces (SoulCrafted's: the slicer needs each piece welded apart).
pub fn build_support_and_raft_world_parts(
    input: &SupportSliceInput<'_>,
) -> Result<TriangleSink, ClipperError> {
    let visible = input.visible_model_ids;
    let mut sink = TriangleSink::default();
    if visible.is_empty() {
        return Ok(sink);
    }
    let state = input.state;
    let tip_scale = 1.0 - input.support_tip_shrink_percent / 100.0;
    let tessellation = resolve_support_slice_tessellation(state);
    let joint_blend = JOINT_DIAMETER_OFFSET_MM * 0.75;
    let mut seen_joint_ids: HashSet<String> = HashSet::new();
    let mut visible_root_ids: HashSet<&str> = HashSet::new();
    let mut root_model_key: HashMap<&str, &str> = HashMap::new();

    for t in state.trunks.values() {
        if !visible.contains(&t.base.model_id) {
            continue;
        }
        visible_root_ids.insert(&t.root_id);
        root_model_key.entry(&t.root_id).or_insert(&t.base.model_id);
    }
    for k in state.kickstands.values() {
        if !visible.contains(&k.base.model_id) {
            continue;
        }
        visible_root_ids.insert(&k.root_id);
        root_model_key.entry(&k.root_id).or_insert(&k.base.model_id);
    }

    let root_top_radius = root_shaft_radii(state);

    let root_visible =
        |id: &str, model_id: &str| visible.contains(model_id) || visible_root_ids.contains(id);
    for root in state.roots.values() {
        if !root_visible(&root.base.id, &root.base.model_id) {
            continue;
        }
        let disk_height = js_max(0.01, root.disk_height);
        let base = root.transform.pos;
        let root_radius = js_max(0.05, root.diameter * 0.5);
        let top_radius = shaft_radius_of(&root_top_radius, root);
        if input.raft.bottom_mode == RaftBottomMode::Line {
            // SoulCrafted (#518): on the lines, a pad as wide as the foot up
            // their height, a 45° cone from it to the shaft, and a neck of
            // the shaft's width up past where the shaft leaves the root.
            let (foot, rise) = line_foot(input.raft, top_radius);
            let line_top = js_max(0.01, input.raft.line_height_mm);
            let up = |z: f64| base.add(Vec3::new(0.0, 0.0, z));
            append_frustum(
                &mut sink,
                base,
                up(line_top),
                foot,
                foot,
                tessellation.root_radial_segments,
            );
            append_frustum(
                &mut sink,
                up(line_top),
                up(line_top + rise),
                foot,
                top_radius,
                tessellation.root_radial_segments,
            );
            let shaft_from = root_top(root).z - base.z;
            append_frustum(
                &mut sink,
                up(line_top),
                up(js_max(line_top + rise, shaft_from) + ROOT_NECK_ABOVE_JOIN_MM),
                top_radius,
                top_radius,
                tessellation.root_radial_segments,
            );
            continue;
        }
        let disk_top = base.add(Vec3::new(0.0, 0.0, disk_height));
        let cone_top = disk_top.add(Vec3::new(0.0, 0.0, js_max(0.01, root.cone_height)));
        append_frustum(
            &mut sink,
            base,
            disk_top,
            root_radius,
            root_radius,
            tessellation.root_radial_segments,
        );
        // SoulCrafted (#360): a 45° join cone from the shaft out, on the
        // disk or on the raft's plate when that is higher.
        if let Some(join) = root.join_cone_mm.filter(|j| *j > 0.0) {
            let plate_top = if input.raft.bottom_mode == RaftBottomMode::Off {
                0.0
            } else {
                js_max(0.0, input.raft.thickness)
            };
            let floor = base.add(Vec3::new(0.0, 0.0, js_max(disk_height, plate_top)));
            // The shaft's tube only reaches its full width a little above the
            // root top, which the root's own cone used to cover: a neck of the
            // shaft's width from the disk up past the join cone does.
            append_frustum(
                &mut sink,
                disk_top,
                floor.add(Vec3::new(0.0, 0.0, join + ROOT_NECK_ABOVE_JOIN_MM)),
                top_radius,
                top_radius,
                tessellation.root_radial_segments,
            );
            append_frustum(
                &mut sink,
                floor,
                floor.add(Vec3::new(0.0, 0.0, join)),
                top_radius + join,
                top_radius,
                tessellation.root_radial_segments,
            );
            continue;
        }
        append_frustum(
            &mut sink,
            disk_top,
            cone_top,
            root_radius,
            top_radius,
            tessellation.root_radial_segments,
        );
    }

    let mut emit_joint = |sink: &mut TriangleSink, joint: Option<&Joint>| {
        let Some(joint) = joint else { return };
        if !seen_joint_ids.insert(joint.id.clone()) {
            return;
        }
        append_joint_sphere(
            sink,
            joint.pos,
            js_max(0.001, joint.diameter - joint_blend),
            tessellation.joint_radial_segments,
        );
    };

    let knot = |id: &str| -> Option<&Knot> { state.knots.get(id) };
    for descriptor in &SUPPORT_TYPES {
        let knot_edges = descriptor
            .edges
            .iter()
            .filter(|e| e.to == SupportEdgeTarget::Collection(SupportCollectionKey::Knots))
            .count();
        for entity in entities_of(state, descriptor.location.key()) {
            if !visible.contains(entity.model_id) {
                continue;
            }
            let root = if descriptor.owns_root {
                entity.root_id.and_then(|id| state.roots.get(id))
            } else {
                None
            };
            let knot_at = |position: usize| {
                (position < knot_edges)
                    .then(|| entity.knot_ids.get(position).copied())
                    .flatten()
                    .and_then(knot)
            };
            let host_knot = knot_at(0);
            if descriptor.lower.kind == SupportEndpointKind::PlateRoot && root.is_none() {
                continue;
            }
            if descriptor.lower.kind == SupportEndpointKind::Knot && host_knot.is_none() {
                continue;
            }

            if descriptor.lower.kind == SupportEndpointKind::InlineRoot
                && let Some((pos, height, base_d, top_d, joint)) = entity.inline_root
            {
                let top = pos.add(Vec3::new(0.0, 0.0, js_max(0.01, height)));
                append_frustum(
                    &mut sink,
                    pos,
                    top,
                    js_max(0.05, base_d * 0.5),
                    js_max(0.05, top_d * 0.5),
                    tessellation.root_radial_segments,
                );
                if input.shaft_style == ShaftStyle::DragonFruit {
                    emit_joint(&mut sink, Some(joint));
                }
            }

            let mut joined_cones: Vec<&str> = Vec::new();
            if let (Some(shaft), ShaftStyle::Smooth(params)) = (entity.shaft, input.shaft_style) {
                let hosts = EndpointHosts { root, host_knot };
                let resolved: Vec<_> = entity
                    .segments
                    .iter()
                    .enumerate()
                    .filter_map(|(index, seg)| {
                        resolve_segment_endpoints(shaft, seg, index, hosts).map(|ep| (seg, ep))
                    })
                    .collect();
                // The shaft's own contact cones become the ends of its tube.
                let cone_at = |field: Option<&str>| {
                    field.and_then(|f| entity.cones.iter().find(|(c, _)| *c == f).map(|(_, c)| *c))
                };
                let start_cone = (descriptor.lower.kind == SupportEndpointKind::Cone)
                    .then(|| cone_at(descriptor.lower.field))
                    .flatten();
                let end_cone = (descriptor.upper.kind == SupportEndpointKind::Cone)
                    .then(|| cone_at(descriptor.upper.field))
                    .flatten();
                if append_smooth_shaft(
                    &mut sink,
                    &resolved,
                    (start_cone, end_cone),
                    (input.tip_penetration_mm, tip_scale, input.tip_drawing),
                    &tessellation,
                    &params,
                ) {
                    joined_cones.extend(
                        start_cone
                            .iter()
                            .chain(end_cone.iter())
                            .map(|c| c.id.as_str()),
                    );
                }
            } else if let Some(shaft) = entity.shaft {
                for (index, seg) in entity.segments.iter().enumerate() {
                    let hosts = EndpointHosts { root, host_knot };
                    let Some(ep) = resolve_segment_endpoints(shaft, seg, index, hosts) else {
                        continue;
                    };
                    append_segment_primitive(
                        &mut sink,
                        ep.start,
                        ep.end,
                        js_max(0.05, seg.diameter),
                        segment_bezier(seg),
                        &tessellation,
                    );
                    emit_joint(&mut sink, seg.bottom_joint.as_ref());
                    emit_joint(&mut sink, seg.top_joint.as_ref());
                }
            }

            if descriptor.lower.kind == SupportEndpointKind::Knot
                && descriptor.upper.kind == SupportEndpointKind::Knot
                && !descriptor.has_segments
                && let Some((brace,)) = entity.brace
            {
                let end_knot = knot_at(1);
                let (Some(start), Some(end)) = (host_knot, end_knot) else {
                    continue;
                };
                let profile_d = js_max(0.001, brace.profile.diameter);
                let start_d = js_max(0.05, start.diameter.unwrap_or(profile_d + 0.1) - 0.1);
                let end_d = js_max(0.05, end.diameter.unwrap_or(profile_d + 0.1) - 0.1);
                append_segment_primitive(
                    &mut sink,
                    start.pos,
                    end.pos,
                    (start_d + end_d) * 0.5,
                    brace.curve.as_ref().map(|c| &c.curve),
                    &tessellation,
                );
            }

            for field in descriptor.contact_fields {
                let kind = if Some(*field) == descriptor.lower.field {
                    descriptor.lower.kind
                } else {
                    descriptor.upper.kind
                };
                // The declared kind picks the primitive; every disk field is
                // declared `Disk` and every cone field `Cone`.
                if let Some((_, disk)) = entity.disks.iter().find(|(f, _)| f == field) {
                    debug_assert_eq!(kind, SupportEndpointKind::Disk);
                    append_contact_disk_with(
                        &mut sink,
                        disk,
                        tessellation.contact_cone_radial_segments,
                        input.tip_penetration_mm,
                        tip_scale,
                        input.tip_drawing,
                    );
                } else if let Some((_, cone)) = entity.cones.iter().find(|(f, _)| f == field) {
                    debug_assert_ne!(kind, SupportEndpointKind::Disk);
                    if joined_cones.contains(&cone.id.as_str()) {
                        continue;
                    }
                    append_contact_cone_with(
                        &mut sink,
                        cone,
                        tessellation.contact_cone_radial_segments,
                        input.tip_penetration_mm,
                        tip_scale,
                        input.tip_drawing,
                    );
                }
            }
        }
    }

    if input.draw_rafts {
        append_slice_rafts(&mut sink, input, &visible_root_ids, &root_model_key)?;
    }
    Ok(sink)
}

/// One raft under every root of `circles` (SoulCrafted, #556): the raft of
/// many sets standing together, drawn as [`append_slice_rafts`] draws one
/// model's, with no model to clear.
pub fn plate_raft_parts(
    circles: &[SupportBaseCircle],
    raft: &RaftSettings,
    robust_raft_geometry: bool,
) -> Result<TriangleSink, ClipperError> {
    let mut sink = TriangleSink::default();
    if raft.bottom_mode == RaftBottomMode::Off || circles.is_empty() {
        return Ok(sink);
    }
    append_raft_of(&mut sink, circles, raft, &[], &[], robust_raft_geometry)?;
    Ok(sink)
}

/// The raft half of `buildSupportAndRaftWorldTriangles`: one raft per model
/// key, with the global settings, the line raft only when there is no base.
fn append_slice_rafts(
    sink: &mut TriangleSink,
    input: &SupportSliceInput<'_>,
    visible_root_ids: &HashSet<&str>,
    root_model_key: &HashMap<&str, &str>,
) -> Result<(), ClipperError> {
    let raft = input.raft;
    if raft.bottom_mode == RaftBottomMode::Off {
        return Ok(());
    }
    let mut order: Vec<String> = Vec::new();
    let mut by_model: HashMap<String, Vec<SupportBaseCircle>> = HashMap::new();
    for root in input.state.roots.values() {
        let by_model_visible = input.visible_model_ids.contains(&root.base.model_id);
        if !by_model_visible && !visible_root_ids.contains(root.base.id.as_str()) {
            continue;
        }
        // `rootModelKeyById.get(id) ?? root.modelId ?? \`__root_${id}\``.
        let key = root_model_key
            .get(root.base.id.as_str())
            .map(|k| k.to_string())
            .unwrap_or_else(|| root.base.model_id.clone());
        by_model
            .entry(key.clone())
            .or_insert_with(|| {
                order.push(key);
                Vec::new()
            })
            .push(SupportBaseCircle {
                x: root.transform.pos.x,
                y: root.transform.pos.y,
                r: root.diameter * 0.5,
            });
    }
    let clearance =
        collect_model_plate_footprint(input.plate_clearance_models, raft_band_top_mm(raft))?;
    let clearance_cut = inflate_model_plate_clearance(&clearance)?;
    for key in order {
        let circles = &by_model[&key];
        if circles.is_empty() {
            continue;
        }
        append_raft_of(
            sink,
            circles,
            raft,
            &clearance,
            &clearance_cut,
            input.robust_raft_geometry,
        )?;
    }
    Ok(())
}

/// The raft under the roots `circles`, cleared of `clearance` (its lines of
/// `clearance_cut`).
fn append_raft_of(
    sink: &mut TriangleSink,
    circles: &[SupportBaseCircle],
    raft: &RaftSettings,
    clearance: &[PolygonWithHoles],
    clearance_cut: &[PolygonWithHoles],
    robust_raft_geometry: bool,
) -> Result<(), ClipperError> {
    {
        let parts = build_raft_footprint_meshes(circles, raft, Some(clearance))?;
        if parts.base_mesh.is_none() && parts.footprint.is_empty() {
            return Ok(());
        }
        let trimmed = !is_untrimmed_footprint(&parts.footprint);
        let robust_base = (robust_raft_geometry && trimmed && parts.base_mesh.is_some())
            .then(|| robust_chamfered_base(&parts.footprint, raft));
        if let Some(base) = robust_base.as_ref().or(parts.base_mesh.as_ref()) {
            append_geometry_triangles(sink, base);
        } else if raft.bottom_mode == RaftBottomMode::Line {
            let nodes: Vec<Vector2> = circles.iter().map(|c| Vector2::new(c.x, c.y)).collect();
            let pairs = build_line_raft_edge_pairs(
                &nodes,
                BuildLineRaftEdgePairsOptions {
                    has_border_ring: !parts.footprint.is_empty(),
                    keep_factor: Some(8.0),
                    abs_max_len: Some(220.0),
                    enforce_connected: Some(true),
                    include_spanning_tree_backbone: None,
                },
            );
            let beam_height = js_max(0.01, raft.line_height_mm);
            let edges: Vec<(Vector2, Vector2)> =
                pairs.iter().map(|&(a, b)| (nodes[a], nodes[b])).collect();
            let union_edges =
                filter_line_raft_edges(&edges, Some(clearance_cut), raft.line_width_mm);
            let union = generate_unioned_line_raft_mesh(
                &union_edges,
                raft.line_width_mm,
                beam_height,
                None,
                None,
            )?;
            if union.vertex_count() > 0 {
                append_geometry_triangles(sink, &union);
            } else {
                for (a, b) in &union_edges {
                    let beam = generate_chamfered_beam(
                        Vec3::new(a.x, a.y, 0.0),
                        Vec3::new(b.x, b.y, 0.0),
                        raft.line_width_mm,
                        beam_height,
                        90.0,
                    );
                    append_geometry_triangles(sink, &beam);
                }
            }
        }
        if robust_raft_geometry && raft.wall_enabled {
            let base_z = raft_wall_base_height_mm(raft);
            if let Some(wall) = robust_wall_from_polygons(&parts.footprint, raft, base_z)? {
                append_geometry_triangles(sink, &wall);
            }
        } else if let Some(wall) = &parts.wall_mesh {
            append_geometry_triangles(sink, wall);
        }
    }
    Ok(())
}

/// One tube's centre line and its edges' (start, end) radii.
type Chain = (Vec<ChainPoint>, Vec<(f64, f64)>);

/// A shaft's resolved segments as smooth tubes: each run of connected
/// segments is one chain, Beziers sampled as DragonFruit samples them, a
/// bicone wherever DragonFruit would draw a joint sphere between segments.
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
            append_joint_sphere(sink, ball, ends.r_tip * 2.0, segments);
        }
    }
    true
}
