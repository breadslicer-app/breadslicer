//! DragonFruit `src/supports/__tests__/contactDiskNib.test.ts`,
//! `jointInsertionKnotRemap.test.ts` and the taper half of `shaftTaperGeometry.test.ts`.

use indexmap::IndexMap;
use support_gen_df::supports::curves::bezier_utils::{
    get_bezier_point_at_t, subdivide_cubic_bezier,
};
use support_gen_df::supports::support_primitives::contact_cone::types::{
    ContactDiskProfile, DiskTag,
};
use support_gen_df::supports::support_primitives::contact_disk::contact_disk_utils::calculate_disk_thickness;
use support_gen_df::supports::support_primitives::joint::joint_utils::{
    SplitHosts, split_support_shaft,
};
use support_gen_df::supports::support_primitives::knot::knot_utils::{
    calculate_knot_position_on_segment_from_t, remap_knot_across_split,
};
use support_gen_df::supports::support_primitives::shaft::shaft_taper_geometry::taper_width_along;
use support_gen_df::supports::types::*;

// --- contactDiskNib.test.ts ---

const FLAT_NORMAL: Vec3 = Vec3::new(0.0, 0.0, 1.0);

/// A surface steep enough that the angle-driven standoff dominates the nib.
fn steep_axis() -> Vec3 {
    let a = 80.0_f64.to_radians();
    Vec3::new(a.sin(), 0.0, a.cos())
}

/// `createDefaultSettings().tip`: Settings/types.ts with Settings/defaults.ts values.
fn nib_profile(contact_diameter_mm: Option<f64>) -> ContactDiskProfile {
    ContactDiskProfile {
        kind: DiskTag::Disk,
        disk_thickness_mm: Some(0.1),
        max_standoff_mm: Some(1.5),
        standoff_angle_threshold: Some(std::f64::consts::PI / 4.0),
        contact_diameter_mm,
    }
}

// The round tip is centered on the nib's top face, so a nib shorter than the
// ball's radius drops the ball's underside below the flat contact face.
#[test]
fn a_wide_contact_grows_the_nib_so_the_round_tip_stays_above_the_contact_face() {
    for contact_diameter_mm in [0.3, 0.4, 1.0, 2.0, 4.0] {
        let thickness = calculate_disk_thickness(
            FLAT_NORMAL,
            FLAT_NORMAL,
            Some(&nib_profile(Some(contact_diameter_mm))),
        );
        let tip_ball_lowest_mm = thickness - contact_diameter_mm / 2.0;
        assert!(
            tip_ball_lowest_mm >= 0.0,
            "contact {contact_diameter_mm}mm: the tip ball dips {:.3}mm below the contact face",
            -tip_ball_lowest_mm
        );
    }
}

#[test]
fn a_steep_surface_still_thickens_the_nib_past_the_tip_ball_radius() {
    let profile = nib_profile(Some(0.3));

    let flat_thickness = calculate_disk_thickness(FLAT_NORMAL, FLAT_NORMAL, Some(&profile));
    let steep_thickness = calculate_disk_thickness(FLAT_NORMAL, steep_axis(), Some(&profile));

    assert_eq!(
        flat_thickness, 0.15,
        "a flat contact holds the nib at the ball radius"
    );
    assert!(
        steep_thickness > 0.3,
        "a steep contact must keep its standoff, got {steep_thickness}mm"
    );
}

#[test]
fn a_profile_that_declares_no_contact_diameter_keeps_the_plain_nib_thickness() {
    let profile = nib_profile(None);
    assert_eq!(
        calculate_disk_thickness(FLAT_NORMAL, FLAT_NORMAL, Some(&profile)),
        profile.disk_thickness_mm.unwrap()
    );
}

// --- jointInsertionKnotRemap.test.ts ---
// Issue #204: inserting a joint splits a host segment into a bottom half (keeps the
// original id) and a top half (new id). Attached knots must be re-anchored so their
// absolute world position does not change, otherwise branches/leaves slide down.

fn dist(a: Vec3, b: Vec3) -> f64 {
    a.distance_to(b)
}

fn make_knot(id: &str, parent_shaft_id: &str, t: Option<f64>) -> Knot {
    Knot {
        id: id.to_owned(),
        parent_shaft_id: parent_shaft_id.to_owned(),
        t,
        ..Knot::default()
    }
}

