//! DragonFruit `src/supports/SupportPrimitives/Joint/jointUtils.ts`.
//!
//! DragonFruit mints ids with `uuidv4()`; callers pass the generator, so a run
//! can be deterministic. `moveJoint` reads `settings.shaft.maxAngleDeg` and the
//! knots from the store; callers pass both, and the model's collision field
//! where DragonFruit passes the mesh.

use indexmap::IndexMap;

use crate::supports::constants::get_joint_diameter;
use crate::supports::curves::bezier_utils::subdivide_cubic_bezier;
use crate::supports::support_primitives::contact_cone::contact_cone_utils::get_final_socket_position;
use crate::supports::support_primitives::knot::knot_utils::{
    KnotSplitRemap, remap_knot_across_split,
};
use crate::supports::support_primitives::knot::segment_endpoints::{ContactRef, ShaftEntity};
use crate::supports::support_kinds::{SupportEndpointKind, get_support_type_descriptor};
use crate::supports::types::{
    Branch, Joint, Kickstand, Knot, Roots, Segment, SegmentShape, Stick, Stump, Trunk, Twig, Vec3,
};

/// A shafted entity that can be rebuilt with new segments.
pub trait ShaftedEntity: Clone {
    fn as_shaft(&self) -> ShaftEntity<'_>;
    fn segments_mut(&mut self) -> &mut Vec<Segment>;
}

macro_rules! shafted {
    ($($ty:ident),*) => {$(
        impl ShaftedEntity for $ty {
            fn as_shaft(&self) -> ShaftEntity<'_> {
                ShaftEntity::$ty(self)
            }
            fn segments_mut(&mut self) -> &mut Vec<Segment> {
                &mut self.segments
            }
        }
    )*};
}
shafted!(Trunk, Branch, Twig, Stick, Stump, Kickstand);

fn remap_knots_for_split(
    knots: Option<&IndexMap<String, Knot>>,
    original_segment_id: &str,
    top_segment_id: &str,
    split_t: Option<f64>,
) -> Vec<KnotSplitRemap> {
    let (Some(knots), Some(split_t)) = (knots, split_t) else {
        return Vec::new();
    };
    knots
        .values()
        .filter_map(|knot| {
            remap_knot_across_split(
                knot,
                original_segment_id,
                original_segment_id,
                top_segment_id,
                split_t,
            )
        })
        .collect()
}

struct SplitResult {
    new_segments: Vec<Segment>,
    top_segment_id: String,
}

/// Generic segment splitter that preserves bezier geometry via De Casteljau
/// subdivision and falls back to a linear split. Start and end resolution are
/// supplied by split_support_shaft, which reads them from the registry.
fn split_segment_array(
    segments: &[Segment],
    segment_id: &str,
    split_point: Vec3,
    split_t: Option<f64>,
    resolve_start: &dyn Fn(usize, &Segment) -> Option<Vec3>,
    resolve_end: &dyn Fn(Vec3, &Segment) -> Option<Vec3>,
    new_id: &mut dyn FnMut() -> String,
) -> Option<SplitResult> {
    let seg_index = segments.iter().position(|s| s.id == segment_id)?;
    let original_segment = &segments[seg_index];
    let joint = Joint {
        id: new_id(),
        pos: split_point,
        diameter: get_joint_diameter(original_segment.diameter),
    };

    let splice = |bottom: Segment, top: Segment| {
        let mut new_segments = Vec::with_capacity(segments.len() + 1);
        new_segments.extend_from_slice(&segments[..seg_index]);
        new_segments.push(bottom);
        new_segments.push(top);
        new_segments.extend_from_slice(&segments[seg_index + 1..]);
        new_segments
    };

    if let (SegmentShape::Bezier(curve), Some(split_t)) = (&original_segment.shape, split_t) {
        let start_pos = resolve_start(seg_index, original_segment);
        let end_pos = start_pos.and_then(|start| resolve_end(start, original_segment));
        if let (Some(start_pos), Some(end_pos)) = (start_pos, end_pos) {
            let [left_curve, right_curve] = subdivide_cubic_bezier(
                start_pos,
                curve.control_point1,
                curve.control_point2,
                end_pos,
                split_t,
            );
            let mut bottom_segment = original_segment.clone();
            bottom_segment.top_joint = Some(joint.clone());
            if let Some(bottom_curve) = bottom_segment.bezier_mut() {
                bottom_curve.control_point1 = left_curve[1];
                bottom_curve.control_point2 = left_curve[2];
            }
            let mut top_segment = original_segment.clone();
            top_segment.id = new_id();
            top_segment.bottom_joint = Some(joint.clone());
            top_segment.top_joint = original_segment.top_joint.clone();
            if let Some(top_curve) = top_segment.bezier_mut() {
                top_curve.control_point1 = right_curve[1];
                top_curve.control_point2 = right_curve[2];
            }
            let top_segment_id = top_segment.id.clone();
            return Some(SplitResult {
                new_segments: splice(bottom_segment, top_segment),
                top_segment_id,
            });
        }
    }

    let mut bottom_segment = original_segment.clone();
    bottom_segment.top_joint = Some(joint.clone());
    let mut top_segment = original_segment.clone();
    top_segment.id = new_id();
    top_segment.bottom_joint = Some(joint);
    top_segment.top_joint = original_segment.top_joint.clone();
    let top_segment_id = top_segment.id.clone();
    Some(SplitResult {
        new_segments: splice(bottom_segment, top_segment),
        top_segment_id,
    })
}

