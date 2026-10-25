//! Parity with DragonFruit itself: `fixtures/df_primitives.json` holds the
//! outputs of DragonFruit's own primitive functions over seeded inputs
//! (`scripts/df-oracle/golden_primitives.ts`, DragonFruit f331fc55). Every case
//! is re-run through the native port and compared.
//!
//! The inputs are read through the port's own serde types, so this also holds
//! the wire format to DragonFruit's. Numbers must agree bit for bit: the port
//! does the same IEEE arithmetic in the same order, and JSON round-trips f64
//! exactly (`float_roundtrip`). Should a platform libm ever differ from V8 in
//! the last ulp of `acos`/`sin`, raise `REL_TOL` rather than the arithmetic.

use serde::de::DeserializeOwned;
use serde_json::Value;

use support_gen_df::supports::auto_support::constants::influence_radius_mm;
use support_gen_df::supports::curves::bezier_utils::*;
use support_gen_df::supports::support_primitives::contact_cone::contact_cone_utils::*;
use support_gen_df::supports::support_primitives::contact_cone::types::{
    ContactCone, ContactDiskProfile,
};
use support_gen_df::supports::support_primitives::contact_disk::contact_disk_interaction::*;
use support_gen_df::supports::support_primitives::contact_disk::contact_disk_utils::*;
use support_gen_df::supports::support_primitives::joint::joint_utils::find_closest_segment;
use support_gen_df::supports::support_primitives::knot::knot_utils::*;
use support_gen_df::supports::support_primitives::knot::segment_endpoints::*;
use support_gen_df::supports::support_primitives::shaft::shaft_taper_geometry::taper_width_along;
use support_gen_df::supports::support_type_registry::{
    SupportPlacementMetric, select_type_for_placement,
};
use support_gen_df::supports::types::*;
use support_gen_df::three::{Quaternion, Ray};

const REL_TOL: f64 = 0.0;

fn fixture() -> Value {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/df_primitives.json"
    ))
    .unwrap();
    serde_json::from_str(&text).unwrap()
}

fn cases<'a>(fixture: &'a Value, name: &str) -> &'a [Value] {
    let cases = fixture["cases"][name]
        .as_array()
        .unwrap_or_else(|| panic!("no cases for {name}"));
    assert!(!cases.is_empty(), "no cases for {name}");
    cases
}

fn read<T: DeserializeOwned>(value: &Value) -> T {
    serde_json::from_value(value.clone()).unwrap_or_else(|e| panic!("{e}: {value}"))
}

fn opt<T: DeserializeOwned>(value: &Value) -> Option<T> {
    if value.is_null() {
        None
    } else {
        Some(read(value))
    }
}

/// Compares two JSON graphs: numbers to `REL_TOL`, everything else exactly.
/// A field DragonFruit left `undefined` is absent; the port must omit it too.
fn assert_close(actual: &Value, expected: &Value, path: &str) {
    match (actual, expected) {
        (Value::Number(a), Value::Number(e)) => {
            let (a, e) = (a.as_f64().unwrap(), e.as_f64().unwrap());
            let scale = a.abs().max(e.abs()).max(1.0);
            assert!(
                (a - e).abs() <= REL_TOL * scale,
                "{path}: port {a} vs DragonFruit {e}"
            );
        }
        (Value::Object(a), Value::Object(e)) => {
            let mut a_keys: Vec<&String> = a.keys().collect();
            let mut e_keys: Vec<&String> = e.keys().collect();
            a_keys.sort();
            e_keys.sort();
            assert_eq!(a_keys, e_keys, "{path}: fields differ");
            for (key, e_value) in e {
                assert_close(&a[key], e_value, &format!("{path}.{key}"));
            }
        }
        (Value::Array(a), Value::Array(e)) => {
            assert_eq!(a.len(), e.len(), "{path}: lengths differ");
            for (i, (a, e)) in a.iter().zip(e).enumerate() {
                assert_close(a, e, &format!("{path}[{i}]"));
            }
        }
        _ => assert_eq!(actual, expected, "{path}"),
    }
}

fn check(name: &str, i: usize, actual: impl serde::Serialize, expected: &Value) {
    assert_close(
        &serde_json::to_value(actual).unwrap(),
        expected,
        &format!("{name}[{i}]"),
    );
}

fn quat(q: Quaternion) -> Value {
    serde_json::json!({ "x": q.x, "y": q.y, "z": q.z, "w": q.w })
}