#[test]
fn remap_returns_none_when_the_knot_is_not_on_the_split_segment() {
    let knot = make_knot("knot-1", "other-seg", Some(0.5));
    assert!(remap_knot_across_split(&knot, "seg-orig", "seg-orig", "seg-top", 0.5).is_none());
}

#[test]
fn remap_returns_none_when_the_knot_has_no_t() {
    let knot = make_knot("knot-1", "seg-orig", None);
    assert!(remap_knot_across_split(&knot, "seg-orig", "seg-orig", "seg-top", 0.5).is_none());
}

#[test]
fn remap_returns_none_for_a_degenerate_split() {
    let knot = make_knot("knot-1", "seg-orig", Some(0.5));
    assert!(remap_knot_across_split(&knot, "seg-orig", "seg-orig", "seg-top", 0.0).is_none());
    assert!(remap_knot_across_split(&knot, "seg-orig", "seg-orig", "seg-top", 1.0).is_none());
}

#[test]
fn remap_keeps_a_below_split_knot_on_the_bottom_segment() {
    let remap = remap_knot_across_split(
        &make_knot("knot-1", "seg-orig", Some(0.2)),
        "seg-orig",
        "seg-orig",
        "seg-top",
        0.5,
    )
    .unwrap();
    assert_eq!(remap.parent_shaft_id, "seg-orig");
    assert!((remap.t - 0.4).abs() < 1e-9);
}

#[test]
fn remap_moves_an_above_split_knot_to_the_top_segment() {
    let remap = remap_knot_across_split(
        &make_knot("knot-1", "seg-orig", Some(0.8)),
        "seg-orig",
        "seg-orig",
        "seg-top",
        0.5,
    )
    .unwrap();
    assert_eq!(remap.parent_shaft_id, "seg-top");
    assert!((remap.t - 0.6).abs() < 1e-9);
}

#[test]
fn remap_keeps_a_knot_exactly_at_the_split_on_the_bottom_segment() {
    let remap = remap_knot_across_split(
        &make_knot("knot-1", "seg-orig", Some(0.5)),
        "seg-orig",
        "seg-orig",
        "seg-top",
        0.5,
    )
    .unwrap();
    assert_eq!(remap.parent_shaft_id, "seg-orig");
    assert!((remap.t - 1.0).abs() < 1e-9);
}

#[test]
fn remap_preserves_world_position_on_a_straight_segment() {
    let start = Vec3::new(0.0, 0.0, 0.0);
    let end = Vec3::new(0.0, 0.0, 10.0);
    let mut straight = Segment::straight("seg-orig", 1.0);
    straight.shape = SegmentShape::Straight {
        explicit: true,
        stray_bias: None,
    };

    let split_t = 0.3;
    let split = Vec3::new(0.0, 0.0, 3.0);

    for original_t in [0.1, 0.3, 0.55, 0.9] {
        let world_before =
            calculate_knot_position_on_segment_from_t(start, end, &straight, original_t);
        let knot = make_knot("knot-1", "seg-orig", Some(original_t));
        let remap =
            remap_knot_across_split(&knot, "seg-orig", "seg-orig", "seg-top", split_t).unwrap();

        let (s, e) = if remap.parent_shaft_id == "seg-orig" {
            (start, split)
        } else {
            (split, end)
        };
        let world_after = calculate_knot_position_on_segment_from_t(s, e, &straight, remap.t);

        assert!(
            dist(world_before, world_after) < 1e-6,
            "t={original_t} drifted {}",
            dist(world_before, world_after)
        );
    }
}

fn bezier_segment(id: &str, cp1: Vec3, cp2: Vec3) -> Segment {
    let mut segment = Segment::straight(id, 1.0);
    segment.shape = SegmentShape::Bezier(BezierCurve {
        control_point1: cp1,
        control_point2: cp2,
        start_tangent: Vec3::new(0.0, 0.0, 1.0),
        end_tangent: Vec3::new(0.0, 0.0, 1.0),
        tension: 0.5,
        bias: 0.5,
        resolution: 16.0,
    });
    segment
}

