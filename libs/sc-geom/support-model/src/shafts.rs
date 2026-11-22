//! Shafts: where each segment of a support runs, where a knot on it sits,
//! and how a segment is split in two by a new joint.

use indexmap::IndexMap;

use crate::bezier::{get_bezier_point_at_t, subdivide_cubic_bezier};
use crate::constants::get_joint_diameter;
use crate::support_kinds::{SupportEndpointKind, SupportTypeId, get_support_type_descriptor};
use crate::tips::{ContactCone, get_final_socket_position};
use crate::types::{
    Branch, ContactDisk, Joint, Kickstand, Knot, Roots, Segment, SegmentShape, Stick, Stump, Trunk,
    Twig, Vec3,
};

/// A support with a shaft.
#[derive(Debug, Clone, Copy)]
pub enum ShaftEntity<'a> {
    Trunk(&'a Trunk),
    Branch(&'a Branch),
    Twig(&'a Twig),
    Stick(&'a Stick),
    Stump(&'a Stump),
    Kickstand(&'a Kickstand),
}

/// A tip at one end of a shaft.
#[derive(Debug, Clone, Copy)]
pub enum ContactRef<'a> {
    Cone(&'a ContactCone),
    Disk(&'a ContactDisk),
}

impl<'a> ShaftEntity<'a> {
    pub fn id(&self) -> &'a str {
        match self {
            ShaftEntity::Trunk(e) => &e.base.id,
            ShaftEntity::Branch(e) => &e.base.id,
            ShaftEntity::Twig(e) => &e.base.id,
            ShaftEntity::Stick(e) => &e.base.id,
            ShaftEntity::Stump(e) => &e.base.id,
            ShaftEntity::Kickstand(e) => &e.base.id,
        }
    }

    pub fn type_id(&self) -> SupportTypeId {
        match self {
            ShaftEntity::Trunk(_) => SupportTypeId::Trunk,
            ShaftEntity::Branch(_) => SupportTypeId::Branch,
            ShaftEntity::Twig(_) => SupportTypeId::Twig,
            ShaftEntity::Stick(_) => SupportTypeId::Stick,
            ShaftEntity::Stump(_) => SupportTypeId::Stump,
            ShaftEntity::Kickstand(_) => SupportTypeId::Kickstand,
        }
    }

    pub fn segments(&self) -> &'a [Segment] {
        match self {
            ShaftEntity::Trunk(e) => &e.segments,
            ShaftEntity::Branch(e) => &e.segments,
            ShaftEntity::Twig(e) => &e.segments,
            ShaftEntity::Stick(e) => &e.segments,
            ShaftEntity::Stump(e) => &e.segments,
            ShaftEntity::Kickstand(e) => &e.segments,
        }
    }

    /// The tip held in `field`, when the support has it.
    pub fn contact(&self, field: &str) -> Option<ContactRef<'a>> {
        Some(match (self, field) {
            (ShaftEntity::Trunk(e), "contactCone") => ContactRef::Cone(e.contact_cone.as_ref()?),
            (ShaftEntity::Branch(e), "contactCone") => ContactRef::Cone(e.contact_cone.as_ref()?),
            (ShaftEntity::Stump(e), "contactCone") => ContactRef::Cone(&e.contact_cone),
            (ShaftEntity::Stick(e), "contactConeA") => ContactRef::Cone(&e.contact_cone_a),
            (ShaftEntity::Stick(e), "contactConeB") => ContactRef::Cone(&e.contact_cone_b),
            (ShaftEntity::Twig(e), "contactDiskA") => ContactRef::Disk(&e.contact_disk_a),
            (ShaftEntity::Twig(e), "contactDiskB") => ContactRef::Disk(&e.contact_disk_b),
            _ => return None,
        })
    }

    /// The socket of the tip the shaft climbs to, when it ends in a cone.
    fn upper_socket(&self) -> Option<Vec3> {
        let upper = get_support_type_descriptor(self.type_id()).upper;
        match self.contact(upper.field?)? {
            ContactRef::Cone(cone) => Some(get_final_socket_position(cone)),
            ContactRef::Disk(_) => None,
        }
    }
}

/// What a shaft stands on that the support does not hold itself.
#[derive(Debug, Clone, Copy, Default)]
pub struct EndpointHosts<'a> {
    /// Its root, for a kind with a root of its own.
    pub root: Option<&'a Roots>,
    /// The knot it hangs from.
    pub host_knot: Option<&'a Knot>,
}

/// Where a segment starts and ends.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentEndpoints {
    pub start: Vec3,
    pub end: Vec3,
}

/// Where a shaft leaves its root: on top of the disk and the cone.
pub fn root_top(root: &Roots) -> Vec3 {
    let finite = |v: f64| v.is_finite().then_some(v);
    let disk = finite(root.disk_height).unwrap_or(0.0);
    let cone = finite(root.cone_height)
        .or_else(|| root.height.and_then(finite))
        .unwrap_or(0.0);
    root.transform.pos.add(Vec3::new(0.0, 0.0, disk + cone))
}

/// Where `segment` (the `index`-th of `shaft`) runs, or none when the shaft
/// needs a host it was not given.
///
/// It starts at its own bottom joint (except on a shaft hanging from a knot,
/// which starts at the knot), else where the shaft starts (the root, the
/// knot) for the first segment, else where the one below ends. It ends at its
/// top joint, else at what the shaft ends in: a knot, a tip's socket, or a
/// short stub straight up.
pub fn resolve_segment_endpoints(
    shaft: ShaftEntity<'_>,
    segment: &Segment,
    index: usize,
    hosts: EndpointHosts<'_>,
) -> Option<SegmentEndpoints> {
    let kind = get_support_type_descriptor(shaft.type_id());
    if !kind.has_segments {
        return None;
    }
    let hangs = kind.lower.kind == SupportEndpointKind::Knot;
    let foot = match kind.lower.kind {
        SupportEndpointKind::PlateRoot => hosts.root.map(root_top),
        SupportEndpointKind::Knot => hosts.host_knot.map(|k| k.pos),
        _ => None,
    };
    if !kind.segments_carry_both_joints && foot.is_none() {
        return None;
    }
    let own_bottom = segment.bottom_joint.as_ref().map(|j| j.pos);
    let start = match own_bottom {
        Some(bottom) if !hangs => bottom,
        _ if index == 0 => foot?,
        _ => {
            let below = shaft
                .segments()
                .get(index - 1)
                .and_then(|s| s.top_joint.as_ref());
            below.map(|j| j.pos).or(foot).or(own_bottom)?
        }
    };
    let end = match &segment.top_joint {
        Some(top) => top.pos,
        None if kind.upper.kind == SupportEndpointKind::Knot => hosts.host_knot?.pos,
        None => shaft
            .upper_socket()
            .unwrap_or_else(|| start.add(Vec3::new(0.0, 0.0, kind.shaft_fallback.stub_length_mm))),
    };
    Some(SegmentEndpoints { start, end })
}

/// Where a knot `t` along a segment from `start` to `end` sits, on the
/// segment's curve when it has one.
pub fn calculate_knot_position_on_segment_from_t(
    start: Vec3,
    end: Vec3,
    segment: &Segment,
    t: f64,
) -> Vec3 {
    let t = t.clamp(0.0, 1.0);
    match &segment.shape {
        SegmentShape::Bezier(c) => {
            get_bezier_point_at_t(start, c.control_point1, c.control_point2, end, t)
        }
        SegmentShape::Straight => Vec3::new(
            start.x + (end.x - start.x) * t,
            start.y + (end.y - start.y) * t,
            start.z + (end.z - start.z) * t,
        ),
    }
}

/// A knot's new segment and place along it after its segment was split.
#[derive(Debug, Clone, PartialEq)]
pub struct KnotSplitRemap {
    pub knot_id: String,
    pub parent_shaft_id: String,
    pub t: f64,
}

/// Where a knot on segment `split` goes when that segment is split at
/// `split_t` into `bottom` and `top`: onto the half it falls on, at the same
/// point (a cubic split at `s` puts the old `t` at `t / s` on the lower half
/// and `(t - s) / (1 - s)` on the upper). None for a knot elsewhere, without
/// a place, or a split too near an end.
pub fn remap_knot_across_split(
    knot: &Knot,
    split: &str,
    bottom: &str,
    top: &str,
    split_t: f64,
) -> Option<KnotSplitRemap> {
    if knot.parent_shaft_id != split {
        return None;
    }
    let t = knot.t?;
    if !(split_t > 1e-6 && split_t < 1.0 - 1e-6) {
        return None;
    }
    let t = t.clamp(0.0, 1.0);
    let (parent, t) = if t <= split_t {
        (bottom, t / split_t)
    } else {
        (top, (t - split_t) / (1.0 - split_t))
    };
    Some(KnotSplitRemap {
        knot_id: knot.id.clone(),
        parent_shaft_id: parent.to_owned(),
        t,
    })
}

/// A support whose shaft can be replaced.
pub trait ShaftedEntity: Clone {
    fn as_shaft(&self) -> ShaftEntity<'_>;
    fn segments_mut(&mut self) -> &mut Vec<Segment>;
}

macro_rules! shafted {
    ($($kind:ident),*) => {$(
        impl ShaftedEntity for $kind {
            fn as_shaft(&self) -> ShaftEntity<'_> {
                ShaftEntity::$kind(self)
            }
            fn segments_mut(&mut self) -> &mut Vec<Segment> {
                &mut self.segments
            }
        }
    )*};
}
shafted!(Trunk, Branch, Twig, Stick, Stump, Kickstand);