#[test]
fn contact_disk_and_cone_match_dragonfruit() {
    let fixture = fixture();
    for (i, c) in cases(&fixture, "calculateDiskThickness").iter().enumerate() {
        let profile: ContactDiskProfile = read(&c["profile"]);
        let result = calculate_disk_thickness(
            read(&c["surfaceNormal"]),
            read(&c["coneAxis"]),
            Some(&profile),
        );
        check("calculateDiskThickness", i, result, &c["result"]);
    }
    for (i, c) in cases(&fixture, "getFinalSocketPosition").iter().enumerate() {
        let cone: ContactCone = read(&c["cone"]);
        check(
            "getFinalSocketPosition",
            i,
            get_final_socket_position(&cone),
            &c["result"],
        );
    }
    for (i, c) in cases(&fixture, "getContactConeLowestZ").iter().enumerate() {
        let cone: ContactCone = read(&c["cone"]);
        check(
            "getContactConeLowestZ",
            i,
            get_contact_cone_lowest_z(&cone),
            &c["result"],
        );
    }
    for (i, c) in cases(&fixture, "getConeQuaternion").iter().enumerate() {
        check(
            "getConeQuaternion",
            i,
            quat(get_cone_quaternion(read(&c["normal"]))),
            &c["result"],
        );
    }
    for (i, c) in cases(&fixture, "getDiskRotation").iter().enumerate() {
        check(
            "getDiskRotation",
            i,
            quat(get_disk_rotation(read(&c["normal"]))),
            &c["result"],
        );
    }
    for (i, c) in cases(&fixture, "recomputeContactConeForMovedDisk")
        .iter()
        .enumerate()
    {
        let cone: ContactCone = read(&c["cone"]);
        let result = recompute_contact_cone_for_moved_disk(
            &cone,
            read(&c["nextContactPos"]),
            read(&c["nextSurfaceNormal"]),
            opt(&c["fixedSocketPos"]),
            None,
        );
        check("recomputeContactConeForMovedDisk", i, result, &c["result"]);
    }
    for (i, c) in cases(&fixture, "moveDiskKeepingConeConnection")
        .iter()
        .enumerate()
    {
        let disk: ContactDisk = read(&c["disk"]);
        let result = move_disk_keeping_cone_connection(
            &disk,
            read(&c["nextContactPos"]),
            read(&c["nextSurfaceNormal"]),
        );
        check("moveDiskKeepingConeConnection", i, result, &c["result"]);
    }
}

#[test]
fn bezier_utils_match_dragonfruit() {
    let fixture = fixture();
    let p = |c: &Value, k: &str| -> Vec3 { read(&c[k]) };
    for (i, c) in cases(&fixture, "calculateBezierControlPoints")
        .iter()
        .enumerate()
    {
        let result = calculate_bezier_control_points(
            p(c, "p0"),
            p(c, "p3"),
            p(c, "startTangent"),
            p(c, "endTangent"),
            read(&c["tension"]),
            read(&c["bias"]),
        );
        check("calculateBezierControlPoints", i, result, &c["result"]);
    }
    for (i, c) in cases(&fixture, "getBezierPointAtT").iter().enumerate() {
        let result = get_bezier_point_at_t(
            p(c, "p0"),
            p(c, "p1"),
            p(c, "p2"),
            p(c, "p3"),
            read(&c["t"]),
        );
        check("getBezierPointAtT", i, result, &c["result"]);
    }
    for (i, c) in cases(&fixture, "getBezierTangentAtT").iter().enumerate() {
        let result = get_bezier_tangent_at_t(
            p(c, "p0"),
            p(c, "p1"),
            p(c, "p2"),
            p(c, "p3"),
            read(&c["t"]),
        );
        check("getBezierTangentAtT", i, result, &c["result"]);
    }
    for (i, c) in cases(&fixture, "subdivideCubicBezier").iter().enumerate() {
        let result = subdivide_cubic_bezier(
            p(c, "p0"),
            p(c, "p1"),
            p(c, "p2"),
            p(c, "p3"),
            read(&c["t"]),
        );
        check("subdivideCubicBezier", i, result, &c["result"]);
    }
    for (i, c) in cases(&fixture, "bezierToLineSegments").iter().enumerate() {
        let result = bezier_to_line_segments(
            p(c, "p0"),
            p(c, "p1"),
            p(c, "p2"),
            p(c, "p3"),
            opt(&c["resolution"]),
        );
        check("bezierToLineSegments", i, result, &c["result"]);
    }
    for (i, c) in cases(&fixture, "calculateAdaptiveBezierResolution")
        .iter()
        .enumerate()
    {
        let o = &c["options"];
        let options = AdaptiveBezierResolutionOptions {
            min_resolution: opt(&o["minResolution"]),
            max_resolution: opt(&o["maxResolution"]),
            target_chord_length_mm: opt(&o["targetChordLengthMm"]),
            curvature_weight: opt(&o["curvatureWeight"]),
        };
        let result = calculate_adaptive_bezier_resolution(
            p(c, "p0"),
            p(c, "p1"),
            p(c, "p2"),
            p(c, "p3"),
            options,
        );
        check("calculateAdaptiveBezierResolution", i, result, &c["result"]);
    }
}