#[test]
fn remap_preserves_world_position_on_a_bezier_segment() {
    let p0 = Vec3::new(0.0, 0.0, 0.0);
    let cp1 = Vec3::new(4.0, 0.0, 2.0);
    let cp2 = Vec3::new(4.0, 0.0, 8.0);
    let p3 = Vec3::new(0.0, 0.0, 10.0);

    let split_t = 0.4;
    let [left_curve, right_curve] = subdivide_cubic_bezier(p0, cp1, cp2, p3, split_t);

    // Bottom half keeps original id; top half is the right sub-curve.
    let bottom_seg = bezier_segment("seg-orig", left_curve[1], left_curve[2]);
    let top_seg = bezier_segment("seg-top", right_curve[1], right_curve[2]);

    for original_t in [0.1, 0.4, 0.65, 0.95] {
        let world_before = get_bezier_point_at_t(p0, cp1, cp2, p3, original_t);
        let knot = make_knot("knot-1", "seg-orig", Some(original_t));
        let remap =
            remap_knot_across_split(&knot, "seg-orig", "seg-orig", "seg-top", split_t).unwrap();

        let world_after = if remap.parent_shaft_id == "seg-orig" {
            calculate_knot_position_on_segment_from_t(
                left_curve[0],
                left_curve[3],
                &bottom_seg,
                remap.t,
            )
        } else {
            calculate_knot_position_on_segment_from_t(
                right_curve[0],
                right_curve[3],
                &top_seg,
                remap.t,
            )
        };

        assert!(
            dist(world_before, world_after) < 1e-6,
            "bezier t={original_t} drifted {}",
            dist(world_before, world_after)
        );
    }
}

fn split_fixture() -> (Roots, Trunk) {
    let root = Roots {
        base: SupportEntity {
            id: "root-1".to_owned(),
            ..SupportEntity::default()
        },
        transform: Transform {
            scale: Some(Vec3::new(1.0, 1.0, 1.0)),
            ..Transform::default()
        },
        ..Roots::default()
    };
    let mut segment = Segment::straight("seg-orig", 1.0);
    segment.shape = SegmentShape::Straight {
        explicit: true,
        stray_bias: None,
    };
    segment.top_joint = Some(Joint {
        id: "j-top".to_owned(),
        pos: Vec3::new(0.0, 0.0, 10.0),
        diameter: 1.5,
    });
    let trunk = Trunk {
        base: SupportEntity {
            id: "trunk-1".to_owned(),
            model_id: "model-1".to_owned(),
            type_id: Some(support_gen_df::supports::support_type_registry::SupportTypeId::Trunk),
            ..SupportEntity::default()
        },
        root_id: "root-1".to_owned(),
        segments: vec![segment],
        ..Trunk::default()
    };
    (root, trunk)
}

fn counter_ids() -> impl FnMut() -> String {
    let mut next = 0;
    move || {
        next += 1;
        format!("id-{next}")
    }
}

#[test]
fn split_support_shaft_rehosts_an_above_split_knot_onto_the_new_top_segment() {
    let (root, trunk) = split_fixture();
    let knots: IndexMap<String, Knot> = [
        make_knot("k-below", "seg-orig", Some(0.2)),
        make_knot("k-above", "seg-orig", Some(0.8)),
        make_knot("k-other", "seg-elsewhere", Some(0.5)),
    ]
    .into_iter()
    .map(|k| (k.id.clone(), k))
    .collect();

    let (after, knot_remaps) = split_support_shaft(
        &trunk,
        "seg-orig",
        Vec3::new(0.0, 0.0, 5.0),
        Some(0.5),
        SplitHosts {
            root: Some(&root),
            host_knot: None,
        },
        Some(&knots),
        &mut counter_ids(),
    );

    // Bottom keeps original id, top is new.
    assert_eq!(after.segments.len(), 2);
    assert_eq!(after.segments[0].id, "seg-orig");
    let top_id = after.segments[1].id.clone();
    assert_ne!(top_id, "seg-orig");
    // The joint id is minted before the top segment's, as uuidv4() is called.
    assert_eq!(after.segments[0].top_joint.as_ref().unwrap().id, "id-1");
    assert_eq!(top_id, "id-2");
    assert_eq!(after.segments[1].top_joint.as_ref().unwrap().id, "j-top");
    assert!((after.segments[0].top_joint.as_ref().unwrap().diameter - 1.1).abs() < 1e-12);

    // Only the two knots on seg-orig are remapped.
    assert_eq!(knot_remaps.len(), 2);
    let below = knot_remaps.iter().find(|r| r.knot_id == "k-below").unwrap();
    let above = knot_remaps.iter().find(|r| r.knot_id == "k-above").unwrap();
    assert_eq!(below.parent_shaft_id, "seg-orig");
    assert!((below.t - 0.4).abs() < 1e-9);
    assert_eq!(above.parent_shaft_id, top_id);
    assert!((above.t - 0.6).abs() < 1e-9);
}