/// What a shaft's first segment may stand on when it is split.
#[derive(Debug, Clone, Copy, Default)]
pub struct SplitHosts<'a> {
    pub root: Option<&'a Roots>,
    pub host_knot: Option<&'a Knot>,
}

/// `entity` with its segment `segment_id` split at `at` by a new joint (as
/// wide as that segment calls for): the lower half keeps the id, the upper
/// gets a new one, and a curve is split along itself at `split_t`. The knots
/// on that segment that move come back too. An unknown segment leaves the
/// entity as it was.
pub fn split_support_shaft<T: ShaftedEntity>(
    entity: &T,
    segment_id: &str,
    at: Vec3,
    split_t: Option<f64>,
    hosts: SplitHosts<'_>,
    knots: Option<&IndexMap<String, Knot>>,
    new_id: &mut dyn FnMut() -> String,
) -> (T, Vec<KnotSplitRemap>) {
    let shaft = entity.as_shaft();
    let segments = shaft.segments();
    let Some(index) = segments.iter().position(|s| s.id == segment_id) else {
        return (entity.clone(), Vec::new());
    };
    let segment = &segments[index];
    let joint = Joint {
        id: new_id(),
        pos: at,
        diameter: get_joint_diameter(segment.diameter),
    };
    let mut lower = segment.clone();
    lower.top_joint = Some(joint.clone());
    let mut upper = segment.clone();
    upper.id = new_id();
    upper.bottom_joint = Some(joint);

    if let (SegmentShape::Bezier(curve), Some(t)) = (&segment.shape, split_t)
        && let Some((from, to)) = split_ends(shaft, index, at, hosts)
    {
        let [left, right] =
            subdivide_cubic_bezier(from, curve.control_point1, curve.control_point2, to, t);
        if let Some(c) = lower.bezier_mut() {
            (c.control_point1, c.control_point2) = (left[1], left[2]);
        }
        if let Some(c) = upper.bezier_mut() {
            (c.control_point1, c.control_point2) = (right[1], right[2]);
        }
    }

    let upper_id = upper.id.clone();
    let mut next = entity.clone();
    let shaft = next.segments_mut();
    shaft.splice(index..=index, [lower, upper]);
    let moved = match (knots, split_t) {
        (Some(knots), Some(t)) => knots
            .values()
            .filter_map(|k| remap_knot_across_split(k, segment_id, segment_id, &upper_id, t))
            .collect(),
        _ => Vec::new(),
    };
    (next, moved)
}