/// The host a type's lower endpoint needs, when it needs one.
#[derive(Debug, Clone, Copy, Default)]
pub struct SplitHosts<'a> {
    pub root: Option<&'a Roots>,
    pub host_knot: Option<&'a Knot>,
}

/// Splits a shaft segment on any type, inserting a joint at `split_point`.
/// Start and end come from the declared endpoints; `shaft_fallback` covers
/// what to do when neither resolves.
///
/// An unknown segment leaves the entity unchanged (DragonFruit warns
/// `[JointUtils] Segment not found`).
pub fn split_support_shaft<T: ShaftedEntity>(
    entity: &T,
    segment_id: &str,
    split_point: Vec3,
    split_t: Option<f64>,
    hosts: SplitHosts<'_>,
    knots: Option<&IndexMap<String, Knot>>,
    new_id: &mut dyn FnMut() -> String,
) -> (T, Vec<KnotSplitRemap>) {
    let shaft = entity.as_shaft();
    let descriptor = get_support_type_descriptor(shaft.type_id());
    let stub_length_mm = descriptor.shaft_fallback.stub_length_mm;
    let unresolved = descriptor
        .shaft_fallback
        .start_falls_back_to_split_point
        .then_some(split_point);
    let segments = shaft.segments();

    let resolve_start = |seg_index: usize, seg: &Segment| -> Option<Vec3> {
        // A knot-hosted shaft starts at its host; the others read the joint
        // first, matching resolve_segment_endpoints.
        if descriptor.lower.kind != SupportEndpointKind::Knot
            && let Some(bottom) = &seg.bottom_joint
        {
            return Some(bottom.pos);
        }
        if seg_index == 0 {
            if descriptor.lower.kind == SupportEndpointKind::PlateRoot
                && let Some(root) = hosts.root
            {
                let r_pos = root.transform.pos;
                return Some(Vec3::new(
                    r_pos.x,
                    r_pos.y,
                    r_pos.z + root.disk_height + root.cone_height,
                ));
            }
            if descriptor.lower.kind == SupportEndpointKind::Knot {
                return hosts.host_knot.map(|knot| knot.pos).or(unresolved);
            }
            return seg.bottom_joint.as_ref().map(|j| j.pos).or(unresolved);
        }
        segments[seg_index - 1]
            .top_joint
            .as_ref()
            .map(|j| j.pos)
            .or(unresolved)
    };

    let resolve_end = |start_pos: Vec3, seg: &Segment| -> Option<Vec3> {
        if let Some(top) = &seg.top_joint {
            return Some(top.pos);
        }
        let upper = descriptor.upper;
        if let Some(field) = upper.field
            && matches!(
                upper.kind,
                SupportEndpointKind::Cone | SupportEndpointKind::Disk
            )
            && let Some(ContactRef::Cone(cone)) = shaft.contact(field)
        {
            return Some(get_final_socket_position(cone));
        }
        Some(Vec3::new(
            start_pos.x,
            start_pos.y,
            start_pos.z + stub_length_mm,
        ))
    };

    let Some(res) = split_segment_array(
        segments,
        segment_id,
        split_point,
        split_t,
        &resolve_start,
        &resolve_end,
        new_id,
    ) else {
        return (entity.clone(), Vec::new());
    };

    let mut next = entity.clone();
    *next.segments_mut() = res.new_segments;
    let knot_remaps = remap_knots_for_split(knots, segment_id, &res.top_segment_id, split_t);
    (next, knot_remaps)
}