#[test]
fn split_support_shaft_emits_no_remaps_when_no_knots_are_supplied() {
    let (root, trunk) = split_fixture();
    let (_, knot_remaps) = split_support_shaft(
        &trunk,
        "seg-orig",
        Vec3::new(0.0, 0.0, 5.0),
        Some(0.5),
        SplitHosts {
            root: Some(&root),
            host_knot: None,
        },
        None,
        &mut counter_ids(),
    );
    assert!(knot_remaps.is_empty());
}

// --- shaftTaperGeometry.test.ts (the brace span and bucket cases need their modules) ---

#[test]
fn a_taper_is_shared_out_across_the_pieces_of_a_curve() {
    // Five points, so four pieces spanning 2mm down to 1mm.
    let width_at = taper_width_along(5, 2.0, Some(1.0));
    assert_eq!(width_at(0), 2.0);
    assert_eq!(width_at(2), 1.5);
    assert_eq!(width_at(4), 1.0);
}

#[test]
fn a_shaft_with_one_width_reads_the_same_at_every_point() {
    let width_at = taper_width_along(5, 2.0, None);
    assert_eq!(width_at(0), 2.0);
    assert_eq!(width_at(4), 2.0);
}

#[test]
fn a_single_piece_runs_the_whole_taper() {
    let width_at = taper_width_along(2, 2.0, Some(1.0));
    assert_eq!(width_at(0), 2.0);
    assert_eq!(width_at(1), 1.0);
}

// --- wire format ---

#[test]
fn a_dragonfruit_trunk_round_trips_through_its_own_json() {
    let json = r#"{
        "id": "t1", "modelId": "m", "typeId": "trunk", "origin": "overhang", "generatedBy": "autoSupport",
        "rootId": "r1",
        "segments": [
            {"id": "s1", "diameter": 1.2, "topJoint": {"id": "j1", "pos": {"x": 0, "y": 0, "z": 5}, "diameter": 1.3}},
            {"id": "s2", "diameter": 1.2, "type": "bezier",
             "controlPoint1": {"x": 0, "y": 1, "z": 6}, "controlPoint2": {"x": 0, "y": 1, "z": 8},
             "startTangent": {"x": 0, "y": 0, "z": 1}, "endTangent": {"x": 0, "y": 0, "z": 1},
             "tension": 0.5, "bias": 0.5, "resolution": 16}
        ],
        "contactCone": {
            "id": "c1", "pos": {"x": 0, "y": 0, "z": 10}, "normal": {"x": 0, "y": 0, "z": -1},
            "profile": {"type": "disk", "contactDiameterMm": 0.3, "bodyDiameterMm": 1, "lengthMm": 2.5,
                        "penetrationMm": 0.1, "diskThicknessMm": 0.1, "maxStandoffMm": 1.5,
                        "standoffAngleThreshold": 0.7853981633974483},
            "socketJointId": "j2"
        }
    }"#;
    let trunk: Trunk = serde_json::from_str(json).unwrap();
    assert!(trunk.segments[1].is_bezier());
    assert!(!trunk.segments[0].is_bezier());
    assert!(trunk.contact_cone.as_ref().unwrap().profile.is_disk());

    let back = serde_json::to_value(&trunk).unwrap();
    let original: serde_json::Value = serde_json::from_str(json).unwrap();
    assert_eq!(as_floats(back), as_floats(original));
}

/// JavaScript has one number type; compare JSON with every number as f64.
fn as_floats(value: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::Number(n) => Value::from(n.as_f64().unwrap()),
        Value::Array(items) => Value::Array(items.into_iter().map(as_floats).collect()),
        Value::Object(map) => {
            Value::Object(map.into_iter().map(|(k, v)| (k, as_floats(v))).collect())
        }
        other => other,
    }
}