#[test]
fn knot_utils_match_dragonfruit() {
    let fixture = fixture();
    for (i, c) in cases(&fixture, "projectOntoSegment").iter().enumerate() {
        let ray = Ray::new(read(&c["origin"]), read(&c["direction"]));
        let result = project_onto_segment(ray, read(&c["start"]), read(&c["end"]));
        check(
            "projectOntoSegment",
            i,
            serde_json::json!({ "point": result.point, "t": result.t }),
            &c["result"],
        );
    }
    for (i, c) in cases(&fixture, "remapKnotAcrossSplit").iter().enumerate() {
        let knot: Knot = read(&c["knot"]);
        let result = remap_knot_across_split(&knot, "seg", "seg", "top", read(&c["splitT"]))
            .map(|r| serde_json::json!({ "knotId": r.knot_id, "parentShaftId": r.parent_shaft_id, "t": r.t }));
        check("remapKnotAcrossSplit", i, result, &c["result"]);
    }
}

#[test]
fn segment_endpoints_and_closest_segment_match_dragonfruit() {
    let fixture = fixture();
    for (i, c) in cases(&fixture, "resolveSegmentEndpoints")
        .iter()
        .enumerate()
    {
        let root: Roots = read(&c["root"]);
        let host_knot: Knot = read(&c["hostKnot"]);
        let hosts = EndpointHosts {
            root: c["giveRoot"].as_bool().unwrap().then_some(&root),
            host_knot: c["giveKnot"].as_bool().unwrap().then_some(&host_knot),
        };
        let entity = &c["entity"];
        let (trunk, branch, stick, kickstand);
        let shaft = match entity["typeId"].as_str().unwrap() {
            "trunk" => {
                trunk = read::<Trunk>(entity);
                ShaftEntity::Trunk(&trunk)
            }
            "branch" => {
                branch = read::<Branch>(entity);
                ShaftEntity::Branch(&branch)
            }
            "stick" => {
                stick = read::<Stick>(entity);
                ShaftEntity::Stick(&stick)
            }
            "kickstand" => {
                kickstand = read::<Kickstand>(entity);
                ShaftEntity::Kickstand(&kickstand)
            }
            other => panic!("unexpected type {other}"),
        };
        let result: Vec<Option<Value>> = shaft
            .segments()
            .iter()
            .enumerate()
            .map(|(k, seg)| {
                resolve_segment_endpoints(shaft, seg, k, hosts)
                    .map(|e| serde_json::json!({ "start": e.start, "end": e.end }))
            })
            .collect();
        check("resolveSegmentEndpoints", i, result, &c["result"]);
    }
    for (i, c) in cases(&fixture, "findClosestSegment").iter().enumerate() {
        let trunk: Trunk = read(&c["entity"]);
        let root: Roots = read(&c["root"]);
        let result = find_closest_segment(&trunk, &root, read(&c["point"])).map(|found| {
            serde_json::json!({ "segmentId": found.segment.id, "t": found.t, "pointOnLine": found.point_on_line })
        });
        check("findClosestSegment", i, result, &c["result"]);
    }
}

#[test]
fn registry_and_constants_match_dragonfruit() {
    let fixture = fixture();
    for (i, c) in cases(&fixture, "taperWidthAlong").iter().enumerate() {
        let point_count = c["pointCount"].as_u64().unwrap() as usize;
        let width_at = taper_width_along(point_count, read(&c["start"]), opt(&c["end"]));
        let result: Vec<f64> = (0..point_count + 2).map(width_at).collect();
        check("taperWidthAlong", i, result, &c["result"]);
    }
    for (i, c) in cases(&fixture, "influenceRadiusMm").iter().enumerate() {
        check(
            "influenceRadiusMm",
            i,
            influence_radius_mm(read(&c["diffZ"])),
            &c["result"],
        );
    }
    for (i, c) in cases(&fixture, "selectTypeForPlacement").iter().enumerate() {
        let metric = match c["metric"].as_str().unwrap() {
            "tipHeight" => SupportPlacementMetric::TipHeight,
            _ => SupportPlacementMetric::ContactSpan,
        };
        let cutoff: Option<f64> = opt(&c["cutoff"]);
        let result = select_type_for_placement(metric, read(&c["value"]), &|_| cutoff);
        check("selectTypeForPlacement", i, result, &c["result"]);
    }
}