/// Where the `index`-th segment of `shaft` runs, for splitting its curve. A
/// kind that splits where it cannot tell starts at the split point instead.
fn split_ends(
    shaft: ShaftEntity<'_>,
    index: usize,
    at: Vec3,
    hosts: SplitHosts<'_>,
) -> Option<(Vec3, Vec3)> {
    let kind = get_support_type_descriptor(shaft.type_id());
    let segments = shaft.segments();
    let segment = &segments[index];
    let lost = kind
        .shaft_fallback
        .start_falls_back_to_split_point
        .then_some(at);
    let hangs = kind.lower.kind == SupportEndpointKind::Knot;
    let own_bottom = segment.bottom_joint.as_ref().map(|j| j.pos);
    let from = match own_bottom {
        Some(bottom) if !hangs => Some(bottom),
        _ if index > 0 => segments[index - 1]
            .top_joint
            .as_ref()
            .map(|j| j.pos)
            .or(lost),
        _ if kind.lower.kind == SupportEndpointKind::PlateRoot && hosts.root.is_some() => {
            let root = hosts.root?;
            let p = root.transform.pos;
            Some(Vec3::new(
                p.x,
                p.y,
                p.z + root.disk_height + root.cone_height,
            ))
        }
        _ if hangs => hosts.host_knot.map(|k| k.pos).or(lost),
        _ => own_bottom.or(lost),
    }?;
    let to = match &segment.top_joint {
        Some(top) => top.pos,
        None => shaft.upper_socket().unwrap_or_else(|| {
            Vec3::new(from.x, from.y, from.z + kind.shaft_fallback.stub_length_mm)
        }),
    };
    Some((from, to))
}
